//! `wl_pointer`'s buttons and wheel, and the edges of a window that a press resizes.

use dsa_host_core::compositor::caption::{CaptionHit, CaptionLayout};
use dsa_host_core::input::MouseButton;

/// The evdev button codes `wl_pointer.button` reports.
pub mod evdev_button {
    pub const LEFT: u32 = 0x110;
    pub const RIGHT: u32 = 0x111;
    pub const MIDDLE: u32 = 0x112;
    pub const SIDE: u32 = 0x113;
    pub const EXTRA: u32 = 0x114;
    pub const FORWARD: u32 = 0x115;
    pub const BACK: u32 = 0x116;
    /// A pen's barrel buttons, as `zwp_tablet_tool_v2.button` reports them.
    pub const STYLUS: u32 = 0x14b;
    pub const STYLUS2: u32 = 0x14c;
}

/// The mouse button with evdev code `code`; `None` for one the guest has no use for.
pub fn mouse_button(code: u32) -> Option<MouseButton> {
    use evdev_button as b;
    Some(match code {
        b::LEFT => MouseButton::Left,
        b::RIGHT => MouseButton::Right,
        b::MIDDLE => MouseButton::Middle,
        b::SIDE | b::BACK => MouseButton::Back,
        b::EXTRA | b::FORWARD => MouseButton::Forward,
        _ => return None,
    })
}

/// The mouse button an X11 `ButtonPress` names by its `detail`: 1 to 3 are the left, middle
/// and right buttons, 8 and 9 the back and forward buttons (4 to 7 are the wheel's steps).
pub fn x11_button(detail: u8) -> Option<MouseButton> {
    Some(match detail {
        1 => MouseButton::Left,
        2 => MouseButton::Middle,
        3 => MouseButton::Right,
        8 => MouseButton::Back,
        9 => MouseButton::Forward,
        _ => return None,
    })
}

/// How far a continuous scroll (a touchpad's two fingers) moves for one wheel notch, in
/// `wl_pointer.axis` units. A wheel reports its notches exactly, through `axis_value120`.
pub const AXIS_PER_NOTCH: f64 = 10.0;

/// One `wl_pointer.frame`'s scrolling, gathered from its axis events.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Scroll {
    /// From `axis_value120` (or `axis_discrete`, times 120): a wheel's notches, exactly.
    value120: (i32, i32),
    /// From `axis`: continuous scrolling, in surface units.
    continuous: (f64, f64),
}

impl Scroll {
    /// A wheel moved `value120` 120ths of a notch on an axis (0 vertical, 1 horizontal).
    pub fn value120(&mut self, axis: u32, value120: i32) {
        if axis == 1 {
            self.value120.0 += value120;
        } else {
            self.value120.1 += value120;
        }
    }

    /// The pointer scrolled `value` surface units on an axis (0 vertical, 1 horizontal).
    pub fn continuous(&mut self, axis: u32, value: f64) {
        if axis == 1 {
            self.continuous.0 += value;
        } else {
            self.continuous.1 += value;
        }
    }

    /// The notches the frame comes to (horizontal, vertical), positive to the right and away
    /// from the user as the guest counts them; `None` when it did not scroll. Wayland counts
    /// downwards as positive, so the vertical axis turns over. A wheel's own notches win over
    /// the continuous values the compositor sends beside them.
    pub fn take(&mut self) -> Option<(f32, f32)> {
        let scroll = std::mem::take(self);
        let (horizontal, vertical) = if scroll.value120 != (0, 0) {
            (
                f64::from(scroll.value120.0) / 120.0,
                f64::from(scroll.value120.1) / 120.0,
            )
        } else {
            (
                scroll.continuous.0 / AXIS_PER_NOTCH,
                scroll.continuous.1 / AXIS_PER_NOTCH,
            )
        };
        if horizontal == 0.0 && vertical == 0.0 {
            return None;
        }
        Some((horizontal as f32, -vertical as f32))
    }
}

/// An edge of a window, as `xdg_toplevel.resize` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top = 1,
    Bottom = 2,
    Left = 4,
    TopLeft = 5,
    BottomLeft = 6,
    Right = 8,
    TopRight = 9,
    BottomRight = 10,
}

/// What a press at (`x`, `y`) of a window `size` pixels large does when the window draws its
/// own frame: resize it by an edge within `border` pixels of it, or act on the title bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowHit {
    /// The press goes to the app, or to the title bar's own rules ([`CaptionHit`]).
    Caption(CaptionHit),
    /// The press starts resizing the window by this edge.
    Resize(Edge),
}

