//! Picture-in-picture windows: small, above every other window, in a corner of the screen.
//!
//! A Wayland toplevel can neither place itself nor stay above the others, so a task in
//! picture-in-picture leaves its toplevel (which is unmapped) for a surface that can:
//!
//! 1. a layer surface (wlr-layer-shell, which KDE Plasma and the wlroots desktops have) on the
//!    overlay layer, anchored to the bottom right corner;
//! 2. else an X11 window through XWayland (GNOME), kept above with `_NET_WM_STATE_ABOVE` and
//!    placed in the corner, which the X11 window managers honor;
//! 3. else the toplevel itself, at the small size, wherever the desktop puts it.
//!
//! Leaving picture-in-picture maps the toplevel again.

use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

use super::State;
use dsa_host_core::compositor::window_hosts::FromHost;

/// The margin from the screen's corner, in logical units.
pub const MARGIN: i32 = 16;

/// Where a task in picture-in-picture is shown instead of its toplevel.
pub enum Pip {
    /// A layer surface on the overlay layer.
    Layer {
        surface: WlSurface,
        layer: ZwlrLayerSurfaceV1,
        viewport: Option<WpViewport>,
        configured: bool,
    },
    /// An X11 window, through XWayland.
    X11,
}

impl Pip {
    /// The Wayland surface the frames go to, when there is one.
    pub fn surface(&self) -> Option<&WlSurface> {
        match self {
            Pip::Layer { surface, .. } => Some(surface),
            Pip::X11 => None,
        }
    }

    pub fn is_configured(&self) -> bool {
        match self {
            Pip::Layer { configured, .. } => *configured,
            Pip::X11 => true,
        }
    }

    pub fn viewport(&mut self) -> Option<&mut Option<WpViewport>> {
        match self {
            Pip::Layer { viewport, .. } => Some(viewport),
            Pip::X11 => None,
        }
    }

    fn destroy(self) {
        if let Pip::Layer {
            surface,
            layer,
            viewport,
            ..
        } = self
        {
            if let Some(viewport) = viewport {
                viewport.destroy();
            }
            layer.destroy();
            surface.destroy();
        }
    }
}

impl State {
    /// The task's window goes into picture-in-picture at `size` pixels. Returns whether it
    /// left its toplevel for a surface that stays above the others.
    pub(crate) fn enter_pip(&mut self, task_id: i32, size: (u32, u32)) -> bool {
        if self.windows.get(&task_id).is_some_and(|w| w.pip.is_some()) {
            return true;
        }
        let Some(window) = self.windows.get(&task_id) else {
            return false;
        };
        let (width, height) = window.model.to_logical(size);
        let pip = if let Some(shell) = &self.layer_shell {
            let surface = self.compositor.create_surface(&self.qh, task_id);
            let layer = shell.get_layer_surface(
                &surface,
                None,
                Layer::Overlay,
                "dsa-picture-in-picture".into(),
                &self.qh,
                task_id,
            );
            layer.set_size(width.max(1) as u32, height.max(1) as u32);
            layer.set_anchor(Anchor::Bottom | Anchor::Right);
            layer.set_margin(0, MARGIN, MARGIN, 0);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            surface.commit();
            Pip::Layer {
                surface,
                layer,
                viewport: None,
                configured: false,
            }
        } else if let Some(x11) = &mut self.x11 {
            let title = window.model.title.clone();
            if !x11.open(task_id, size, &title) {
                return false;
            }
            Pip::X11
        } else {
            return false;
        };
        if let Some(window) = self.windows.get_mut(&task_id) {
            window.unmap();
            window.pip = Some(pip);
        }
        true
    }

    /// The task's window leaves picture-in-picture: its toplevel comes back.
    pub(crate) fn leave_pip(&mut self, task_id: i32) {
        let Some(window) = self.windows.get_mut(&task_id) else {
            return;
        };
        let Some(pip) = window.pip.take() else {
            return;
        };
        if matches!(pip, Pip::X11) {
            if let Some(x11) = &mut self.x11 {
                x11.close(task_id);
            }
        }
        pip.destroy();
        window.remap();
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, i32> for State {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        task_id: &i32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let task_id = *task_id;
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                layer.ack_configure(serial);
                if let Some(Some(Pip::Layer { configured, .. })) =
                    state.windows.get_mut(&task_id).map(|w| w.pip.as_mut())
                {
                    *configured = true;
                }
                state.redraw(task_id);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.report(FromHost::Closed { task_id });
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(State: ignore ZwlrLayerShellV1);
