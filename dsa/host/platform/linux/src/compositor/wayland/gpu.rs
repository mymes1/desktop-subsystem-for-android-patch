//! Frames that stay on the GPU: the renderer's dmabufs as the desktop's buffers
//! (linux-dmabuf), cropped to the frame in them (viewporter), with the title bar's band on a
//! subsurface above them.
//!
//! The renderer keeps each task's images at the largest size the task can have, and a frame
//! fills their top-left region, so a window crops the image to the frame and shows it one to
//! one. Nothing of the frame passes through the CPU. Where the desktop lacks one of the three
//! protocols, every frame is read back instead.
//!
//! A screenshot of a GPU frame maps its image and reads it, which works for an image in the
//! linear layout; a GPU that keeps its images tiled gives no screenshot, and says so in the
//! log.

use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::Format;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{delegate_noop, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

use super::shm::ShmBuffer;
use super::State;
use crate::compositor::frames::{Frame, GpuFrame, GpuImage};

/// The linear layout's modifier, the only one the CPU can read as rows.
const DRM_FORMAT_MOD_LINEAR: u64 = 0;

/// `DMA_BUF_IOCTL_SYNC` (`_IOW('b', 0, struct dma_buf_sync)`), and its flags.
const DMA_BUF_IOCTL_SYNC: libc::c_ulong = 0x4008_6200;
const DMA_BUF_SYNC_READ: u64 = 1;
const DMA_BUF_SYNC_END: u64 = 4;

/// The four-character codes of the images' formats.
const fn fourcc(code: &[u8; 4]) -> u32 {
    (code[0] as u32) | ((code[1] as u32) << 8) | ((code[2] as u32) << 16) | ((code[3] as u32) << 24)
}

/// One of a display's GPU images, as a buffer of the desktop.
pub struct GpuBuffer {
    buffer: WlBuffer,
    size: (u32, u32),
    image: GpuImage,
    /// The image's dmabuf, kept for a screenshot.
    fd: OwnedFd,
}

impl Drop for GpuBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

impl GpuBuffer {
    /// The top-left `width` x `height` pixels of the image, as a frame. Only an image in the
    /// linear layout, in one of the 8-bit RGB formats, can be read.
    pub fn read(&self, width: u32, height: u32) -> Result<Frame, String> {
        let image = &self.image;
        if image.modifier != DRM_FORMAT_MOD_LINEAR {
            return Err(format!(
                "the GPU keeps the frame in a layout the CPU cannot read (modifier {:#x})",
                image.modifier
            ));
        }
        let swap = match image.fourcc {
            code if code == fourcc(b"XR24") || code == fourcc(b"AR24") => false,
            code if code == fourcc(b"XB24") || code == fourcc(b"AB24") => true,
            code => return Err(format!("the frame's format {code:#x} is not read")),
        };
        let (width, height) = (width.min(image.width), height.min(image.height));
        let stride = image.stride as usize;
        let len = image.offset as usize + stride * height as usize;
        if width == 0 || height == 0 || stride < width as usize * 4 {
            return Err("the frame is empty".into());
        }
        let fd = self.fd.as_raw_fd();
        // SAFETY: maps `len` bytes of the dmabuf for reading, reads them between the sync
        // calls that make the GPU's writes visible, and unmaps them.
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
            let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
            for row in 0..height as usize {
                let start = image.offset as usize + row * stride;
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
            Ok(Frame {
                width,
                height,
                pixels,
            })
        }
    }
}

/// The title bar's band over a GPU frame: a subsurface with a buffer of its own.
pub struct CaptionLayer {
    surface: WlSurface,
    subsurface: WlSubsurface,
    buffer: Option<ShmBuffer>,
    /// Where the layer stands in its window, in logical units.
    position: (i32, i32),
}

impl CaptionLayer {
    fn new(
        compositor: &wayland_client::protocol::wl_compositor::WlCompositor,
        subcompositor: &WlSubcompositor,
        parent: &WlSurface,
        task_id: i32,
        qh: &QueueHandle<State>,
    ) -> CaptionLayer {
        // The band's surface carries the task's ID, so a press on it reaches the window.
        let surface = compositor.create_surface(qh, task_id);
        let subsurface = subcompositor.get_subsurface(&surface, parent, qh, ());
        subsurface.set_sync();
        CaptionLayer {
            surface,
            subsurface,
            buffer: None,
            position: (0, 0),
        }
    }

    /// Takes the band away, as for a full screen window.
    pub fn hide(&mut self) {
        self.surface.attach(None, 0, 0);
        self.surface.commit();
        self.buffer = None;
    }

    pub fn destroy(self) {
        self.subsurface.destroy();
        self.surface.destroy();
    }
}

impl State {
    /// Where `surface` sits in its task's window, in logical units: the title bar's layer
    /// where it was placed, the window's own surface at the origin.
    pub(crate) fn surface_offset(&self, task_id: i32, surface: &WlSurface) -> (f64, f64) {
        let Some(window) = self.windows.get(&task_id) else {
            return (0.0, 0.0);
        };
        match &window.caption_layer {
            Some(layer) if &layer.surface == surface => {
                (f64::from(layer.position.0), f64::from(layer.position.1))
            }
            _ => (0.0, 0.0),
        }
    }

    /// Whether the desktop can show the renderer's dmabufs, cropped, with a title bar above.
    pub fn takes_gpu_frames(&self) -> bool {
        self.dmabuf.is_some() && self.viewporter.is_some() && self.subcompositor.is_some()
    }

    /// The renderer exported a GPU image of a display: it becomes a buffer of the desktop.
    pub fn add_gpu_image(&mut self, image: GpuImage, fd: OwnedFd) {
        if let (Some(x11), Ok(copy)) = (&mut self.x11, fd.try_clone()) {
            x11.import(&image, copy);
        }
        let Some(dmabuf) = &self.dmabuf else {
            return;
        };
        let params = dmabuf.create_params(&self.qh, ());
        params.add(
            fd.as_fd(),
            0,
            image.offset,
            image.stride,
            (image.modifier >> 32) as u32,
            image.modifier as u32,
        );
        let buffer = params.create_immed(
            image.width as i32,
            image.height as i32,
            image.fourcc,
            zwp_linux_buffer_params_v1::Flags::empty(),
            &self.qh,
            Arc::new(AtomicBool::new(false)),
        );
        params.destroy();
        self.gpu_buffers.insert(
            (image.display_id, image.slot),
            GpuBuffer {
                buffer,
                size: (image.width, image.height),
                image,
                fd,
            },
        );
    }

    /// The pixels of a GPU frame of `display_id`, for a screenshot.
    pub(crate) fn read_gpu_frame(&self, display_id: u32, frame: GpuFrame) -> Result<Frame, String> {
        self.gpu_buffers
            .get(&(display_id, frame.slot))
            .ok_or_else(|| "the frame's GPU image is gone".to_string())?
            .read(frame.width, frame.height)
    }

    /// Shows a frame that is in one of the display's GPU images.
    pub fn present_gpu(&mut self, task_id: i32, display_id: u32, frame: GpuFrame) {
        let in_x11 = self
            .windows
            .get(&task_id)
            .is_some_and(|w| matches!(w.pip, Some(super::pip::Pip::X11)));
        if in_x11 {
            if let Some(x11) = &mut self.x11 {
                x11.show_gpu(task_id, display_id, frame);
            }
            if let Some(window) = self.windows.get_mut(&task_id) {
                window.last_gpu = Some((display_id, frame));
                window.model.frame_shown((frame.width, frame.height));
            }
            return;
        }
        let Some(gpu) = self.gpu_buffers.get(&(display_id, frame.slot)) else {
            return;
        };
        let width = frame.width.min(gpu.size.0).max(1);
        let height = frame.height.min(gpu.size.1).max(1);
        let (Some(viewporter), Some(subcompositor)) = (&self.viewporter, &self.subcompositor)
        else {
            return;
        };
        let Some(window) = self.windows.get_mut(&task_id) else {
            return;
        };
        if !window.is_configured() {
            return;
        }
        window.last_gpu = Some((display_id, frame));
        window.model.frame_shown((width, height));
        let (logical_width, logical_height) = window.model.to_logical((width, height));
        let in_pip = window.pip.is_some();
        let target = window.target().clone();
        let viewport = match window.pip.as_mut().and_then(|pip| pip.viewport()) {
            Some(viewport) => viewport,
            None => &mut window.viewport,
        };
        let viewport =
            viewport.get_or_insert_with(|| viewporter.get_viewport(&target, &self.qh, ()));
        viewport.set_source(0.0, 0.0, f64::from(width), f64::from(height));
        viewport.set_destination(logical_width, logical_height);

        // The title bar's band, on its own surface over the frame. The CPU cannot read the
        // frame, so the band is never filled with the app's top color here.
        if window.model.has_caption() && !in_pip {
            let scale = window.model.scale.max(1) as i32;
            let parent = window.surface.clone();
            let (layout, pixels) = window.band_pixels(self.text.as_ref(), None);
            let pixels = pixels.to_vec();
            let size = (layout.width.max(1), layout.height.max(1));
            let layer = window.caption_layer.get_or_insert_with(|| {
                CaptionLayer::new(&self.compositor, subcompositor, &parent, task_id, &self.qh)
            });
            let reuse = layer
                .buffer
                .as_ref()
                .is_some_and(|buffer| buffer.size == size && !buffer.is_busy());
            if !reuse {
                layer.buffer =
                    ShmBuffer::with_format(&self.shm, &self.qh, size, Format::Argb8888).ok();
            }
            if let Some(buffer) = &mut layer.buffer {
                buffer.write(&pixels);
                buffer.set_busy();
                layer.position = (0, 0);
                layer.subsurface.set_position(0, 0);
                layer.surface.set_buffer_scale(scale);
                layer.surface.attach(Some(&buffer.buffer), 0, 0);
                layer
                    .surface
                    .damage_buffer(0, 0, size.0 as i32, size.1 as i32);
                layer.surface.commit();
            }
        } else if let Some(layer) = &mut window.caption_layer {
            layer.hide();
        }

        target.set_buffer_scale(1);
        target.attach(Some(&gpu.buffer), 0, 0);
        target.damage_buffer(0, 0, gpu.size.0 as i32, gpu.size.1 as i32);
        if !in_pip {
            window
                .xdg_surface()
                .set_window_geometry(0, 0, logical_width, logical_height);
        }
        target.commit();
    }
}

delegate_noop!(State: ignore ZwpLinuxDmabufV1);
delegate_noop!(State: ignore ZwpLinuxBufferParamsV1);
delegate_noop!(State: ignore WpViewporter);
delegate_noop!(State: ignore WpViewport);
delegate_noop!(State: ignore WlSubcompositor);
delegate_noop!(State: ignore WlSubsurface);
