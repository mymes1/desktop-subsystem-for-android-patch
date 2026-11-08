//! The window menu as a popup (`xdg_popup`) of its task window, and what its items do.
//!
//! The menu opens below the icon's cell when the cell is pressed, and at the pointer on a
//! right click on the title bar. It takes the pointer and the keyboard while it is open (a
//! popup grab), so a click elsewhere closes it, Escape closes it, the arrow keys move through
//! it and Enter chooses. A press that opened it and its release choose nothing: a release
//! chooses only after a press in the menu, or once the pointer moved in it, so pressing and
//! letting go where the menu opens never picks the item under the pointer.
//!
//! Screenshot is taken here: the window's last frame without the title bar's band goes to the
//! clipboard as `image/png` and into the Screenshots folder, encoded on a thread of its own.

use std::io::Write as _;
use std::sync::{Arc, OnceLock};

use dsa_host_core::compositor::window_hosts::FromHost;
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_shm::Format;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::shell::client::xdg_popup::{self, XdgPopup};
use wayland_protocols::xdg::shell::client::xdg_positioner::{
    Anchor, ConstraintAdjustment, Gravity, XdgPositioner,
};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};

use super::shm::ShmBuffer;
use super::State;
use crate::compositor::session::log;
use crate::compositor::title_bar::Coverage;
use crate::compositor::window_menu::{
    action, items, label, menu_text, paint_menu, MenuItem, MenuLayout, TEXT_AT_100,
};
use crate::shell::png::{self, Rgba};
use crate::shell::screenshot;

/// The user data of the menu's surfaces, which no task's window has.
#[derive(Debug, Clone, Copy)]
pub struct MenuSurface;

/// How far the pointer moves in the menu, in logical units, before a release may choose.
const ARMING_MOVE: f64 = 4.0;

/// The evdev key codes the menu answers.
mod key {
    pub const ESC: u32 = 1;
    pub const ENTER: u32 = 28;
    pub const SPACE: u32 = 57;
    pub const UP: u32 = 103;
    pub const DOWN: u32 = 108;
    pub const KP_ENTER: u32 = 96;
}

/// Where the menu opens against its window, in logical units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuPlace {
    /// Below this rectangle (the icon's cell): left, top, width, height.
    Below(i32, i32, i32, i32),
    /// At this point (where the title bar was right clicked).
    At(i32, i32),
}

/// The open window menu.
pub struct Menu {
    pub task_id: i32,
    layout: MenuLayout,
    labels: Vec<Option<Coverage>>,
    highlighted: Option<usize>,
    surface: WlSurface,
    xdg_surface: XdgSurface,
    popup: XdgPopup,
    buffer: Option<ShmBuffer>,
    scale: u32,
    configured: bool,
    /// Whether a release may choose the item under the pointer.
    armed: bool,
    /// Where the pointer first was in the menu, in logical units.
    first: Option<(f64, f64)>,
}

impl Menu {
    fn item_at(&self, (x, y): (f64, f64)) -> Option<usize> {
        let scale = f64::from(self.scale);
        self.layout.item_at(x * scale, y * scale)
    }
}

/// A screenshot on its way to the clipboard and the Screenshots folder.
pub struct Shot {
    image: Rgba,
    png: OnceLock<Vec<u8>>,
}

impl Shot {
    /// The screenshot as a PNG, encoded the first time it is asked for.
    fn png(&self) -> &[u8] {
        self.png.get_or_init(|| png::encode(&self.image, false))
    }
}

