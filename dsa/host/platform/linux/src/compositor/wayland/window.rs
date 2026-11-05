//! A task's toplevel: its surface, its buffers, its title bar, and the window rules' changes
//! applied to it.

use std::time::Instant;

use dsa_host_core::compositor::caption::{
    paint_buttons, CaptionButton, CaptionPalette, CaptionState, CaptionTone,
};
use dsa_host_core::compositor::task_windows::WindowChange;
use dsa_host_core::compositor::window_hosts::FromHost;
use dsa_host_core::compositor::window_mode::{picture_in_picture_place, WindowMode};
use dsa_host_core::ime::ImeCursor;
use dsa_host_core::input::PointerMapper;
use dsa_host_core::keys::KeyMapper;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::{
    self, ZxdgToplevelDecorationV1,
};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};

use super::shm::ShmBuffer;
use super::State;
use crate::compositor::frames::Frame;
use crate::compositor::toplevel::{Reported, States, Toplevel};
use crate::input::text_input::TextInputV3;
use crate::shell::desktop_entry::app_id;

/// How many buffers a window cycles through, so it can draw while the desktop reads one.
const BUFFERS: usize = 3;

/// The color a window shows before its task's first frame.
const BLANK: u32 = 0x00_20_20_20;

/// The margin of a picture-in-picture window from the work area's corner, at 100%.
const PIP_MARGIN: i32 = 16;

/// One task window.
pub struct Window {
    pub model: Toplevel,
    pub surface: WlSurface,
    xdg_surface: XdgSurface,
    pub toplevel: XdgToplevel,
    decoration: Option<ZxdgToplevelDecorationV1>,
    buffers: Vec<ShmBuffer>,
    /// Whether the first configure has been acknowledged; nothing is drawn before.
    configured: bool,
    /// The frame shown last (without the title bar), drawn again when the title bar changes.
    last: Option<Frame>,
    /// A frame that waits for a free buffer.
    waiting: Option<Frame>,
    pub caption: CaptionState,
    pub tone: CaptionTone,
    pub pointer: PointerMapper,
    pub keys: KeyMapper,
    pub ime: TextInputV3,
    pub ime_cursor: Option<ImeCursor>,
    /// When the title bar was last clicked, for a double click.
    pub last_caption_click: Option<Instant>,
    /// Whether the window was reported to the window rules as opened (with its DPI).
    pub dpi_told: bool,
    /// Crops a GPU image to the frame in it, when frames come as dmabufs.
    pub(crate) viewport: Option<wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport>,
    /// The title bar's buttons over a GPU frame, which the CPU cannot draw into.
    pub(crate) caption_layer: Option<super::gpu::CaptionLayer>,
    /// The GPU frame shown last, drawn again when the title bar changes.
    pub(crate) last_gpu: Option<(u32, crate::compositor::frames::GpuFrame)>,
}

