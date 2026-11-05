//! gfxstream's host renderer on macOS (`libgfxstream_backend.dylib`, on Vulkan through
//! MoltenVK), loaded at run time from the app bundle's `Frameworks` folder.
//!
//! The guest's graphics streams are served through it, and each task's newest frame comes
//! back read to memory (`dsa_gfx_set_display_pixels_callback`), which the window manager
//! writes into the task's shared surface for its window host.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::frames::{bgra_from_rgba, OwnedFrame};

type StreamRead = extern "C" fn(*mut c_void, *mut c_void, usize) -> c_int;
type StreamWrite = extern "C" fn(*mut c_void, *const c_void, usize) -> c_int;
type PixelsCallback = extern "C" fn(*mut c_void, u32, u32, u32, *const u8);

/// Where a display's frames go.
pub type FrameSink = Arc<dyn Fn(OwnedFrame) + Send + Sync>;

type Sinks = Mutex<HashMap<u32, FrameSink>>;

struct Api {
    init: extern "C" fn(u32, u32) -> c_int,
    teardown: extern "C" fn(),
    serve_stream: extern "C" fn(StreamRead, StreamWrite, *mut c_void) -> c_int,
    process_pipe: extern "C" fn(StreamRead, StreamWrite, *mut c_void) -> c_int,
    set_pixels: extern "C" fn(Option<PixelsCallback>, *mut c_void) -> c_int,
}

/// The loaded renderer.
pub struct Renderer {
    api: Api,
    /// Each display's sink. Boxed so the address the renderer holds stays put.
    sinks: Box<Sinks>,
}

// The renderer's entry points are thread-safe; the library is never unloaded.
unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

unsafe fn symbol<T: Copy>(library: *mut c_void, name: &str) -> Result<T, String> {
    let c_name = CString::new(name).expect("symbol names have no NUL");
    let found = libc::dlsym(library, c_name.as_ptr());
    if found.is_null() {
        return Err(format!(
            "the graphics renderer has no {name}; it is older than this host"
        ));
    }
    Ok(std::mem::transmute_copy::<*mut c_void, T>(&found))
}

extern "C" fn read_stream(user: *mut c_void, buffer: *mut c_void, count: usize) -> c_int {
    let stream = unsafe { &mut *(user as *mut &mut dyn ReadWrite) };
    let buffer = unsafe { std::slice::from_raw_parts_mut(buffer as *mut u8, count) };
    match stream.read(buffer) {
        Ok(n) => n.min(c_int::MAX as usize) as c_int,
        Err(_) => -1,
    }
}

extern "C" fn write_stream(user: *mut c_void, buffer: *const c_void, count: usize) -> c_int {
    let stream = unsafe { &mut *(user as *mut &mut dyn ReadWrite) };
    let buffer = unsafe { std::slice::from_raw_parts(buffer as *const u8, count) };
    match stream.write_all(buffer) {
        Ok(()) => count.min(c_int::MAX as usize) as c_int,
        Err(_) => -1,
    }
}

extern "C" fn on_pixels(
    context: *mut c_void,
    display_id: u32,
    width: u32,
    height: u32,
    rgba: *const u8,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if context.is_null() || rgba.is_null() {
            return;
        }
        let sinks = unsafe { &*(context as *const Sinks) };
        let Some(sink) = sinks.lock().ok().and_then(|s| s.get(&display_id).cloned()) else {
            return;
        };
        let len = width as usize * height as usize * 4;
        let rgba = unsafe { std::slice::from_raw_parts(rgba, len) };
        if let Some(frame) = bgra_from_rgba(display_id, width, height, rgba) {
            sink(frame);
        }
    }));
}

/// A stream both ways, as a vsock connection is.
pub trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

impl Renderer {
    /// Loads the renderer at `path` and starts it.
    pub fn load(path: &Path) -> Result<Renderer, String> {
        let c_path = CString::new(path.as_os_str().to_string_lossy().as_bytes())
            .map_err(|_| "the renderer's path has a NUL in it".to_string())?;
        unsafe {
            let library = libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            if library.is_null() {
                let why = libc::dlerror();
                let why = if why.is_null() {
                    String::from("unknown reason")
                } else {
                    CStr::from_ptr(why as *const c_char)
                        .to_string_lossy()
                        .into_owned()
                };
                return Err(format!(
                    "the graphics renderer at {} could not be loaded: {why}",
                    path.display()
                ));
            }
            let api = Api {
                init: symbol(library, "dsa_gfx_init")?,
                teardown: symbol(library, "dsa_gfx_teardown")?,
                serve_stream: symbol(library, "dsa_gfx_serve_stream")?,
                process_pipe: symbol(library, "dsa_gfx_process_pipe")?,
                set_pixels: symbol(library, "dsa_gfx_set_display_pixels_callback")?,
            };
            if (api.init)(1920, 1080) != 0 {
                return Err(
                    "the graphics renderer did not start: this Mac's Vulkan (MoltenVK) may be \
                     missing"
                        .into(),
                );
            }
            let sinks: Box<Sinks> = Box::new(Mutex::new(HashMap::new()));
            let context = &*sinks as *const Sinks as *mut c_void;
            if (api.set_pixels)(Some(on_pixels), context) != 0 {
                (api.teardown)();
                return Err("the graphics renderer cannot hand frames over on this Mac".into());
            }
            Ok(Renderer { api, sinks })
        }
    }

    /// Sends display `display_id`'s frames to `sink` until [`Renderer::unwatch`].
    pub fn watch(&self, display_id: u32, sink: FrameSink) {
        if let Ok(mut sinks) = self.sinks.lock() {
            sinks.insert(display_id, sink);
        }
    }

    /// Stops display `display_id`'s frames.
    pub fn unwatch(&self, display_id: u32) {
        if let Ok(mut sinks) = self.sinks.lock() {
            sinks.remove(&display_id);
        }
    }

    /// Serves one render stream from the guest until it closes.
    pub fn serve_stream(&self, mut stream: &mut dyn ReadWrite) -> Result<(), String> {
        let user = &mut stream as *mut &mut dyn ReadWrite as *mut c_void;
        match (self.api.serve_stream)(read_stream, write_stream, user) {
            0 => Ok(()),
            code => Err(format!("the render stream ended with error {code}")),
        }
    }

    /// Serves one guest process's pipe until it closes.
    pub fn serve_process(&self, mut stream: &mut dyn ReadWrite) -> Result<(), String> {
        let user = &mut stream as *mut &mut dyn ReadWrite as *mut c_void;
        match (self.api.process_pipe)(read_stream, write_stream, user) {
            0 => Ok(()),
            code => Err(format!("the process pipe ended with error {code}")),
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        (self.api.set_pixels)(None, std::ptr::null_mut());
        (self.api.teardown)();
    }
}