impl State {
    /// Opens the window menu of `task_id`'s window at `place`, from the press `serial`.
    /// Returns false when it cannot be drawn (no text, or the window has no title bar), so
    /// the caller opens the desktop's window menu instead.
    pub(crate) fn open_menu(&mut self, task_id: i32, place: MenuPlace, serial: u32) -> bool {
        self.close_menu();
        let (Some(text), Some(seat)) = (self.text.as_ref(), self.seat.as_ref()) else {
            return false;
        };
        let Some(window) = self.windows.get(&task_id) else {
            return false;
        };
        if window.pip.is_some() || !window.model.has_caption() {
            return false;
        }
        let scale = window.model.scale.max(1);
        let dpi = window.model.dpi();
        let words = menu_text(self.korean);
        let entries = items(window.model.states.maximized, window.multiple_windows);
        let px = f64::from(dsa_host_core::compositor::caption::scaled(TEXT_AT_100, dpi));
        let labels: Vec<Option<Coverage>> = entries
            .iter()
            .map(|item| label(&words, *item).and_then(|label| text.line(label, px, None)))
            .collect();
        let widths: Vec<u32> = labels
            .iter()
            .map(|label| label.as_ref().map_or(0, |label| label.width))
            .collect();
        let layout = MenuLayout::new(entries, &widths, dpi);
        let (width, height) = (
            layout.width.div_ceil(scale) as i32,
            layout.height.div_ceil(scale) as i32,
        );

        let surface = self.compositor.create_surface(&self.qh, MenuSurface);
        let xdg_surface = self
            .wm_base
            .get_xdg_surface(&surface, &self.qh, MenuSurface);
        let positioner = self.wm_base.create_positioner(&self.qh, ());
        positioner.set_size(width.max(1), height.max(1));
        match place {
            MenuPlace::Below(left, top, cell_width, cell_height) => {
                positioner.set_anchor_rect(left, top, cell_width.max(1), cell_height.max(1));
                positioner.set_anchor(Anchor::BottomLeft);
            }
            MenuPlace::At(x, y) => {
                positioner.set_anchor_rect(x, y, 1, 1);
                positioner.set_anchor(Anchor::TopLeft);
            }
        }
        positioner.set_gravity(Gravity::BottomRight);
        positioner.set_constraint_adjustment(
            ConstraintAdjustment::FlipX
                | ConstraintAdjustment::FlipY
                | ConstraintAdjustment::SlideX
                | ConstraintAdjustment::SlideY,
        );
        let popup = xdg_surface.get_popup(
            Some(window.xdg_surface()),
            &positioner,
            &self.qh,
            MenuSurface,
        );
        positioner.destroy();
        popup.grab(seat, serial);
        surface.commit();
        self.menu = Some(Menu {
            task_id,
            layout,
            labels,
            highlighted: None,
            surface,
            xdg_surface,
            popup,
            buffer: None,
            scale,
            configured: false,
            armed: false,
            first: None,
        });
        true
    }

    /// Closes the window menu, if it is open.
    pub(crate) fn close_menu(&mut self) {
        if let Some(menu) = self.menu.take() {
            menu.popup.destroy();
            menu.xdg_surface.destroy();
            menu.surface.destroy();
        }
        self.seat_state.on_menu = false;
    }

