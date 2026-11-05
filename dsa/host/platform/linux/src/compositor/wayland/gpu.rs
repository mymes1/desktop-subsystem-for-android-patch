//! Frames that stay on the GPU: the renderer's dmabufs as the desktop's buffers
//! (linux-dmabuf), cropped to the frame in them (viewporter), with the title bar's buttons on a
//! subsurface above them.
//!
//! The renderer keeps each task's images at the largest size the task can have, and a frame
//! fills their top-left region, so a window crops the image to the frame and shows it one to
//! one. Nothing of the frame passes through the CPU. Where the desktop lacks one of the three
//! protocols, every frame is read back instead.

use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use dsa_host_core::compositor::caption::{paint_buttons, CaptionPalette, CaptionState};
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
use crate::compositor::frames::{GpuFrame, GpuImage};

/// One of a display's GPU images, as a buffer of the desktop.
pub struct GpuBuffer {
    buffer: WlBuffer,
    size: (u32, u32),
}

impl Drop for GpuBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

/// The title bar's buttons over a GPU frame: a subsurface with a buffer of its own.
pub struct CaptionLayer {
    surface: WlSurface,
    subsurface: WlSubsurface,
    buffer: Option<ShmBuffer>,
}

impl CaptionLayer {
    fn new(
        compositor: &wayland_client::protocol::wl_compositor::WlCompositor,
        subcompositor: &WlSubcompositor,
        parent: &WlSurface,
        task_id: i32,
        qh: &QueueHandle<State>,
    ) -> CaptionLayer {
        // The buttons' surface carries the task's ID, so a press on them reaches the window.
        let surface = compositor.create_surface(qh, task_id);
        let subsurface = subcompositor.get_subsurface(&surface, parent, qh, ());
        subsurface.set_sync();
        CaptionLayer {
            surface,
            subsurface,
            buffer: None,
        }
    }

    /// Takes the buttons away, as for a full screen window.
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
    /// stands over the buttons, the window's own surface at the origin.
    pub(crate) fn surface_offset(&self, task_id: i32, surface: &WlSurface) -> (f64, f64) {
        let Some(window) = self.windows.get(&task_id) else {
            return (0.0, 0.0);
        };
        match &window.caption_layer {
            Some(layer) if &layer.surface == surface => {
                let scale = window.model.scale.max(1) as i32;
                let left = window.model.caption_layout().buttons_left() / scale;
                (f64::from(left), 0.0)
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
        drop(fd);
        self.gpu_buffers.insert(
            (image.display_id, image.slot),
            GpuBuffer {
                buffer,
                size: (image.width, image.height),
            },
        );
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

        // The title bar's buttons, on their own surface over the frame.
        if window.model.has_caption() && !in_pip {
            let layout = window.model.caption_layout();
            let pixels = paint_buttons(
                &layout,
                window.model.dpi(),
                &CaptionPalette::of(window.tone),
                &CaptionState {
                    maximized: window.model.states.maximized,
                    active: window.model.states.activated,
                    ..window.caption
                },
            );
            let size = (3 * layout.button_width, layout.height.max(1));
            let layer = window.caption_layer.get_or_insert_with(|| {
                CaptionLayer::new(&self.compositor, subcompositor, &window.surface, task_id, &self.qh)
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
                let scale = window.model.scale.max(1) as i32;
                layer
                    .subsurface
                    .set_position(layout.buttons_left() / scale, 0);
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
