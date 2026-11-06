//! One task window's state as the desktop sees it, with no Wayland in it.
//!
//! The compositor proposes sizes and states (`xdg_toplevel.configure`); the window turns them
//! into what the window rules want to hear (resized, dragging, focused, shown), in the guest's
//! pixels. The window never stretches a frame: it takes the size of the frame it shows, so a
//! proposed size only becomes the window's size once the task has drawn a frame of it, as on
//! Windows. Sizes on the wire are in the desktop's logical units, which the output's scale
//! turns into pixels.

use dsa_host_core::compositor::caption::{scaled, CaptionLayout};
use dsa_host_core::compositor::window_mode::{ModeState, WindowMode};
use dsa_host_core::compositor::window_sizing::BASE_DPI;
use dsa_host_core::input::WindowGeometry;

/// The title bar's height at 100%, in pixels: the height of a caption with its top border
/// on Windows 11, so the apps lay out the same on both hosts.
pub const CAPTION_AT_100: u32 = 32;

/// How deep the resize edges a window draws itself are, at 100%.
pub const RESIZE_BORDER_AT_100: u32 = 6;

/// The states `xdg_toplevel.configure` carries that the window rules use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct States {
    pub maximized: bool,
    pub fullscreen: bool,
    pub resizing: bool,
    pub activated: bool,
    /// The window is not shown at all (minimized, or on another workspace).
    pub suspended: bool,
}

/// What a configure means to the window rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reported {
    /// The user (or the desktop) gave the window this client size, in pixels.
    Resized { width: u32, height: u32 },
    /// The user started dragging the window's edge, or let go.
    Dragging(bool),
    /// The window became the active one, or stopped being it.
    Focus(bool),
    /// The window was hidden, or shown again.
    Shown(bool),
}

/// One task window.
#[derive(Debug, Clone)]
pub struct Toplevel {
    pub task_id: i32,
    pub package: String,
    pub title: String,
    /// The output scale the window is on: one logical unit is this many pixels.
    pub scale: u32,
    /// The size of the frame the window shows now, in pixels.
    pub shown: (u32, u32),
    /// The task's size in the guest, which pointer input is measured in.
    pub task: (u32, u32),
    pub max: Option<(u32, u32)>,
    pub min: Option<(u32, u32)>,
    pub mode: ModeState,
    pub resizable: bool,
    pub states: States,
    /// The last size the desktop proposed, in pixels.
    proposed: Option<(u32, u32)>,
    /// The title bar height last told to the guest.
    caption_told: Option<u32>,
}

impl Toplevel {
    pub fn new(task_id: i32, package: &str, title: &str, size: (u32, u32)) -> Toplevel {
        Toplevel {
            task_id,
            package: package.into(),
            title: title.into(),
            scale: 1,
            shown: size,
            task: size,
            max: None,
            min: None,
            mode: ModeState::new(),
            resizable: true,
            states: States::default(),
            proposed: None,
            caption_told: None,
        }
    }

    /// The window's DPI: 96 at a scale of 1.
    pub fn dpi(&self) -> u32 {
        BASE_DPI * self.scale.max(1)
    }

    /// A size in logical units as pixels.
    pub fn to_pixels(&self, (width, height): (i32, i32)) -> (u32, u32) {
        let scale = self.scale.max(1);
        (width.max(0) as u32 * scale, height.max(0) as u32 * scale)
    }

    /// A size in pixels as logical units, rounded up so no pixel is lost.
    pub fn to_logical(&self, (width, height): (u32, u32)) -> (i32, i32) {
        let scale = self.scale.max(1);
        (width.div_ceil(scale) as i32, height.div_ceil(scale) as i32)
    }

    /// A point in logical units as pixels of the window.
    pub fn point_to_pixels(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let scale = f64::from(self.scale.max(1));
        (x * scale, y * scale)
    }

    /// The sizes pointer input is converted between.
    pub fn geometry(&self) -> WindowGeometry {
        WindowGeometry {
            client: self.shown,
            task: self.task,
        }
    }

    /// Whether the window draws its title bar: an ordinary window does; a full screen or
    /// picture-in-picture one shows only the app.
    pub fn has_caption(&self) -> bool {
        self.mode.effective() == WindowMode::Normal
    }

    /// The title bar's height in pixels; 0 when the window has none.
    pub fn caption_height(&self) -> u32 {
        if self.has_caption() {
            scaled(CAPTION_AT_100, self.dpi())
        } else {
            0
        }
    }

    /// The title bar's layout over the frame shown now. A maximized or full screen window,
    /// or one the user may not resize, has no edges to drag.
    pub fn caption_layout(&self) -> CaptionLayout {
        let border = if self.states.maximized || self.states.fullscreen || !self.resizable {
            0
        } else {
            scaled(RESIZE_BORDER_AT_100, self.dpi()).min(self.caption_height())
        };
        CaptionLayout::new(self.shown.0, self.caption_height(), self.dpi(), border)
    }

