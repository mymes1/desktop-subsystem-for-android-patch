//! Reading a GPU frame back to memory, in whatever layout the GPU keeps it.
//!
//! The renderer's dmabufs go to the desktop as they are, so the compositor never draws them
//! itself. Two things need their pixels: the title bar's band, which takes the app's top color
//! when the app leaves it empty (two rows, sampled now and then), and a screenshot (every row
//! under the band, once). A dmabuf in the linear layout is mapped and read by the CPU; any
//! other layout (a GPU's tiled or compressed ones) is imported with its modifier into an
//! OpenGL ES context of the compositor's own (EGL, `EGL_EXT_image_dma_buf_import`), bound to
//! a framebuffer and read with `glReadPixels`, which untiles it on the GPU.
//!
//! EGL and OpenGL ES are loaded at run time, the first time a frame needs them, so a desktop
//! without them still shows its windows: a frame it cannot read then gives no screenshot and
//! an unfilled band, and the log says why.

use std::ffi::{c_char, c_void, CStr};
use std::os::fd::{AsRawFd, BorrowedFd};

use libloading::Library;

use super::frames::{
    fourcc, rgba_to_xrgb, row_runs, GpuImage, DRM_FORMAT_MOD_INVALID, DRM_FORMAT_MOD_LINEAR,
};

type EglDisplay = *mut c_void;
type EglConfig = *mut c_void;
type EglContext = *mut c_void;
type EglImage = *mut c_void;
type EglDevice = *mut c_void;

const EGL_NONE: i32 = 0x3038;
const EGL_EXTENSIONS: i32 = 0x3055;
const EGL_HEIGHT: i32 = 0x3056;
const EGL_WIDTH: i32 = 0x3057;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_OPENGL_ES2_BIT: i32 = 0x0004;
const EGL_OPENGL_ES_API: u32 = 0x30A0;
const EGL_CONTEXT_CLIENT_VERSION: i32 = 0x3098;
const EGL_LINUX_DMA_BUF_EXT: u32 = 0x3270;
const EGL_LINUX_DRM_FOURCC_EXT: i32 = 0x3271;
const EGL_DMA_BUF_PLANE0_FD_EXT: i32 = 0x3272;
const EGL_DMA_BUF_PLANE0_OFFSET_EXT: i32 = 0x3273;
const EGL_DMA_BUF_PLANE0_PITCH_EXT: i32 = 0x3274;
const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: i32 = 0x3443;
const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: i32 = 0x3444;
const EGL_PLATFORM_SURFACELESS_MESA: u32 = 0x31DD;
const EGL_PLATFORM_DEVICE_EXT: u32 = 0x313F;

const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_FRAMEBUFFER: u32 = 0x8D40;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
const GL_PACK_ALIGNMENT: u32 = 0x0D05;
const GL_RGBA: u32 = 0x1908;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_NO_ERROR: u32 = 0;

/// `DMA_BUF_IOCTL_SYNC` (`_IOW('b', 0, struct dma_buf_sync)`), and its flags.
const DMA_BUF_IOCTL_SYNC: libc::c_ulong = 0x4008_6200;
const DMA_BUF_SYNC_READ: u64 = 1;
const DMA_BUF_SYNC_END: u64 = 4;

/// Rows of a frame read back: `width` pixels each, `XRGB8888`, in the order asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rows {
    pub width: u32,
    pub count: u32,
    pub pixels: Vec<u8>,
}

struct Api {
    _egl: Library,
    _gles: Library,
    get_proc_address: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    get_display: unsafe extern "C" fn(*mut c_void) -> EglDisplay,
    initialize: unsafe extern "C" fn(EglDisplay, *mut i32, *mut i32) -> u32,
    terminate: unsafe extern "C" fn(EglDisplay) -> u32,
    query_string: unsafe extern "C" fn(EglDisplay, i32) -> *const c_char,
    bind_api: unsafe extern "C" fn(u32) -> u32,
    choose_config:
        unsafe extern "C" fn(EglDisplay, *const i32, *mut EglConfig, i32, *mut i32) -> u32,
    create_context:
        unsafe extern "C" fn(EglDisplay, EglConfig, EglContext, *const i32) -> EglContext,
    destroy_context: unsafe extern "C" fn(EglDisplay, EglContext) -> u32,
    make_current: unsafe extern "C" fn(EglDisplay, *mut c_void, *mut c_void, EglContext) -> u32,
    get_error: unsafe extern "C" fn() -> i32,
    gen_textures: unsafe extern "C" fn(i32, *mut u32),
    delete_textures: unsafe extern "C" fn(i32, *const u32),
    bind_texture: unsafe extern "C" fn(u32, u32),
    gen_framebuffers: unsafe extern "C" fn(i32, *mut u32),
    delete_framebuffers: unsafe extern "C" fn(i32, *const u32),
    bind_framebuffer: unsafe extern "C" fn(u32, u32),
    framebuffer_texture_2d: unsafe extern "C" fn(u32, u32, u32, u32, i32),
    check_framebuffer_status: unsafe extern "C" fn(u32) -> u32,
    pixel_storei: unsafe extern "C" fn(u32, i32),
    read_pixels: unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void),
    gl_get_error: unsafe extern "C" fn() -> u32,
}