impl State {
    /// Opens the task's window.
    pub(crate) fn open_window(
        &mut self,
        task_id: i32,
        package: &str,
        title: &str,
        size: (u32, u32),
    ) {
        if self.windows.contains_key(&task_id) {
            return;
        }
        let surface = self.compositor.create_surface(&self.qh, task_id);
        let xdg_surface = self.wm_base.get_xdg_surface(&surface, &self.qh, task_id);
        let toplevel = xdg_surface.get_toplevel(&self.qh, task_id);
        toplevel.set_app_id(app_id(package));
        toplevel.set_title(title.to_string());
        let decoration = self.decorations.as_ref().map(|manager| {
            let decoration = manager.get_toplevel_decoration(&toplevel, &self.qh, ());
            decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ClientSide);
            decoration
        });
        let model = Toplevel::new(task_id, package, title, size);
        let (width, height) = model.to_logical(size);
        xdg_surface.set_window_geometry(0, 0, width, height);
        // The first commit, with no buffer, asks the desktop for the first configure.
        surface.commit();
        self.windows.insert(
            task_id,
            Window {
                model,
                surface,
                xdg_surface,
                toplevel,
                decoration,
                buffers: Vec::new(),
                configured: false,
                last: None,
                waiting: None,
                caption: CaptionState::default(),
                tone: CaptionTone::default(),
                pointer: PointerMapper::new(task_id),
                keys: KeyMapper::new(task_id),
                ime: TextInputV3::new(task_id),
                ime_cursor: None,
                last_caption_click: None,
                dpi_told: false,
                viewport: None,
                caption_layer: None,
                last_gpu: None,
            },
        );
    }

    /// Applies one of the window rules' changes.
    pub fn change(&mut self, change: &WindowChange) {
        match change {
            WindowChange::Open {
                task_id,
                package_name,
                title,
                width,
                height,
                ..
            } => self.open_window(*task_id, package_name, title, (*width, *height)),
            WindowChange::Retitle { task_id, title } => {
                if let Some(window) = self.windows.get_mut(task_id) {
                    window.model.title = title.clone();
                    window.toplevel.set_title(title.clone());
                }
            }
            WindowChange::Resize {
                task_id,
                width,
                height,
            } => {
                // The window takes the size with the task's next frame; until then a window
                // with no frame yet shows a blank of the size.
                if let Some(window) = self.windows.get_mut(task_id) {
                    if window.last.is_none() {
                        window.model.frame_shown((*width, *height));
                    }
                }
            }
            WindowChange::Close { task_id } => self.close_window(*task_id),
            WindowChange::TaskSize {
                task_id,
                width,
                height,
                max,
                min,
            } => {
                if let Some(window) = self.windows.get_mut(task_id) {
                    window.model.task = (*width, *height);
                    window.model.max = *max;
                    window.model.min = *min;
                    window.limits();
                }
            }
            WindowChange::ImeCursor { task_id, cursor } => {
                if let Some(window) = self.windows.get_mut(task_id) {
                    window.ime_cursor = Some(*cursor);
                }
                if self.seat_state.keyboard_focus == Some(*task_id) {
                    self.place_input_method(*task_id);
                }
            }
            WindowChange::CaptionTone { task_id, tone } => {
                if let Some(window) = self.windows.get_mut(task_id) {
                    window.tone = *tone;
                }
                self.redraw(*task_id);
            }
            WindowChange::Mode {
                task_id,
                mode,
                resizable,
            } => {
                let work = self.work_area();
                let Some(window) = self.windows.get_mut(task_id) else {
                    return;
                };
                window.model.resizable = *resizable;
                let shown = window.model.mode.app_asks(*mode);
                let small = window.apply_mode(shown, work);
                window.limits();
                self.after_mode(*task_id);
                self.picture_in_picture(*task_id, small);
            }
        }
    }

    /// The user pressed F11 in the task's window.
    pub(crate) fn toggle_fullscreen(&mut self, task_id: i32) {
        let work = self.work_area();
        let small = match self.windows.get_mut(&task_id) {
            Some(window) => {
                let shown = window.model.mode.toggle_fullscreen();
                window.apply_mode(shown, work)
            }
            None => return,
        };
        self.after_mode(task_id);
        self.picture_in_picture(task_id, small);
    }

    /// Asks the task for the small size of a picture-in-picture window.
    fn picture_in_picture(&mut self, task_id: i32, size: Option<(u32, u32)>) {
        if let Some((width, height)) = size {
            self.report(FromHost::Resized {
                task_id,
                width,
                height,
            });
        }
    }

    /// Tells the window rules the title bar's height after the mode changed, and draws.
    fn after_mode(&mut self, task_id: i32) {
        let caption = self
            .windows
            .get_mut(&task_id)
            .and_then(|window| window.model.caption_change());
        if let Some(height) = caption {
            self.report(FromHost::Caption { task_id, height });
        }
        self.redraw(task_id);
    }

    pub(crate) fn close_window(&mut self, task_id: i32) {
        if let Some(window) = self.windows.remove(&task_id) {
            if let Some(viewport) = &window.viewport {
                viewport.destroy();
            }
            if let Some(layer) = window.caption_layer {
                layer.destroy();
            }
            if let Some(decoration) = &window.decoration {
                decoration.destroy();
            }
            window.toplevel.destroy();
            window.xdg_surface.destroy();
            window.surface.destroy();
        }
        if self.seat_state.keyboard_focus == Some(task_id) {
            self.seat_state.keyboard_focus = None;
        }
        if self.seat_state.pointer_focus == Some(task_id) {
            self.seat_state.pointer_focus = None;
        }
    }

    /// Shows a frame of the task in its window.
    pub fn present(&mut self, task_id: i32, frame: Frame) {
        let shm = self.shm.clone();
        let qh = self.qh.clone();
        if let Some(window) = self.windows.get_mut(&task_id) {
            window.waiting = Some(frame);
            window.draw(&shm, &qh);
        }
    }

    /// Draws the window again with its last frame: its title bar changed.
    pub(crate) fn redraw(&mut self, task_id: i32) {
        let shm = self.shm.clone();
        let qh = self.qh.clone();
        if let Some((display_id, frame)) = self.windows.get(&task_id).and_then(|w| w.last_gpu)
        {
            self.present_gpu(task_id, display_id, frame);
            return;
        }
        if let Some(window) = self.windows.get_mut(&task_id) {
            if window.waiting.is_none() {
                window.waiting = window.last.clone();
            }
            window.draw(&shm, &qh);
        }
    }

    /// Draws every window whose frame waits for a free buffer.
    pub fn draw_waiting(&mut self) {
        let shm = self.shm.clone();
        let qh = self.qh.clone();
        for window in self.windows.values_mut() {
            if window.waiting.is_some() {
                window.draw(&shm, &qh);
            }
        }
    }
}