    /// The title bar's height when the guest has not been told it yet: the app's top inset.
    pub fn caption_change(&mut self) -> Option<u32> {
        let height = self.caption_height();
        if self.caption_told == Some(height) {
            return None;
        }
        self.caption_told = Some(height);
        Some(height)
    }

    /// `xdg_toplevel.configure`: the desktop proposes `logical` (0 on an axis leaves it to
    /// the window) with `states`.
    pub fn configure(&mut self, logical: (i32, i32), states: States) -> Vec<Reported> {
        let mut reported = Vec::new();
        let before = self.states;
        self.states = states;
        if states.activated != before.activated {
            reported.push(Reported::Focus(states.activated));
        }
        if states.suspended != before.suspended {
            reported.push(Reported::Shown(!states.suspended));
        }
        if states.resizing && !before.resizing {
            reported.push(Reported::Dragging(true));
        }
        if logical.0 > 0 && logical.1 > 0 {
            let size = self.to_pixels(logical);
            if Some(size) != self.proposed && size != self.shown {
                reported.push(Reported::Resized {
                    width: size.0,
                    height: size.1,
                });
            }
            self.proposed = Some(size);
        }
        if !states.resizing && before.resizing {
            reported.push(Reported::Dragging(false));
        }
        reported
    }

    /// The output scale changed (`wl_surface.preferred_buffer_scale`, or the output the
    /// window entered). Returns the new DPI when it changed.
    pub fn rescale(&mut self, scale: u32) -> Option<u32> {
        let scale = scale.max(1);
        if scale == self.scale {
            return None;
        }
        self.scale = scale;
        self.proposed = None;
        Some(self.dpi())
    }

    /// The window shows a frame of `size` pixels: that is its size now.
    pub fn frame_shown(&mut self, size: (u32, u32)) {
        self.shown = size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Toplevel {
        Toplevel::new(7, "com.example", "Example", (400, 600))
    }

    #[test]
    fn fr4_a_proposed_size_is_reported_once_in_pixels() {
        let mut window = window();
        window.rescale(2);
        let states = States {
            activated: true,
            ..States::default()
        };
        assert_eq!(
            window.configure((300, 200), states),
            vec![
                Reported::Focus(true),
                Reported::Resized {
                    width: 600,
                    height: 400
                }
            ]
        );
        assert_eq!(window.configure((300, 200), states), vec![]);
        assert_eq!(
            window.configure((0, 0), states),
            vec![],
            "no size: the window decides"
        );
    }

    #[test]
    fn fr4_the_window_keeps_the_size_of_the_frame_it_shows_until_a_new_one_comes() {
        let mut window = window();
        window.configure((500, 700), States::default());
        assert_eq!(
            window.shown,
            (400, 600),
            "nothing is stretched to the new size"
        );
        window.frame_shown((500, 700));
        assert_eq!(window.geometry().client, (500, 700));
    }

    #[test]
    fn fr4_dragging_an_edge_starts_and_ends_a_live_resize() {
        let mut window = window();
        let dragging = States {
            resizing: true,
            ..States::default()
        };
        assert_eq!(
            window.configure((410, 600), dragging),
            vec![
                Reported::Dragging(true),
                Reported::Resized {
                    width: 410,
                    height: 600
                }
            ]
        );
        assert_eq!(
            window.configure((420, 600), States::default()),
            vec![
                Reported::Resized {
                    width: 420,
                    height: 600
                },
                Reported::Dragging(false)
            ]
        );
    }

    #[test]
    fn fr4_moving_to_a_scaled_output_changes_the_dpi() {
        let mut window = window();
        assert_eq!(window.dpi(), 96);
        assert_eq!(window.rescale(2), Some(192));
        assert_eq!(window.rescale(2), None);
        assert_eq!(window.to_logical((401, 600)), (201, 300));
    }

    #[test]
    fn fr10_a_minimized_window_says_it_is_hidden() {
        let mut window = window();
        let hidden = States {
            suspended: true,
            ..States::default()
        };
        assert_eq!(
            window.configure((0, 0), hidden),
            vec![Reported::Shown(false)]
        );
        assert_eq!(
            window.configure((0, 0), States::default()),
            vec![Reported::Shown(true)]
        );
    }

    #[test]
    fn fr22_the_title_bar_scales_and_goes_away_in_full_screen() {
        let mut window = window();
        assert_eq!(window.caption_height(), 32);
        assert_eq!(window.caption_change(), Some(32));
        assert_eq!(window.caption_change(), None);
        window.rescale(2);
        assert_eq!(window.caption_change(), Some(64));
        window.mode.toggle_fullscreen();
        assert_eq!(window.caption_height(), 0);
        assert_eq!(window.caption_change(), Some(0));
    }

    #[test]
    fn fr22_a_maximized_window_has_no_resize_edge_in_its_title_bar() {
        let mut window = window();
        assert!(window.caption_layout().resize_border > 0);
        window.configure(
            (0, 0),
            States {
                maximized: true,
                ..States::default()
            },
        );
        assert_eq!(window.caption_layout().resize_border, 0);
    }
}