fn open(names: &[&str]) -> Result<Library, String> {
    let mut why = String::new();
    for name in names {
        // SAFETY: the EGL and OpenGL ES loaders run no code of their own that matters when
        // loaded.
        match unsafe { Library::new(name) } {
            Ok(library) => return Ok(library),
            Err(error) => why = format!("cannot load {name}: {error}"),
        }
    }
    Err(why)
}

impl Api {
    fn load() -> Result<Api, String> {
        let egl = open(&["libEGL.so.1", "libEGL.so"])?;
        let gles = open(&["libGLESv2.so.2", "libGLESv2.so"])?;
        // SAFETY: each symbol is looked up with the type its header gives it.
        unsafe {
            macro_rules! symbol {
                ($library:ident, $name:literal) => {
                    *$library
                        .get(concat!($name, "\0").as_bytes())
                        .map_err(|error| format!("{} is missing: {error}", $name))?
                };
            }
            Ok(Api {
                get_proc_address: symbol!(egl, "eglGetProcAddress"),
                get_display: symbol!(egl, "eglGetDisplay"),
                initialize: symbol!(egl, "eglInitialize"),
                terminate: symbol!(egl, "eglTerminate"),
                query_string: symbol!(egl, "eglQueryString"),
                bind_api: symbol!(egl, "eglBindAPI"),
                choose_config: symbol!(egl, "eglChooseConfig"),
                create_context: symbol!(egl, "eglCreateContext"),
                destroy_context: symbol!(egl, "eglDestroyContext"),
                make_current: symbol!(egl, "eglMakeCurrent"),
                get_error: symbol!(egl, "eglGetError"),
                gen_textures: symbol!(gles, "glGenTextures"),
                delete_textures: symbol!(gles, "glDeleteTextures"),
                bind_texture: symbol!(gles, "glBindTexture"),
                gen_framebuffers: symbol!(gles, "glGenFramebuffers"),
                delete_framebuffers: symbol!(gles, "glDeleteFramebuffers"),
                bind_framebuffer: symbol!(gles, "glBindFramebuffer"),
                framebuffer_texture_2d: symbol!(gles, "glFramebufferTexture2D"),
                check_framebuffer_status: symbol!(gles, "glCheckFramebufferStatus"),
                pixel_storei: symbol!(gles, "glPixelStorei"),
                read_pixels: symbol!(gles, "glReadPixels"),
                gl_get_error: symbol!(gles, "glGetError"),
                _egl: egl,
                _gles: gles,
            })
        }
    }

    /// An extension function, by name; `None` when the driver has none of that name.
    unsafe fn extension<T: Copy>(&self, name: &CStr) -> Option<T> {
        let address = (self.get_proc_address)(name.as_ptr());
        if address.is_null() {
            return None;
        }
        debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
        Some(std::mem::transmute_copy(&address))
    }
}

/// Whether the space separated extension list `list` names `name`.
pub fn has_extension(list: &str, name: &str) -> bool {
    list.split_ascii_whitespace().any(|item| item == name)
}

type GetPlatformDisplayFn = unsafe extern "C" fn(u32, *mut c_void, *const i32) -> EglDisplay;
type QueryDevicesFn = unsafe extern "C" fn(i32, *mut EglDevice, *mut i32) -> u32;
type CreateImageFn =
    unsafe extern "C" fn(EglDisplay, EglContext, u32, *mut c_void, *const i32) -> EglImage;
type DestroyImageFn = unsafe extern "C" fn(EglDisplay, EglImage) -> u32;
type ImageTargetTextureFn = unsafe extern "C" fn(u32, EglImage);