impl Window {
    /// Sets the smallest and largest sizes the desktop may give the window, in logical
    /// units: the guest's limits for the task, none for a window the user may not resize
    /// but its own size.
    fn limits(&self) {
        let model = &self.model;
        if !model.resizable && model.mode.effective() == WindowMode::Normal {
            let (width, height) = model.to_logical(model.shown);
            self.toplevel.set_min_size(width, height);
            self.toplevel.set_max_size(width, height);
            return;
        }
        let (min_width, min_height) = model.min.map(|min| model.to_logical(min)).unwrap_or((0, 0));
        let (max_width, max_height) = model.max.map(|max| model.to_logical(max)).unwrap_or((0, 0));
        self.toplevel.set_min_size(min_width, min_height);
        self.toplevel.set_max_size(max_width, max_height);
    }

    /// Shows the window in `mode`. Returns the size a picture-in-picture window asks the
    /// task for.
    fn apply_mode(&mut self, mode: WindowMode, work: (i32, i32)) -> Option<(u32, u32)> {
        match mode {
            WindowMode::Fullscreen => {
                self.toplevel.set_fullscreen(None);
                None
            }
            WindowMode::Normal => {
                self.toplevel.unset_fullscreen();
                None
            }
            WindowMode::PictureInPicture => {
                // Wayland lets no window place itself or stay above the others; the window
                // takes the small size, and the desktop places it.
                self.toplevel.unset_fullscreen();
                let scale = self.model.scale.max(1) as i32;
                let (_, _, width, height) = picture_in_picture_place(
                    self.model.task,
                    (0, 0, work.0 * scale, work.1 * scale),
                    PIP_MARGIN * scale,
                );
                Some((width, height))
            }
        }
    }

    /// Draws the waiting frame, with the title bar over it, into a free buffer.
    /// Whether the first configure has been acknowledged, so the window may draw.
    pub(crate) fn is_configured(&self) -> bool {
        self.configured
    }

    pub(crate) fn xdg_surface(&self) -> &XdgSurface {
        &self.xdg_surface
    }

    fn draw(&mut self, shm: &wayland_client::protocol::wl_shm::WlShm, qh: &QueueHandle<State>) {
        if !self.configured {
            return;
        }
        if self.last_gpu.is_some() && self.waiting.is_none() {
            return;
        }
        self.last_gpu = None;
        if let Some(layer) = &mut self.caption_layer {
            layer.hide();
        }
        if let Some(viewport) = &self.viewport {
            viewport.set_source(-1.0, -1.0, -1.0, -1.0);
            viewport.set_destination(-1, -1);
        }
        let frame = match self.waiting.take() {
            Some(frame) => frame,
            None if self.last.is_none() => {
                let (width, height) = self.model.shown;
                Frame::solid(width, height, BLANK)
            }
            None => return,
        };
        let size = (frame.width, frame.height);
        self.buffers.retain(|buffer| buffer.size == size || buffer.is_busy());
        let free = self
            .buffers
            .iter()
            .position(|buffer| buffer.size == size && !buffer.is_busy());
        let at = match free {
            Some(at) => at,
            None if self.buffers.len() < BUFFERS => match ShmBuffer::new(shm, qh, size) {
                Ok(buffer) => {
                    self.buffers.push(buffer);
                    self.buffers.len() - 1
                }
                Err(_) => return,
            },
            None => {
                // Every buffer is on screen; the frame waits for one to come back.
                self.waiting = Some(frame);
                return;
            }
        };
        self.model.frame_shown(size);
        let mut shown = frame.clone();
        if self.model.has_caption() {
            let layout = self.model.caption_layout();
            let buttons = paint_buttons(
                &layout,
                self.model.dpi(),
                &CaptionPalette::of(self.tone),
                &CaptionState {
                    maximized: self.model.states.maximized,
                    active: self.model.states.activated,
                    ..self.caption
                },
            );
            shown.blend(
                &buttons,
                (3 * layout.button_width, layout.height),
                (layout.buttons_left(), 0),
            );
        }
        let buffer = &mut self.buffers[at];
        buffer.write(&shown.pixels);
        buffer.set_busy();
        let (width, height) = self.model.to_logical(size);
        self.surface.set_buffer_scale(self.model.scale.max(1) as i32);
        self.surface.attach(Some(&buffer.buffer), 0, 0);
        self.surface
            .damage_buffer(0, 0, size.0 as i32, size.1 as i32);
        self.xdg_surface.set_window_geometry(0, 0, width, height);
        self.surface.commit();
        self.last = Some(frame);
    }

