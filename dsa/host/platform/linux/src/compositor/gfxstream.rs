//! gfxstream's host renderer (`libgfxstream_backend.so`), which runs the guest's Vulkan on
//! the host's GPU, and the graphics streams the guest opens to it.
//!
//! The guest opens two kinds of stream over vsock, as on Windows: the command stream (port
//! 5566), and a process pipe (port 5567) for each guest process that draws. The renderer
//! serves each on a thread of its own. Each task's finished frame is read back from the GPU
//! and handed over, newest first, to the window that shows it.

use std::ffi::{c_int, c_void};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use libloading::Library;

use super::frames::{Frame, GpuFrame, GpuImage, LatestFrames};
use crate::compositor::wayland::Wake;

/// The command stream's port.
pub const COMMAND_PORT: u32 = 5566;
/// The process pipe's port.
pub const PROCESS_PIPE_PORT: u32 = 5567;

type ReadFn = unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> c_int;
type WriteFn = unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> c_int;
type PixelsFn = unsafe extern "C" fn(*mut c_void, u32, u32, u32, *const u8);
type DmabufFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32, c_int, u32, u32, u32, u64) -> c_int;
type DmabufFrameFn = unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32);

/// Where the frames go.
pub struct FrameSink {
    pub frames: Arc<Mutex<LatestFrames>>,
    pub waker: Arc<dyn Wake>,
    /// Where the GPU images go, when the desktop takes dmabufs; `None` reads every frame
    /// back.
    pub images: Option<Arc<Mutex<Vec<(GpuImage, OwnedFd)>>>>,
}

// The backend's C callback names every property of the image.
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn on_dmabuf(
    context: *mut c_void,
    display_id: u32,
    slot: u32,
    width: u32,
    height: u32,
    fd: c_int,
    stride: u32,
    offset: u32,
    fourcc: u32,
    modifier: u64,
) -> c_int {
    if context.is_null() || fd < 0 {
        return -1;
    }
    // SAFETY: the backend hands over the descriptor, ours from here on.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: `context` is the sink `load` leaked, which lives as long as the process.
    let sink = unsafe { &*(context as *const FrameSink) };
    let Some(images) = &sink.images else {
        return -1;
    };
    let image = GpuImage {
        display_id,
        slot,
        width,
        height,
        stride,
        offset,
        fourcc,
        modifier,
    };
    match images.lock() {
        Ok(mut images) => images.push((image, fd)),
        Err(_) => return -1,
    }
    sink.waker.wake();
    0
}

unsafe extern "C" fn on_dmabuf_frame(
    context: *mut c_void,
    display_id: u32,
    slot: u32,
    width: u32,
    height: u32,
) {
    if context.is_null() {
        return;
    }
    // SAFETY: `context` is the sink `load` leaked, which lives as long as the process.
    let sink = unsafe { &*(context as *const FrameSink) };
    if let Ok(mut frames) = sink.frames.lock() {
        frames.put_gpu(
            display_id,
            GpuFrame {
                slot,
                width,
                height,
            },
        );
    }
    sink.waker.wake();
}

unsafe extern "C" fn on_pixels(
    context: *mut c_void,
    display_id: u32,
    width: u32,
    height: u32,
    rgba: *const u8,
) {
    if context.is_null() || rgba.is_null() {
        return;
    }
    // SAFETY: `context` is the sink `present` leaked, which lives as long as the process.
    let sink = unsafe { &*(context as *const FrameSink) };
    let len = width as usize * height as usize * 4;
    // SAFETY: the backend hands `width * height` RGBA pixels, valid during the call.
    let rgba = unsafe { std::slice::from_raw_parts(rgba, len) };
    if let Some(frame) = Frame::from_rgba(width, height, rgba) {
        if let Ok(mut frames) = sink.frames.lock() {
            frames.put(display_id, frame);
        }
        sink.waker.wake();
    }
}

/// The loaded renderer.
pub struct Renderer {
    _library: Library,
    serve_stream: unsafe extern "C" fn(ReadFn, WriteFn, *mut c_void) -> c_int,
    process_pipe: unsafe extern "C" fn(ReadFn, WriteFn, *mut c_void) -> c_int,
}