/// Where a press lands in a window that draws its own frame (no server-side decorations):
/// the left, right and bottom edges, within `border` pixels, resize it, and the title bar
/// decides the rest, the top edge among it. A maximized or full screen window has no edges.
pub fn window_hit(
    layout: &CaptionLayout,
    size: (u32, u32),
    border: u32,
    x: i32,
    y: i32,
) -> WindowHit {
    let (width, height) = (size.0 as i32, size.1 as i32);
    let border = border as i32;
    if border > 0 && layout.resize_border > 0 {
        let left = x < border;
        let right = x >= width - border;
        let bottom = y >= height - border;
        let edge = match (left, right, bottom) {
            (true, _, true) => Some(Edge::BottomLeft),
            (_, true, true) => Some(Edge::BottomRight),
            (_, _, true) => Some(Edge::Bottom),
            (true, _, _) if y >= layout.resize_border as i32 => Some(Edge::Left),
            (_, true, _) if y >= layout.resize_border as i32 => Some(Edge::Right),
            _ => None,
        };
        if let Some(edge) = edge {
            return WindowHit::Resize(edge);
        }
    }
    match layout.hit(x, y) {
        CaptionHit::Top => WindowHit::Resize(Edge::Top),
        CaptionHit::TopLeft => WindowHit::Resize(Edge::TopLeft),
        CaptionHit::TopRight => WindowHit::Resize(Edge::TopRight),
        hit => WindowHit::Caption(hit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsa_host_core::compositor::caption::CaptionButton;

    #[test]
    fn fr5_mouse_buttons_are_named_as_the_guest_names_them() {
        assert_eq!(mouse_button(evdev_button::LEFT), Some(MouseButton::Left));
        assert_eq!(mouse_button(evdev_button::RIGHT), Some(MouseButton::Right));
        assert_eq!(
            mouse_button(evdev_button::MIDDLE),
            Some(MouseButton::Middle)
        );
        assert_eq!(mouse_button(evdev_button::SIDE), Some(MouseButton::Back));
        assert_eq!(
            mouse_button(evdev_button::EXTRA),
            Some(MouseButton::Forward)
        );
        assert_eq!(mouse_button(0x120), None);
    }

    #[test]
    fn fr5_a_wheel_notch_scrolls_one_notch_away_from_the_user() {
        let mut scroll = Scroll::default();
        // Wayland: the wheel turned one notch towards the user is +120 vertically.
        scroll.value120(0, -120);
        scroll.continuous(0, -15.0);
        assert_eq!(
            scroll.take(),
            Some((0.0, 1.0)),
            "the notch wins over the axis value"
        );
        assert_eq!(scroll.take(), None, "a frame's scroll is taken once");
    }

    #[test]
    fn fr5_a_touchpad_scrolls_smoothly_in_fractions_of_a_notch() {
        let mut scroll = Scroll::default();
        scroll.continuous(1, 5.0);
        scroll.continuous(0, 2.5);
        assert_eq!(scroll.take(), Some((0.5, -0.25)));
    }

    #[test]
    fn fr22_a_window_that_draws_its_frame_resizes_by_its_edges_and_moves_by_its_title_bar() {
        let layout = CaptionLayout::new(400, 32, 96, 4);
        let size = (400, 300);
        assert_eq!(
            window_hit(&layout, size, 6, 1, 150),
            WindowHit::Resize(Edge::Left)
        );
        assert_eq!(
            window_hit(&layout, size, 6, 398, 150),
            WindowHit::Resize(Edge::Right)
        );
        assert_eq!(
            window_hit(&layout, size, 6, 200, 298),
            WindowHit::Resize(Edge::Bottom)
        );
        assert_eq!(
            window_hit(&layout, size, 6, 1, 298),
            WindowHit::Resize(Edge::BottomLeft)
        );
        assert_eq!(
            window_hit(&layout, size, 6, 200, 1),
            WindowHit::Resize(Edge::Top)
        );
        assert_eq!(
            window_hit(&layout, size, 6, 100, 16),
            WindowHit::Caption(CaptionHit::Caption)
        );
        assert_eq!(
            window_hit(&layout, size, 6, 390, 16),
            WindowHit::Caption(CaptionHit::Button(CaptionButton::Close))
        );
        assert_eq!(
            window_hit(&layout, size, 6, 200, 150),
            WindowHit::Caption(CaptionHit::Client)
        );
    }

    #[test]
    fn fr22_a_maximized_window_has_no_edges_to_resize() {
        let layout = CaptionLayout::new(400, 32, 96, 0);
        assert_eq!(
            window_hit(&layout, (400, 300), 0, 1, 150),
            WindowHit::Caption(CaptionHit::Client)
        );
    }

    #[test]
    fn fr5_the_mouse_back_and_forward_buttons_reach_the_guest_as_back_and_forward() {
        use dsa_host_core::input::buttons;
        // Wayland names them BTN_SIDE and BTN_EXTRA on most mice, BTN_BACK and BTN_FORWARD on
        // some; X11 numbers them 8 and 9. Each reaches the guest's pointer as bit 8 (back) or
        // bit 16 (forward), which the guest turns into KEYCODE_BACK and KEYCODE_FORWARD.
        for code in [evdev_button::SIDE, evdev_button::BACK] {
            assert_eq!(
                mouse_button(code).map(MouseButton::bit),
                Some(buttons::BACK)
            );
        }
        for code in [evdev_button::EXTRA, evdev_button::FORWARD] {
            assert_eq!(
                mouse_button(code).map(MouseButton::bit),
                Some(buttons::FORWARD)
            );
        }
        assert_eq!(buttons::BACK, 8);
        assert_eq!(buttons::FORWARD, 16);
        assert_eq!(x11_button(8).map(MouseButton::bit), Some(buttons::BACK));
        assert_eq!(x11_button(9).map(MouseButton::bit), Some(buttons::FORWARD));
        assert_eq!(x11_button(1), Some(MouseButton::Left));
        assert_eq!(x11_button(3), Some(MouseButton::Right));
        // The wheel's steps are no buttons.
        assert_eq!(x11_button(4), None);
    }
}
