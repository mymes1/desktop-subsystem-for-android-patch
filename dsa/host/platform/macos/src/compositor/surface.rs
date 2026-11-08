//! A task's frames shared between the window manager and the app's window host: `IOSurface`s
//! the window manager writes (or has the renderer write on the GPU) and the window host shows.
//!
//! A surface reaches the window host as a Mach port ([`super::surface_port`]). Only where
//! that cannot be done (the window manager was started by hand, outside its launchd agent)
//! is the surface made global, so the window host finds it by its ID, which macOS keeps for
//! compatibility. The window manager never writes a surface the window host still reads, so
//! nothing shows a half-written frame.

use std::ffi::c_void;

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;

use crate::objc::{class, ns_string};

#[link(name = "IOSurface", kind = "framework")]
extern "C" {
    fn IOSurfaceCreate(properties: *const AnyObject) -> *mut c_void;
    fn IOSurfaceLookup(id: u32) -> *mut c_void;
    fn IOSurfaceCreateMachPort(surface: *mut c_void) -> u32;
    fn IOSurfaceLookupFromMachPort(port: u32) -> *mut c_void;
    fn IOSurfaceGetID(surface: *mut c_void) -> u32;
    fn IOSurfaceLock(surface: *mut c_void, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceUnlock(surface: *mut c_void, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceGetBaseAddress(surface: *mut c_void) -> *mut c_void;
    fn IOSurfaceGetBytesPerRow(surface: *mut c_void) -> usize;
    fn IOSurfaceGetWidth(surface: *mut c_void) -> usize;
    fn IOSurfaceGetHeight(surface: *mut c_void) -> usize;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(object: *const c_void);
}

/// `kIOSurfaceLockReadOnly`.
const LOCK_READ_ONLY: u32 = 1;
/// `'BGRA'`.
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");

/// One task's shared frame surface.
pub struct Surface {
    surface: *mut c_void,
}

// An IOSurface may be used from any thread; locking orders the access.
unsafe impl Send for Surface {}

impl Surface {
    /// A new BGRA surface of `width` by `height`. A `global` one can be found by its ID from
    /// any process; any other is handed over as a Mach port.
    pub fn create(width: u32, height: u32, global: bool) -> Result<Surface, String> {
        unsafe {
            let dict_class = class(c"NSMutableDictionary")?;
            let number_class = class(c"NSNumber")?;
            let properties: Option<Retained<AnyObject>> = msg_send![dict_class, new];
            let properties = properties.ok_or("no surface properties could be made")?;
            let set = |key: &str, value: *mut AnyObject| {
                let _: () = msg_send![&*properties, setObject: value, forKey: &*ns_string(key)];
            };
            let number = |value: u32| -> *mut AnyObject {
                msg_send![number_class, numberWithUnsignedInt: value]
            };
            set("IOSurfaceWidth", number(width));
            set("IOSurfaceHeight", number(height));
            set("IOSurfaceBytesPerElement", number(4));
            set("IOSurfacePixelFormat", number(BGRA));
            if global {
                let yes: *mut AnyObject = msg_send![number_class, numberWithBool: true];
                set("IOSurfaceIsGlobal", yes);
            }
            let surface = IOSurfaceCreate(Retained::as_ptr(&properties));
            if surface.is_null() {
                return Err(format!("no {width}x{height} frame surface could be made"));
            }
            Ok(Surface { surface })
        }
    }

    /// The surface with `id`, which another process made.
    pub fn lookup(id: u32) -> Result<Surface, String> {
        let surface = unsafe { IOSurfaceLookup(id) };
        if surface.is_null() {
            Err(format!("the frame surface {id} is gone"))
        } else {
            Ok(Surface { surface })
        }
    }

    /// The surface a Mach port another process sent names. The port is left to the caller.
    pub fn from_mach_port(port: u32) -> Result<Surface, String> {
        let surface = unsafe { IOSurfaceLookupFromMachPort(port) };
        if surface.is_null() {
            Err("the frame surface sent by the window manager is gone".into())
        } else {
            Ok(Surface { surface })
        }
    }

    /// A send right to the surface, for another process. The caller owns the right.
    pub fn mach_port(&self) -> u32 {
        unsafe { IOSurfaceCreateMachPort(self.surface) }
    }

    /// A send right to the surface `raw` names (an `IOSurfaceRef` the caller holds), for
    /// another process. The caller owns the right.
    ///
    /// # Safety
    ///
    /// `raw` must be a live `IOSurfaceRef` for the whole call.
    pub unsafe fn mach_port_of(raw: *mut c_void) -> u32 {
        // SAFETY: a live IOSurfaceRef, as the caller promises.
        unsafe { IOSurfaceCreateMachPort(raw) }
    }

    /// The ID of the surface `raw` names, which another process finds a global surface by.
    ///
    /// # Safety
    ///
    /// `raw` must be a live `IOSurfaceRef` for the whole call.
    pub unsafe fn id_of(raw: *mut c_void) -> u32 {
        // SAFETY: a live IOSurfaceRef, as the caller promises.
        unsafe { IOSurfaceGetID(raw) }
    }

    /// The `IOSurfaceRef`, valid while this value lives.
    pub fn as_raw(&self) -> *mut c_void {
        self.surface
    }

    pub fn id(&self) -> u32 {
        unsafe { IOSurfaceGetID(self.surface) }
    }

    pub fn size(&self) -> (u32, u32) {
        unsafe {
            (
                IOSurfaceGetWidth(self.surface) as u32,
                IOSurfaceGetHeight(self.surface) as u32,
            )
        }
    }

    /// Writes `pixels` (BGRA, `width * 4` bytes a row) into the top-left `width` by `height`
    /// region, cut to the surface's size.
    pub fn write(&self, width: u32, height: u32, pixels: &[u8]) -> Result<(), String> {
        let (surface_w, surface_h) = self.size();
        let (w, h) = (
            width.min(surface_w) as usize,
            height.min(surface_h) as usize,
        );
        let row = width as usize * 4;
        if pixels.len() < row * height as usize {
            return Err("the frame is shorter than its size".into());
        }
        unsafe {
            if IOSurfaceLock(self.surface, 0, std::ptr::null_mut()) != 0 {
                return Err("the frame surface could not be locked".into());
            }
            let base = IOSurfaceGetBaseAddress(self.surface) as *mut u8;
            let stride = IOSurfaceGetBytesPerRow(self.surface);
            for y in 0..h {
                let from = pixels.as_ptr().add(y * row);
                std::ptr::copy_nonoverlapping(from, base.add(y * stride), w * 4);
            }
            IOSurfaceUnlock(self.surface, 0, std::ptr::null_mut());
        }
        Ok(())
    }

    /// Writes `rgba` (RGBA, `width * 4` bytes a row, as the renderer reads frames back) into
    /// the top-left `width` by `height` region as BGRA, cut to the surface's size.
    pub fn write_rgba(&self, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
        let (surface_w, surface_h) = self.size();
        let (w, h) = (
            width.min(surface_w) as usize,
            height.min(surface_h) as usize,
        );
        let row = width as usize * 4;
        if rgba.len() < row * height as usize {
            return Err("the frame is shorter than its size".into());
        }
        unsafe {
            if IOSurfaceLock(self.surface, 0, std::ptr::null_mut()) != 0 {
                return Err("the frame surface could not be locked".into());
            }
            let base = IOSurfaceGetBaseAddress(self.surface) as *mut u8;
            let stride = IOSurfaceGetBytesPerRow(self.surface);
            for y in 0..h {
                let to = std::slice::from_raw_parts_mut(base.add(y * stride), w * 4);
                let from = &rgba[y * row..y * row + w * 4];
                super::frames::swap_red_blue(from, to);
            }
            IOSurfaceUnlock(self.surface, 0, std::ptr::null_mut());
        }
        Ok(())
    }

    /// Runs `read` on the surface's pixels and its bytes a row, locked for reading.
    pub fn read<R>(&self, read: impl FnOnce(&[u8], usize) -> R) -> Result<R, String> {
        unsafe {
            if IOSurfaceLock(self.surface, LOCK_READ_ONLY, std::ptr::null_mut()) != 0 {
                return Err("the frame surface could not be locked".into());
            }
            let base = IOSurfaceGetBaseAddress(self.surface) as *const u8;
            let stride = IOSurfaceGetBytesPerRow(self.surface);
            let len = stride * IOSurfaceGetHeight(self.surface);
            let result = read(std::slice::from_raw_parts(base, len), stride);
            IOSurfaceUnlock(self.surface, LOCK_READ_ONLY, std::ptr::null_mut());
            Ok(result)
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe { CFRelease(self.surface) }
    }
}