/// Where the renderer is: `GFXSTREAM_BACKEND_PATH` when set, else beside this program, else
/// the library path.
pub fn backend_path() -> PathBuf {
    if let Some(path) = std::env::var_os("GFXSTREAM_BACKEND_PATH") {
        return PathBuf::from(path);
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        let beside = dir.join("libgfxstream_backend.so");
        if beside.exists() {
            return beside;
        }
    }
    PathBuf::from("libgfxstream_backend.so")
}

impl Renderer {
    /// Loads the renderer at `path`, starts it, and hands each task's frames to `sink`.
    pub fn load(path: &Path, sink: FrameSink) -> Result<Renderer, String> {
        // SAFETY: the renderer's library initialises nothing when loaded.
        let library = unsafe { Library::new(path) }
            .map_err(|error| format!("cannot load {}: {error}", path.display()))?;
        // SAFETY: each symbol has the type the backend's header gives it.
        unsafe {
            let init: unsafe extern "C" fn(u32, u32) -> c_int = *library
                .get(b"dsa_gfx_init\0")
                .map_err(|error| format!("the renderer has no dsa_gfx_init: {error}"))?;
            let serve_stream = *library
                .get(b"dsa_gfx_serve_stream\0")
                .map_err(|error| format!("the renderer has no dsa_gfx_serve_stream: {error}"))?;
            let process_pipe = *library
                .get(b"dsa_gfx_process_pipe\0")
                .map_err(|error| format!("the renderer has no dsa_gfx_process_pipe: {error}"))?;
            let set_pixels: unsafe extern "C" fn(Option<PixelsFn>, *mut c_void) -> c_int = *library
                .get(b"dsa_gfx_set_display_pixels_callback\0")
                .map_err(|error| {
                    format!("the renderer cannot hand frames to this host: {error}")
                })?;
            let code = init(1280, 720);
            if code != 0 {
                return Err(format!("the renderer did not start (code {code})"));
            }
            let set_dmabuf: Option<
                unsafe extern "C" fn(
                    Option<DmabufFn>,
                    Option<DmabufFrameFn>,
                    Option<PixelsFn>,
                    *mut c_void,
                ) -> c_int,
            > = library
                .get(b"dsa_gfx_set_display_dmabuf_callbacks\0")
                .ok()
                .map(|symbol| *symbol);
            let gpu = sink.images.is_some();
            let sink = Box::into_raw(Box::new(sink));
            let code = match set_dmabuf {
                Some(set_dmabuf) if gpu => set_dmabuf(
                    Some(on_dmabuf),
                    Some(on_dmabuf_frame),
                    Some(on_pixels),
                    sink.cast(),
                ),
                _ => set_pixels(Some(on_pixels), sink.cast()),
            };
            if code != 0 {
                drop(Box::from_raw(sink));
                return Err(format!("the renderer cannot present frames (code {code})"));
            }
            Ok(Renderer {
                _library: library,
                serve_stream,
                process_pipe,
            })
        }
    }

    /// Serves one stream until the guest closes it.
    pub fn serve(&self, port: u32, stream: OwnedFd) {
        let fd = stream.as_raw_fd();
        let mut context = fd;
        let user = (&mut context as *mut RawFd).cast();
        // SAFETY: the callbacks read `user` as the stream's descriptor, which stays open
        // until this returns.
        unsafe {
            if port == PROCESS_PIPE_PORT {
                (self.process_pipe)(read_stream, write_stream, user);
            } else {
                (self.serve_stream)(read_stream, write_stream, user);
            }
        }
        drop(stream);
    }
}

// SAFETY: the renderer's entry points may be called from any thread.
unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

/// The backend's read: some bytes, 0 when none came within a moment (it asks again), -1
/// when the stream ended.
unsafe extern "C" fn read_stream(user: *mut c_void, buf: *mut c_void, count: usize) -> c_int {
    if user.is_null() || buf.is_null() || count == 0 {
        return 0;
    }
    // SAFETY: `user` points at the descriptor `serve` keeps.
    let fd = unsafe { *(user as *const RawFd) };
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd.
    let ready = unsafe { libc::poll(&mut poll, 1, 50) };
    if ready == 0 {
        return 0;
    }
    if ready < 0 {
        return if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            0
        } else {
            -1
        };
    }
    let count = count.min(c_int::MAX as usize);
    // SAFETY: `buf` has room for `count` bytes.
    let read = unsafe { libc::read(fd, buf, count) };
    match read {
        n if n > 0 => n as c_int,
        _ => -1,
    }
}

