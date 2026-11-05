//! Presenting a task's frames through a `CAMetalLayer`.
//!
//! A frame arrives as BGRA pixels (what the gfxstream renderer reads back from the task's
//! ColorBuffer). It is written into a staging texture, copied into the layer's next drawable
//! by a blit, and presented. The drawable takes the size of the frame's content
//! ([`super::sizing::place`]), and the layer pins it to the top left without scaling.

use std::ffi::c_void;

use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;

use super::sizing::place;
use crate::objc::{class, ns_string, CGSize};

extern "C" {
    fn MTLCreateSystemDefaultDevice() -> *mut AnyObject;
}

/// `MTLPixelFormatBGRA8Unorm`.
const BGRA8_UNORM: usize = 80;
/// `MTLStorageModeManaged`: the CPU writes, the GPU reads; on every Mac.
const STORAGE_MANAGED: usize = 1;
/// `MTLTextureUsageShaderRead`.
const USAGE_SHADER_READ: usize = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct MTLOrigin {
    x: usize,
    y: usize,
    z: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct MTLSize {
    width: usize,
    height: usize,
    depth: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct MTLRegion {
    origin: MTLOrigin,
    size: MTLSize,
}

unsafe impl Encode for MTLOrigin {
    const ENCODING: Encoding = Encoding::Struct(
        "MTLOrigin",
        &[usize::ENCODING, usize::ENCODING, usize::ENCODING],
    );
}
unsafe impl RefEncode for MTLOrigin {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}
unsafe impl Encode for MTLSize {
    const ENCODING: Encoding = Encoding::Struct(
        "MTLSize",
        &[usize::ENCODING, usize::ENCODING, usize::ENCODING],
    );
}
unsafe impl RefEncode for MTLSize {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}
unsafe impl Encode for MTLRegion {
    const ENCODING: Encoding =
        Encoding::Struct("MTLRegion", &[MTLOrigin::ENCODING, MTLSize::ENCODING]);
}
unsafe impl RefEncode for MTLRegion {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

/// One frame of a task: BGRA pixels, `stride` bytes a row, of which `content` pixels from the
/// top left are the task.
#[derive(Debug, Clone, Copy)]
pub struct FrameRef<'a> {
    pub pixels: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub content: (u32, u32),
}

/// The layer of one task window and what it takes to fill it.
pub struct Presenter {
    device: Retained<AnyObject>,
    queue: Retained<AnyObject>,
    layer: Retained<AnyObject>,
    staging: Option<(Retained<AnyObject>, (u32, u32))>,
    drawable: (u32, u32),
}

impl Presenter {
    /// A `CAMetalLayer` on the system's default GPU, set to show drawables one to one from
    /// the top left at the display's `scale`.
    pub fn new(scale: f64) -> Result<Presenter, String> {
        unsafe {
            let device = Retained::from_raw(MTLCreateSystemDefaultDevice())
                .ok_or("this Mac has no Metal device")?;
            let queue: Option<Retained<AnyObject>> = msg_send![&*device, newCommandQueue];
            let queue = queue.ok_or("the Metal device gave no command queue")?;
            let layer_class = class(c"CAMetalLayer")?;
            let layer: Option<Retained<AnyObject>> = msg_send![layer_class, layer];
            let layer = layer.ok_or("no CAMetalLayer could be made")?;
            let _: () = msg_send![&*layer, setDevice: &*device];
            let _: () = msg_send![&*layer, setPixelFormat: BGRA8_UNORM];
            // The blit writes into the drawable, which a framebuffer-only texture forbids.
            let _: () = msg_send![&*layer, setFramebufferOnly: false];
            let _: () = msg_send![&*layer, setContentsScale: scale];
            // No scaling: a drawable that does not match the window is never stretched.
            // AppKit's layers are not flipped, so the visual top is the layer's top only once
            // the geometry is flipped.
            let _: () = msg_send![&*layer, setGeometryFlipped: true];
            let gravity = ns_string("topLeft");
            let _: () = msg_send![&*layer, setContentsGravity: &*gravity];
            let _: () = msg_send![&*layer, setOpaque: true];
            let _: () = msg_send![&*layer, setAllowsNextDrawableTimeout: false];
            Ok(Presenter {
                device,
                queue,
                layer,
                staging: None,
                drawable: (0, 0),
            })
        }
    }

    /// The layer, for the window's content view to host.
    pub fn layer(&self) -> &AnyObject {
        &self.layer
    }

    /// The display's backing scale changed (the window moved to another display).
    pub fn set_scale(&self, scale: f64) {
        unsafe {
            let _: () = msg_send![&*self.layer, setContentsScale: scale];
        }
    }

    fn staging(&mut self, size: (u32, u32)) -> Result<&AnyObject, String> {
        let fits = matches!(&self.staging, Some((_, have)) if *have == size);
        if !fits {
            unsafe {
                let descriptor_class = class(c"MTLTextureDescriptor")?;
                let descriptor: Option<Retained<AnyObject>> = msg_send![
                    descriptor_class,
                    texture2DDescriptorWithPixelFormat: BGRA8_UNORM,
                    width: size.0 as usize,
                    height: size.1 as usize,
                    mipmapped: false
                ];
                let descriptor = descriptor.ok_or("no texture descriptor")?;
                let _: () = msg_send![&*descriptor, setStorageMode: STORAGE_MANAGED];
                let _: () = msg_send![&*descriptor, setUsage: USAGE_SHADER_READ];
                let texture: Option<Retained<AnyObject>> =
                    msg_send![&*self.device, newTextureWithDescriptor: &*descriptor];
                let texture = texture.ok_or_else(|| {
                    format!("the GPU could not make a {}x{} texture", size.0, size.1)
                })?;
                self.staging = Some((texture, size));
            }
        }
        Ok(&self.staging.as_ref().expect("made above").0)
    }

    /// Shows `frame`. A frame that is not whole (its pixels are fewer than its size says) is
    /// refused rather than read past its end.
    pub fn present(&mut self, frame: FrameRef<'_>) -> Result<(), String> {
        let needed = (frame.stride as usize).checked_mul(frame.height as usize);
        if frame.stride < frame.width.saturating_mul(4) || needed.is_none_or(|n| n > frame.pixels.len())
        {
            return Err(format!(
                "a {}x{} frame with {} bytes a row does not fit its {} bytes",
                frame.width,
                frame.height,
                frame.stride,
                frame.pixels.len()
            ));
        }
        let Some(placement) = place(frame.content, (frame.width, frame.height)) else {
            return Ok(());
        };
        let (width, height) = placement.copy;
        unsafe {
            if self.drawable != placement.drawable {
                let size = CGSize {
                    width: f64::from(placement.drawable.0),
                    height: f64::from(placement.drawable.1),
                };
                let _: () = msg_send![&*self.layer, setDrawableSize: size];
                self.drawable = placement.drawable;
            }
            let region = MTLRegion {
                origin: MTLOrigin::default(),
                size: MTLSize {
                    width: width as usize,
                    height: height as usize,
                    depth: 1,
                },
            };
            let staging: *const AnyObject = self.staging((width, height))?;
            let _: () = msg_send![
                staging,
                replaceRegion: region,
                mipmapLevel: 0usize,
                withBytes: frame.pixels.as_ptr() as *const c_void,
                bytesPerRow: frame.stride as usize
            ];
            let drawable: Option<Retained<AnyObject>> = msg_send![&*self.layer, nextDrawable];
            let Some(drawable) = drawable else {
                // The window is hidden or off screen: nothing to show into.
                return Ok(());
            };
            let target: *mut AnyObject = msg_send![&*drawable, texture];
            let buffer: Option<Retained<AnyObject>> = msg_send![&*self.queue, commandBuffer];
            let buffer = buffer.ok_or("the GPU gave no command buffer")?;
            let blit: Option<Retained<AnyObject>> = msg_send![&*buffer, blitCommandEncoder];
            let blit = blit.ok_or("the GPU gave no blit encoder")?;
            let _: () = msg_send![
                &*blit,
                copyFromTexture: staging,
                sourceSlice: 0usize,
                sourceLevel: 0usize,
                sourceOrigin: MTLOrigin::default(),
                sourceSize: region.size,
                toTexture: target,
                destinationSlice: 0usize,
                destinationLevel: 0usize,
                destinationOrigin: MTLOrigin::default()
            ];
            let _: () = msg_send![&*blit, endEncoding];
            let _: () = msg_send![&*buffer, presentDrawable: &*drawable];
            let _: () = msg_send![&*buffer, commit];
        }
        Ok(())
    }
}