/// One GPU image imported into the context, kept while the image is the renderer's.
pub struct Imported {
    image: EglImage,
    texture: u32,
    framebuffer: u32,
}

/// The compositor's own OpenGL ES context, for reading GPU frames back.
pub struct DmabufReader {
    api: Api,
    display: EglDisplay,
    context: EglContext,
    create_image: CreateImageFn,
    destroy_image: DestroyImageFn,
    image_target_texture: ImageTargetTextureFn,
    modifiers: bool,
}

impl Drop for DmabufReader {
    fn drop(&mut self) {
        // SAFETY: the context and display this reader made.
        unsafe {
            (self.api.make_current)(
                self.display,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            (self.api.destroy_context)(self.display, self.context);
            (self.api.terminate)(self.display);
        }
    }
}

impl DmabufReader {
    /// Loads EGL and OpenGL ES and makes a context with no surface: on Mesa's surfaceless
    /// platform, else on the first EGL device, else on the default display.
    pub fn new() -> Result<DmabufReader, String> {
        let api = Api::load()?;
        // SAFETY: EGL is called as its specification says, with lists ended by EGL_NONE.
        unsafe {
            let client = (api.query_string)(std::ptr::null_mut(), EGL_EXTENSIONS);
            let client = if client.is_null() {
                String::new()
            } else {
                CStr::from_ptr(client).to_string_lossy().into_owned()
            };
            let platform: Option<GetPlatformDisplayFn> = api
                .extension(c"eglGetPlatformDisplayEXT")
                .or_else(|| api.extension(c"eglGetPlatformDisplay"));
            let mut display: EglDisplay = std::ptr::null_mut();
            if let Some(platform) = platform {
                if has_extension(&client, "EGL_MESA_platform_surfaceless") {
                    display = platform(
                        EGL_PLATFORM_SURFACELESS_MESA,
                        std::ptr::null_mut(),
                        std::ptr::null(),
                    );
                }
                if display.is_null() && has_extension(&client, "EGL_EXT_platform_device") {
                    if let Some(query) = api.extension::<QueryDevicesFn>(c"eglQueryDevicesEXT") {
                        let mut device: EglDevice = std::ptr::null_mut();
                        let mut count = 0;
                        if query(1, &mut device, &mut count) != 0 && count > 0 {
                            display = platform(EGL_PLATFORM_DEVICE_EXT, device, std::ptr::null());
                        }
                    }
                }
            }
            if display.is_null() {
                display = (api.get_display)(std::ptr::null_mut());
            }
            if display.is_null() {
                return Err("EGL has no display to read GPU frames with".into());
            }
            let (mut major, mut minor) = (0, 0);
            if (api.initialize)(display, &mut major, &mut minor) == 0 {
                return Err(format!(
                    "EGL could not start (error {:#x})",
                    (api.get_error)()
                ));
            }
            let extensions = (api.query_string)(display, EGL_EXTENSIONS);
            let extensions = if extensions.is_null() {
                String::new()
            } else {
                CStr::from_ptr(extensions).to_string_lossy().into_owned()
            };
            let fail = |why: String| -> Result<DmabufReader, String> {
                (api.terminate)(display);
                Err(why)
            };
            for needed in [
                "EGL_EXT_image_dma_buf_import",
                "EGL_KHR_surfaceless_context",
            ] {
                if !has_extension(&extensions, needed) {
                    return fail(format!("the GPU driver's EGL has no {needed}"));
                }
            }
            let modifiers = has_extension(&extensions, "EGL_EXT_image_dma_buf_import_modifiers");
            if (api.bind_api)(EGL_OPENGL_ES_API) == 0 {
                return fail("EGL has no OpenGL ES".into());
            }
            let attributes = [EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_NONE];
            let mut config: EglConfig = std::ptr::null_mut();
            let mut configs = 0;
            let chosen =
                (api.choose_config)(display, attributes.as_ptr(), &mut config, 1, &mut configs);
            if chosen == 0 || configs < 1 {
                return fail("EGL has no OpenGL ES configuration".into());
            }
            let attributes = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
            let context =
                (api.create_context)(display, config, std::ptr::null_mut(), attributes.as_ptr());
            if context.is_null() {
                return fail(format!(
                    "EGL could not make an OpenGL ES context (error {:#x})",
                    (api.get_error)()
                ));
            }
            let (Some(create_image), Some(destroy_image), Some(image_target_texture)) = (
                api.extension::<CreateImageFn>(c"eglCreateImageKHR"),
                api.extension::<DestroyImageFn>(c"eglDestroyImageKHR"),
                api.extension::<ImageTargetTextureFn>(c"glEGLImageTargetTexture2DOES"),
            ) else {
                (api.destroy_context)(display, context);
                return fail("the GPU driver cannot bind a dmabuf to a texture".into());
            };
            Ok(DmabufReader {
                api,
                display,
                context,
                create_image,
                destroy_image,
                image_target_texture,
                modifiers,
            })
        }
    }