/// The backend's write: all of `count` bytes, or -1.
unsafe extern "C" fn write_stream(user: *mut c_void, buf: *const c_void, count: usize) -> c_int {
    if user.is_null() {
        return -1;
    }
    if buf.is_null() || count == 0 {
        return 0;
    }
    // SAFETY: `user` points at the descriptor `serve` keeps.
    let fd = unsafe { *(user as *const RawFd) };
    let mut written = 0usize;
    while written < count {
        // SAFETY: writes from within `buf`'s `count` bytes.
        let n = unsafe { libc::write(fd, buf.cast::<u8>().add(written).cast(), count - written) };
        if n <= 0 {
            if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return -1;
        }
        written += n as usize;
    }
    count.min(c_int::MAX as usize) as c_int
}

/// How the guest's graphics streams reach this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listen {
    /// vsock, from the guest's virtio socket device.
    Vsock,
    /// Unix sockets named `gfx-<port>.sock` in this folder: a virtual machine monitor that
    /// proxies the guest's vsock to Unix sockets (cloud-hypervisor, Firecracker) puts them
    /// there.
    UnixDir(PathBuf),
}

impl Listen {
    /// `DSA_GRAPHICS_SOCKET_DIR` when set, else vsock.
    pub fn from_env() -> Listen {
        match std::env::var_os("DSA_GRAPHICS_SOCKET_DIR") {
            Some(dir) if !dir.is_empty() => Listen::UnixDir(PathBuf::from(dir)),
            _ => Listen::Vsock,
        }
    }
}

/// A listening socket.
pub struct Listener {
    fd: OwnedFd,
}

impl Listener {
    pub fn bind(listen: &Listen, port: u32) -> io::Result<Listener> {
        match listen {
            Listen::Vsock => {
                // SAFETY: a new socket, owned from here on.
                let fd = unsafe {
                    let raw =
                        libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
                    if raw < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    OwnedFd::from_raw_fd(raw)
                };
                // SAFETY: a zeroed sockaddr_vm is valid; the fields are set below.
                let mut address: libc::sockaddr_vm = unsafe { std::mem::zeroed() };
                address.svm_family = libc::AF_VSOCK as libc::sa_family_t;
                address.svm_port = port;
                address.svm_cid = libc::VMADDR_CID_ANY;
                // SAFETY: binds the socket to a whole sockaddr_vm.
                let bound = unsafe {
                    libc::bind(
                        fd.as_raw_fd(),
                        (&address as *const libc::sockaddr_vm).cast(),
                        std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
                    )
                };
                if bound != 0 {
                    return Err(io::Error::last_os_error());
                }
                // SAFETY: a bound socket.
                if unsafe { libc::listen(fd.as_raw_fd(), 16) } != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Listener { fd })
            }
            Listen::UnixDir(dir) => {
                let path = dir.join(format!("gfx-{port}.sock"));
                let _ = std::fs::remove_file(&path);
                let listener = std::os::unix::net::UnixListener::bind(&path)?;
                Ok(Listener {
                    fd: OwnedFd::from(listener),
                })
            }
        }
    }

    /// Waits for the next stream.
    pub fn accept(&self) -> io::Result<OwnedFd> {
        loop {
            // SAFETY: accepts on a listening socket; the new descriptor is owned from here.
            let raw = unsafe {
                libc::accept4(
                    self.fd.as_raw_fd(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_CLOEXEC,
                )
            };
            if raw >= 0 {
                // SAFETY: a new descriptor no one else owns.
                return Ok(unsafe { OwnedFd::from_raw_fd(raw) });
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

/// Accepts the guest's streams on `port` for as long as the process runs, serving each on a
/// thread of its own. `log` hears what went wrong.
pub fn serve_port(
    renderer: Arc<Renderer>,
    listen: Listen,
    port: u32,
    log: Arc<dyn Fn(&str) + Send + Sync>,
) {
    std::thread::spawn(move || {
        let listener = match Listener::bind(&listen, port) {
            Ok(listener) => listener,
            Err(error) => {
                log(&format!(
                    "graphics: cannot listen for the guest on port {port}: {error}"
                ));
                return;
            }
        };
        loop {
            match listener.accept() {
                Ok(stream) => {
                    let renderer = renderer.clone();
                    std::thread::spawn(move || renderer.serve(port, stream));
                }
                Err(error) => {
                    log(&format!("graphics: port {port} stopped: {error}"));
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        }
    });
}