    fn draw_menu(&mut self) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        if !menu.configured {
            return;
        }
        let dpi = dsa_host_core::compositor::window_sizing::BASE_DPI * menu.scale;
        let pixels = paint_menu(&menu.layout, &menu.labels, menu.highlighted, dpi);
        let size = (menu.layout.width, menu.layout.height);
        let reuse = menu
            .buffer
            .as_ref()
            .is_some_and(|buffer| buffer.size == size && !buffer.is_busy());
        if !reuse {
            menu.buffer = ShmBuffer::with_format(&self.shm, &self.qh, size, Format::Argb8888).ok();
        }
        let Some(buffer) = &mut menu.buffer else {
            return;
        };
        buffer.write(&pixels);
        buffer.set_busy();
        menu.surface.set_buffer_scale(menu.scale as i32);
        menu.surface.attach(Some(&buffer.buffer), 0, 0);
        menu.surface
            .damage_buffer(0, 0, size.0 as i32, size.1 as i32);
        menu.surface.commit();
    }

    fn highlight(&mut self, item: Option<usize>) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        if menu.highlighted != item {
            menu.highlighted = item;
            self.draw_menu();
        }
    }

    /// The pointer is at `position` (logical units) on the menu.
    pub(crate) fn menu_pointer(&mut self, position: (f64, f64)) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        match menu.first {
            None => menu.first = Some(position),
            Some((x, y)) => {
                if (position.0 - x).abs() > ARMING_MOVE || (position.1 - y).abs() > ARMING_MOVE {
                    menu.armed = true;
                }
            }
        }
        let item = menu.item_at(position);
        self.highlight(item);
    }

    /// The pointer left the menu.
    pub(crate) fn menu_pointer_left(&mut self) {
        self.highlight(None);
    }

    /// A button went down or up at `position` (logical units) on the menu.
    pub(crate) fn menu_button(&mut self, serial: u32, position: (f64, f64), down: bool) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        if down {
            menu.armed = true;
            return;
        }
        if !menu.armed {
            return;
        }
        if let Some(index) = menu.item_at(position) {
            self.choose(index, serial);
        }
    }

    /// A touch on the menu at `position` (logical units) chooses what it touched.
    pub(crate) fn menu_touch(&mut self, serial: u32, position: (f64, f64)) {
        let Some(index) = self.menu.as_ref().and_then(|menu| menu.item_at(position)) else {
            return;
        };
        self.choose(index, serial);
    }

    /// A key went down while the menu is open; the menu takes every key.
    pub(crate) fn menu_key(&mut self, serial: u32, code: u32) {
        let Some((highlighted, next)) = self.menu.as_ref().map(|menu| {
            let next = matches!(code, key::UP | key::DOWN)
                .then(|| menu.layout.step(menu.highlighted, code == key::DOWN))
                .flatten();
            (menu.highlighted, next)
        }) else {
            return;
        };
        match code {
            key::ESC => self.close_menu(),
            key::UP | key::DOWN => self.highlight(next),
            key::ENTER | key::KP_ENTER | key::SPACE => {
                if let Some(index) = highlighted {
                    self.choose(index, serial);
                }
            }
            _ => {}
        }
    }

    /// Carries out the menu's entry `index`, from the input event `serial`.
    fn choose(&mut self, index: usize, serial: u32) {
        let Some((task_id, item)) = self
            .menu
            .as_ref()
            .and_then(|menu| Some((menu.task_id, *menu.layout.items.get(index)?)))
        else {
            return;
        };
        // The clipboard takes the serial while the menu still has the keyboard.
        if item == MenuItem::Screenshot {
            self.screenshot(task_id, serial);
        }
        self.close_menu();
        let Some(window) = self.windows.get(&task_id) else {
            return;
        };
        match item {
            MenuItem::Minimize => window.toplevel.set_minimized(),
            MenuItem::Maximize => window.toplevel.set_maximized(),
            MenuItem::Restore => window.toplevel.unset_maximized(),
            MenuItem::Close => self.report(FromHost::Closed { task_id }),
            MenuItem::Separator | MenuItem::Screenshot => {}
            other => {
                if let Some(action) = action(other) {
                    self.report(FromHost::MenuChosen { task_id, action });
                }
            }
        }
    }

    /// Takes a screenshot of `task_id`'s window: its last frame without the title bar's
    /// band, to the clipboard (from the input event `serial`) and the Screenshots folder.
    fn screenshot(&mut self, task_id: i32, serial: u32) {
        let Some(window) = self.windows.get(&task_id) else {
            return;
        };
        let band = window.model.caption_height();
        let frame = match window.last_gpu {
            Some((display_id, frame)) => self.read_gpu_frame(display_id, frame),
            None => window
                .last
                .clone()
                .ok_or_else(|| "the app has not drawn its window yet".to_string()),
        };
        let image = frame.and_then(|frame| {
            screenshot::app_content(&frame.pixels, frame.width, frame.height, band)
                .ok_or_else(|| "the window shows nothing under its title bar".to_string())
        });
        let image = match image {
            Ok(image) => image,
            Err(why) => {
                log(&format!(
                    "task {task_id}: the screenshot was not taken: {why}"
                ));
                return;
            }
        };
        let shot = Arc::new(Shot {
            image,
            png: OnceLock::new(),
        });
        self.set_clipboard_shot(Arc::clone(&shot), serial);
        let prefix = menu_text(self.korean).screenshot_file;
        let spawned = std::thread::Builder::new()
            .name("dsa-screenshot".into())
            .spawn(move || match screenshot::save(shot.png(), prefix) {
                Ok(path) => log(&format!(
                    "task {task_id}: screenshot saved as {}",
                    path.display()
                )),
                Err(why) => log(&format!(
                    "task {task_id}: the screenshot was not saved: {why}"
                )),
            });
        if let Err(why) = spawned {
            log(&format!("the screenshot could not be saved: {why}"));
        }
    }

    /// The screenshot becomes the desktop's clipboard, as `image/png`.
    fn set_clipboard_shot(&mut self, shot: Arc<Shot>, serial: u32) {
        let (Some(manager), Some(device)) = (&self.data_manager, &self.data_device) else {
            return;
        };
        let source = manager.create_data_source(&self.qh, shot);
        source.offer("image/png".to_string());
        device.set_selection(Some(&source), serial);
        self.data_state.replace_source(source);
    }
}

impl Dispatch<WlSurface, MenuSurface> for State {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: wl_surface::Event,
        _: &MenuSurface,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgSurface, MenuSurface> for State {
    fn event(
        state: &mut Self,
        xdg_surface: &XdgSurface,
        event: xdg_surface::Event,
        _: &MenuSurface,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            if let Some(menu) = state.menu.as_mut() {
                if &menu.xdg_surface == xdg_surface {
                    menu.configured = true;
                }
            }
            state.draw_menu();
        }
    }
}

impl Dispatch<XdgPopup, MenuSurface> for State {
    fn event(
        state: &mut Self,
        popup: &XdgPopup,
        event: xdg_popup::Event,
        _: &MenuSurface,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_popup::Event::PopupDone = event {
            if state.menu.as_ref().is_some_and(|menu| &menu.popup == popup) {
                state.close_menu();
            }
        }
    }
}

delegate_noop!(State: ignore XdgPositioner);

impl Dispatch<WlDataSource, Arc<Shot>> for State {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        shot: &Arc<Shot>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { fd, .. } => {
                let shot = Arc::clone(shot);
                std::thread::spawn(move || {
                    let mut file = std::fs::File::from(fd);
                    let _ = file.write_all(shot.png());
                });
            }
            wl_data_source::Event::Cancelled => {
                state.data_state.source_cancelled(source);
                source.destroy();
            }
            _ => {}
        }
    }
}