    fn make_current(&self) -> Result<(), String> {
        // SAFETY: the reader's own display and context, with no surface.
        let made = unsafe {
            (self.api.make_current)(
                self.display,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                self.context,
            )
        };
        if made == 0 {
            return Err("the OpenGL ES context could not be made current".into());
        }
        Ok(())
    }

    /// Imports `image` (its dmabuf `fd`) and binds it to a framebuffer to read from.
    pub fn import(&self, image: &GpuImage, fd: BorrowedFd) -> Result<Imported, String> {
        self.make_current()?;
        let mut attributes = vec![
            EGL_WIDTH,
            image.width as i32,
            EGL_HEIGHT,
            image.height as i32,
            EGL_LINUX_DRM_FOURCC_EXT,
            image.fourcc as i32,
            EGL_DMA_BUF_PLANE0_FD_EXT,
            fd.as_raw_fd(),
            EGL_DMA_BUF_PLANE0_OFFSET_EXT,
            image.offset as i32,
            EGL_DMA_BUF_PLANE0_PITCH_EXT,
            image.stride as i32,
        ];
        if image.modifier != DRM_FORMAT_MOD_INVALID {
            if !self.modifiers && image.modifier != DRM_FORMAT_MOD_LINEAR {
                return Err(format!(
                    "the GPU driver's EGL cannot import the frame's layout (modifier {:#x})",
                    image.modifier
                ));
            }
            if self.modifiers {
                attributes.extend([
                    EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                    image.modifier as u32 as i32,
                    EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
                    (image.modifier >> 32) as u32 as i32,
                ]);
            }
        }
        attributes.push(EGL_NONE);
        // SAFETY: EGL and OpenGL ES are called on the current context as their
        // specifications say; every object made is either returned or deleted.
        unsafe {
            let egl_image = (self.create_image)(
                self.display,
                std::ptr::null_mut(),
                EGL_LINUX_DMA_BUF_EXT,
                std::ptr::null_mut(),
                attributes.as_ptr(),
            );
            if egl_image.is_null() {
                return Err(format!(
                    "the GPU driver could not import the frame (modifier {:#x}, error {:#x})",
                    image.modifier,
                    (self.api.get_error)()
                ));
            }
            let mut texture = 0;
            (self.api.gen_textures)(1, &mut texture);
            (self.api.bind_texture)(GL_TEXTURE_2D, texture);
            (self.image_target_texture)(GL_TEXTURE_2D, egl_image);
            let mut framebuffer = 0;
            (self.api.gen_framebuffers)(1, &mut framebuffer);
            (self.api.bind_framebuffer)(GL_FRAMEBUFFER, framebuffer);
            (self.api.framebuffer_texture_2d)(
                GL_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_TEXTURE_2D,
                texture,
                0,
            );
            let status = (self.api.check_framebuffer_status)(GL_FRAMEBUFFER);
            let imported = Imported {
                image: egl_image,
                texture,
                framebuffer,
            };
            if status != GL_FRAMEBUFFER_COMPLETE {
                self.release(imported);
                return Err(format!(
                    "the frame cannot be read as a framebuffer (status {status:#x})"
                ));
            }
            Ok(imported)
        }
    }

    /// Lets go of an image the renderer no longer has.
    pub fn release(&self, imported: Imported) {
        if self.make_current().is_err() {
            return;
        }
        // SAFETY: the objects `import` made on this context.
        unsafe {
            (self.api.bind_framebuffer)(GL_FRAMEBUFFER, 0);
            (self.api.delete_framebuffers)(1, &imported.framebuffer);
            (self.api.delete_textures)(1, &imported.texture);
            (self.destroy_image)(self.display, imported.image);
        }
    }