    /// Which caption button the pointer is over, if any, for the hover look.
    pub fn hover(&mut self, button: Option<CaptionButton>) -> bool {
        if self.caption.hovered == button {
            return false;
        }
        self.caption.hovered = button;
        true
    }
}

impl Dispatch<WlSurface, i32> for State {
    fn event(
        state: &mut Self,
        _: &WlSurface,
        event: wl_surface::Event,
        task_id: &i32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let scale = match event {
            wl_surface::Event::Enter { output } => {
                let name = wayland_client::Proxy::data::<u32>(&output).copied();
                name.map(|name| state.output_scale(name))
            }
            wl_surface::Event::PreferredBufferScale { factor } => Some(factor.max(1) as u32),
            _ => None,
        };
        let Some(scale) = scale else {
            return;
        };
        let task_id = *task_id;
        let Some(window) = state.windows.get_mut(&task_id) else {
            return;
        };
        if let Some(dpi) = window.model.rescale(scale) {
            if window.dpi_told {
                state.report(FromHost::Dpi { task_id, dpi });
            }
            state.after_mode(task_id);
        }
    }
}

impl Dispatch<XdgSurface, i32> for State {
    fn event(
        state: &mut Self,
        xdg_surface: &XdgSurface,
        event: xdg_surface::Event,
        task_id: &i32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let xdg_surface::Event::Configure { serial } = event else {
            return;
        };
        xdg_surface.ack_configure(serial);
        let task_id = *task_id;
        let first = match state.windows.get_mut(&task_id) {
            Some(window) => !std::mem::replace(&mut window.configured, true),
            None => return,
        };
        if first {
            // The window is open: the window rules hear its DPI and its title bar's height.
            let (dpi, caption) = match state.windows.get_mut(&task_id) {
                Some(window) => {
                    window.dpi_told = true;
                    window.limits();
                    (window.model.dpi(), window.model.caption_change())
                }
                None => return,
            };
            state.report(FromHost::Dpi { task_id, dpi });
            if let Some(height) = caption {
                state.report(FromHost::Caption { task_id, height });
            }
        }
        state.redraw(task_id);
    }
}

/// The states of an `xdg_toplevel.configure`, from its array of 32-bit values.
fn states(raw: &[u8]) -> States {
    let mut states = States::default();
    for value in raw.chunks_exact(4) {
        let value = u32::from_ne_bytes([value[0], value[1], value[2], value[3]]);
        match xdg_toplevel::State::try_from(value) {
            Ok(xdg_toplevel::State::Maximized) => states.maximized = true,
            Ok(xdg_toplevel::State::Fullscreen) => states.fullscreen = true,
            Ok(xdg_toplevel::State::Resizing) => states.resizing = true,
            Ok(xdg_toplevel::State::Activated) => states.activated = true,
            Ok(xdg_toplevel::State::Suspended) => states.suspended = true,
            _ => {}
        }
    }
    states
}

impl Dispatch<XdgToplevel, i32> for State {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        task_id: &i32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let task_id = *task_id;
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states: raw,
            } => {
                let Some(window) = state.windows.get_mut(&task_id) else {
                    return;
                };
                let maximized = window.model.states.maximized;
                let reported = window.model.configure((width, height), states(&raw));
                let maximized_changed = window.model.states.maximized != maximized;
                for report in reported {
                    let report = match report {
                        Reported::Resized { width, height } => FromHost::Resized {
                            task_id,
                            width,
                            height,
                        },
                        Reported::Dragging(dragging) => FromHost::Dragging { task_id, dragging },
                        Reported::Focus(focused) => FromHost::Focus { task_id, focused },
                        Reported::Shown(shown) => FromHost::Shown { task_id, shown },
                    };
                    state.report(report);
                }
                if maximized_changed {
                    state.redraw(task_id);
                }
            }
            xdg_toplevel::Event::Close => {
                state.report(FromHost::Closed { task_id });
            }
            _ => {}
        }
    }
}