    /// Reads `rows` (from the top) of the imported image, `width` pixels from the left.
    /// Waits for the GPU to finish the frame.
    pub fn read(&self, imported: &Imported, width: u32, rows: &[u32]) -> Result<Rows, String> {
        if width == 0 || rows.is_empty() {
            return Err("no rows were asked for".into());
        }
        self.make_current()?;
        let mut rgba = vec![0u8; width as usize * 4 * rows.len()];
        let row_bytes = width as usize * 4;
        // SAFETY: each read fills one row of `rgba`, which has room for every row asked for.
        unsafe {
            (self.api.bind_framebuffer)(GL_FRAMEBUFFER, imported.framebuffer);
            (self.api.pixel_storei)(GL_PACK_ALIGNMENT, 1);
            // Rows next to each other are read in one call.
            let mut index = 0;
            for (top, count) in row_runs(rows) {
                // An imported image's first row in memory is the framebuffer's row 0, so the
                // rows read are the frame's rows from the top, with no flip.
                (self.api.read_pixels)(
                    0,
                    top as i32,
                    width as i32,
                    count as i32,
                    GL_RGBA,
                    GL_UNSIGNED_BYTE,
                    rgba[index * row_bytes..].as_mut_ptr().cast(),
                );
                index += count as usize;
            }
            let error = (self.api.gl_get_error)();
            if error != GL_NO_ERROR {
                return Err(format!("reading the frame failed (error {error:#x})"));
            }
        }
        Ok(Rows {
            width,
            count: rows.len() as u32,
            pixels: rgba_to_xrgb(&rgba),
        })
    }
}

/// Reads `rows` of a GPU image in the linear layout by mapping its dmabuf, `width` pixels
/// from the left. Only the 8-bit RGB formats are read.
pub fn read_linear(
    image: &GpuImage,
    fd: BorrowedFd,
    width: u32,
    rows: &[u32],
) -> Result<Rows, String> {
    if image.modifier != DRM_FORMAT_MOD_LINEAR {
        return Err(format!(
            "the frame is not in the linear layout (modifier {:#x})",
            image.modifier
        ));
    }
    let swap = match image.fourcc {
        code if code == fourcc(b"XR24") || code == fourcc(b"AR24") => false,
        code if code == fourcc(b"XB24") || code == fourcc(b"AB24") => true,
        code => return Err(format!("the frame's format {code:#x} is not read")),
    };
    let width = width.min(image.width);
    let stride = image.stride as usize;
    let last = rows.iter().copied().max().unwrap_or(0);
    if width == 0 || rows.is_empty() || last >= image.height || stride < width as usize * 4 {
        return Err("the rows asked for are not in the frame".into());
    }
    let len = image.offset as usize + stride * (last as usize + 1);
    let fd = fd.as_raw_fd();
    // SAFETY: maps `len` bytes of the dmabuf for reading, reads them between the sync calls
    // that make the GPU's writes visible, and unmaps them.
    unsafe {
        let map = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            fd,
            0,
        );
        if map == libc::MAP_FAILED {
            return Err(format!(
                "the frame cannot be mapped: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut sync: u64 = DMA_BUF_SYNC_READ;
        libc::ioctl(fd, DMA_BUF_IOCTL_SYNC as _, &mut sync as *mut u64);
        let bytes = std::slice::from_raw_parts(map.cast::<u8>(), len);
        let mut pixels = Vec::with_capacity(width as usize * 4 * rows.len());
        for row in rows {
            let start = image.offset as usize + *row as usize * stride;
            for pixel in bytes[start..start + width as usize * 4].chunks_exact(4) {
                if swap {
                    pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0xff]);
                } else {
                    pixels.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 0xff]);
                }
            }
        }
        let mut sync: u64 = DMA_BUF_SYNC_READ | DMA_BUF_SYNC_END;
        libc::ioctl(fd, DMA_BUF_IOCTL_SYNC as _, &mut sync as *mut u64);
        libc::munmap(map, len);
        Ok(Rows {
            width,
            count: rows.len() as u32,
            pixels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr22_a_tiled_gpu_frame_is_read_only_where_the_driver_imports_dmabufs() {
        let list = "EGL_KHR_surfaceless_context EGL_EXT_image_dma_buf_import \
                    EGL_EXT_image_dma_buf_import_modifiers";
        assert!(has_extension(list, "EGL_EXT_image_dma_buf_import"));
        assert!(has_extension(
            list,
            "EGL_EXT_image_dma_buf_import_modifiers"
        ));
        // A name that only starts another is not there.
        assert!(!has_extension(
            "EGL_EXT_image_dma_buf_import_modifiers",
            "EGL_EXT_image_dma_buf_import"
        ));
        assert!(!has_extension("", "EGL_KHR_surfaceless_context"));
    }
}
