//! Where the parts of a widget window are, in its pixels: its size at a monitor's scale, the
//! edges that resize it, its More button, and the room, border and handles of a widget being
//! edited.
//!
//! Every host measures its windows in device-independent pixels of 1/96 inch: Windows' DIPs,
//! a Wayland output's logical pixels, a Mac's points. A monitor at a scale of `s` has
//! `96 x s` of the host's pixels per inch, its `dpi` here, which is also Android's density
//! there, as a dp is 1/160 inch and 0.6 of a device-independent pixel.

use std::time::{Duration, Instant};

use crate::widgets::{self, Placement};

/// The width of the edges a widget window is resized by, in device-independent pixels.
pub const BORDER_DIP: i32 = 6;

/// The diameter of a widget window's More button, in device-independent pixels.
pub const MORE_BUTTON_DIP: u32 = 28;

/// How far the More button sits from the window's top and right edges, which keeps it
/// inside every corner radius an Android widget is drawn with (up to 41 dp) and off the edges
/// that resize the window.
pub const MORE_BUTTON_MARGIN_DIP: u32 = 8;

/// The size of the More button's glyph.
pub const MORE_GLYPH_DIP: u32 = 16;

/// How long the More button takes to fade in or out: WinUI's `ControlFastAnimationDuration`.
pub const FADE: Duration = Duration::from_millis(167);

/// How long a press on the widget is held still before it is a long press, which edits the
/// widget as a long press does on a phone's home screen.
pub const HOLD_TO_MOVE: Duration = Duration::from_millis(500);

/// How far a finger or a pen may wander, at least, before its press is a drag rather than a
/// long press, in device-independent pixels: about Android's touch slop, as a finger is never
/// as still as a mouse.
pub const TOUCH_SLOP_DIP: u32 = 8;

/// The room around a widget being edited, for its shadow and its handles, in
/// device-independent pixels.
pub const EDIT_MARGIN_DIP: u32 = 16;

/// How much a widget being edited grows on each side as it lifts, in device-independent
/// pixels.
pub const LIFT_DIP: u32 = 4;

/// The width of the accent border of a widget being edited.
pub const EDIT_BORDER_DIP: u32 = 2;

/// The corner radius of that border: the radius of a Windows 11 window.
pub const EDIT_RADIUS_DIP: u32 = 8;

/// The diameter of a resize handle of a widget being edited.
pub const HANDLE_DIP: u32 = 12;

/// How far from a handle's middle a press still grabs it.
pub const HANDLE_HIT_DIP: u32 = 10;

/// The host's density at a scale of 100%.
pub const HOST_BASE_DPI: u32 = 96;

/// `dp` at a monitor of `dpi` (96 at a scale of 100%), in pixels, rounded to the nearest.
/// A dp is one device-independent pixel.
pub fn px_of_dip(dp: u32, dpi: u32) -> i32 {
    let dpi = if dpi == 0 { HOST_BASE_DPI } else { dpi };
    ((u64::from(dp) * u64::from(dpi) + u64::from(HOST_BASE_DPI) / 2) / u64::from(HOST_BASE_DPI))
        .min(i32::MAX as u64) as i32
}

/// `px` pixels at a monitor of `dpi`, in dp, rounded to the nearest.
pub fn dip_of_px(px: i32, dpi: u32) -> u32 {
    let dpi = if dpi == 0 { HOST_BASE_DPI } else { dpi };
    let px = px.max(0) as u64;
    ((px * u64::from(HOST_BASE_DPI) + u64::from(dpi) / 2) / u64::from(dpi)) as u32
}

/// Android's density for a monitor of `dpi`: 96 at a scale of 100%, as a dp is 1/160 inch and
/// a device-independent pixel 1/96 inch.
pub fn android_density(dpi: u32) -> u32 {
    crate::compositor::window_sizing::density_for_dpi(dpi)
}

/// `dp` of Android's at a monitor of `dpi`, in pixels, rounded to the nearest: 0.6 of a
/// device-independent pixel each.
pub fn px_of_dp(dp: u32, dpi: u32) -> i32 {
    widgets::px_of(dp, android_density(dpi)).min(i32::MAX as u32) as i32
}

/// `px` pixels at a monitor of `dpi`, in Android's dp, rounded to the nearest.
pub fn dp_of_px(px: i32, dpi: u32) -> u32 {
    widgets::dp_of(px.max(0) as u32, android_density(dpi))
}

/// The size of a widget's window on a monitor of `dpi`, in pixels.
pub fn window_size(placement: &Placement, dpi: u32) -> (i32, i32) {
    (
        px_of_dp(placement.width_dp, dpi).max(1),
        px_of_dp(placement.height_dp, dpi).max(1),
    )
}

/// The placement of a window at `x`, `y` of `width` by `height` pixels on a monitor of `dpi`.
pub fn placement_of(x: i32, y: i32, width: i32, height: i32, dpi: u32) -> Placement {
    Placement {
        x,
        y,
        width_dp: dp_of_px(width, dpi).max(1),
        height_dp: dp_of_px(height, dpi).max(1),
    }
}

/// The smallest a widget's window may be made on a monitor of `dpi`, in pixels.
pub fn min_window_size(provider: &widgets::WidgetProvider, dpi: u32) -> (i32, i32) {
    let (width, height) = widgets::clamp_size(provider, 0, 0);
    (px_of_dp(width, dpi), px_of_dp(height, dpi))
}

/// The largest a widget's window may be made on a monitor of `dpi`, in pixels: the widget's
/// maximum, and its default along a side it cannot be resized along.
pub fn max_window_size(provider: &widgets::WidgetProvider, dpi: u32) -> (i32, i32) {
    let (width, height) = widgets::clamp_size(provider, widgets::MAX_SIDE, widgets::MAX_SIDE);
    (px_of_dp(width, dpi), px_of_dp(height, dpi))
}

/// How long a widget window waits for its first image from Android, after it opened, before
/// the window manager logs that it has none. Android draws a widget within a second of being
/// told its size, so a window still without an image after this shows only its
/// placeholder, and the log says so.
pub const NO_FRAME: Duration = Duration::from_secs(10);

/// How often the widget windows are checked for an image that never came.
pub const NO_FRAME_CHECK: Duration = Duration::from_secs(5);

/// Whether a window that has waited for an image since `waiting_since` (None: it has one) is
/// overdue at `now`, and not logged yet.
pub fn frame_overdue(waiting_since: Option<Instant>, noted: bool, now: Instant) -> bool {
    match waiting_since {
        Some(since) => !noted && now.saturating_duration_since(since) >= NO_FRAME,
        None => false,
    }
}

/// The corner radius of a widget window's placeholder, in device-independent pixels.
const PLACEHOLDER_RADIUS_DIP: u32 = 12;

/// What a widget window shows until its first image from Android comes, or while the image
/// it has is all clear: a dark grey card with a lighter edge and rounded corners, as
/// premultiplied BGRA rows of `width` by `height` pixels on a monitor of `dpi`. It is mostly
/// opaque, so the user sees where the widget is and can grab it even when Android has not
/// drawn it yet; a window that showed nothing would look like no widget at all.
pub fn placeholder(width: usize, height: usize, dpi: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; width * height * 4];
    let radius = (px_of_dip(PLACEHOLDER_RADIUS_DIP, dpi).max(1) as usize)
        .min(width / 2)
        .min(height / 2) as i64;
    let edge = px_of_dip(1, dpi).max(1) as i64;
    let (w, h) = (width as i64, height as i64);
    // Premultiplied BGRA: #383838 at 0xd8 inside, #909090 at 0xe8 on the edge.
    let fill = [0x2f, 0x2f, 0x2f, 0xd8];
    let border = [0x83, 0x83, 0x83, 0xe8];
    for y in 0..h {
        for x in 0..w {
            // How far into a rounded corner's square the pixel is, 0 outside the squares.
            let cx = if x < radius {
                radius - x
            } else if x >= w - radius {
                x - (w - radius - 1)
            } else {
                0
            };
            let cy = if y < radius {
                radius - y
            } else if y >= h - radius {
                y - (h - radius - 1)
            } else {
                0
            };
            let in_corner = cx > 0 && cy > 0;
            if in_corner && cx * cx + cy * cy > radius * radius {
                continue;
            }
            let inner = (radius - edge).max(0);
            let on_edge = x < edge
                || y < edge
                || x >= w - edge
                || y >= h - edge
                || (in_corner && cx * cx + cy * cy > inner * inner);
            let at = ((y * w + x) * 4) as usize;
            pixels[at..at + 4].copy_from_slice(if on_edge { &border } else { &fill });
        }
    }
    pixels
}

/// Whether premultiplied BGRA `pixels` show nothing: every alpha is zero. A widget always
/// draws something, so such an image is a drawing that went wrong, and the window shows its
/// placeholder instead of a see-through hole.
pub fn all_clear(pixels: &[u8]) -> bool {
    pixels.chunks_exact(4).all(|pixel| pixel[3] == 0)
}

/// Where in a widget window a point is, which says what dragging it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Inside the widget: input goes to Android.
    Client,
    /// The More button: a click opens the menu and a drag moves the window.
    More,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    /// Around a widget being edited, outside it: a press there ends the edit.
    Outside,
}

/// What `x`, `y` (from the window's top left corner) hits in a window of `width` by `height`
/// pixels on a monitor of `dpi`. Only the edges the widget may be resized along resize it;
/// there is no band along the top, so all of the widget but its More button is the widget's.
pub fn hit_test(x: i32, y: i32, width: i32, height: i32, dpi: u32, resize_mode: u32) -> Hit {
    let border = px_of_dip(BORDER_DIP as u32, dpi).max(1);
    let horizontal = resize_mode & widgets::RESIZE_HORIZONTAL != 0;
    let vertical = resize_mode & widgets::RESIZE_VERTICAL != 0;
    let left = horizontal && x < border;
    let right = horizontal && x >= width - border;
    let top = vertical && y < border;
    let bottom = vertical && y >= height - border;
    match (left, right, top, bottom) {
        (true, _, true, _) => Hit::TopLeft,
        (_, true, true, _) => Hit::TopRight,
        (true, _, _, true) => Hit::BottomLeft,
        (_, true, _, true) => Hit::BottomRight,
        (true, ..) => Hit::Left,
        (_, true, ..) => Hit::Right,
        (_, _, true, _) => Hit::Top,
        (.., true) => Hit::Bottom,
        _ if on_more_button(x, y, width, height, dpi) => Hit::More,
        _ => Hit::Client,
    }
}

/// Where the parts of a widget being edited are in its window, which has
/// [`EDIT_MARGIN_DIP`] of room on every side of the widget. Each is left, top, width and
/// height in the window's pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct EditLayout {
    /// The widget's own place, where it sits when it is not edited.
    pub widget: (i32, i32, i32, i32),
    /// The widget lifted: [`LIFT_DIP`] larger on each side, which its image is drawn into.
    pub lifted: (i32, i32, i32, i32),
    /// A handle in the middle of each edge the widget may be resized along, with that edge.
    pub handles: Vec<(Hit, Circle)>,
}

/// The room around a widget being edited on a monitor of `dpi`, in pixels.
pub fn edit_margin(dpi: u32) -> i32 {
    px_of_dip(EDIT_MARGIN_DIP, dpi)
}

/// The parts of a widget being edited in a window of `width` by `height` pixels (the widget
/// and its margin) on a monitor of `dpi`, with handles on the edges `resize_mode` allows.
pub fn edit_layout(width: i32, height: i32, dpi: u32, resize_mode: u32) -> EditLayout {
    let margin = edit_margin(dpi);
    let (w, h) = ((width - 2 * margin).max(1), (height - 2 * margin).max(1));
    let lift = px_of_dip(LIFT_DIP, dpi).min(margin);
    let lifted = (margin - lift, margin - lift, w + 2 * lift, h + 2 * lift);
    let (lx, ly, lw, lh) = lifted;
    let radius = px_of_dip(HANDLE_DIP, dpi) as f32 / 2.0;
    let (cx, cy) = (lx as f32 + lw as f32 / 2.0, ly as f32 + lh as f32 / 2.0);
    let at = |cx: f32, cy: f32| Circle { cx, cy, radius };
    let mut handles = Vec::with_capacity(4);
    if resize_mode & widgets::RESIZE_HORIZONTAL != 0 {
        handles.push((Hit::Left, at(lx as f32, cy)));
        handles.push((Hit::Right, at((lx + lw) as f32, cy)));
    }
    if resize_mode & widgets::RESIZE_VERTICAL != 0 {
        handles.push((Hit::Top, at(cx, ly as f32)));
        handles.push((Hit::Bottom, at(cx, (ly + lh) as f32)));
    }
    EditLayout {
        widget: (margin, margin, w, h),
        lifted,
        handles,
    }
}

/// What `x`, `y` (from the window's top left corner) hits in the window of a widget being
/// edited, `width` by `height` pixels on a monitor of `dpi`: a handle or an edge of the lifted
/// widget resizes it along the edges `resize_mode` allows, inside it is the widget (a drag
/// moves it), and around it is [`Hit::Outside`].
pub fn edit_hit_test(x: i32, y: i32, width: i32, height: i32, dpi: u32, resize_mode: u32) -> Hit {
    let layout = edit_layout(width, height, dpi, resize_mode);
    let reach = px_of_dip(HANDLE_HIT_DIP, dpi) as f32;
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    for (hit, handle) in &layout.handles {
        let (dx, dy) = (px - handle.cx, py - handle.cy);
        if dx * dx + dy * dy <= reach * reach {
            return *hit;
        }
    }
    let (lx, ly, lw, lh) = layout.lifted;
    let band = px_of_dip(BORDER_DIP as u32, dpi).max(1);
    let horizontal = resize_mode & widgets::RESIZE_HORIZONTAL != 0;
    let vertical = resize_mode & widgets::RESIZE_VERTICAL != 0;
    let within_x = x >= lx - band && x < lx + lw + band;
    let within_y = y >= ly - band && y < ly + lh + band;
    if within_x && within_y {
        let left = horizontal && x < lx + band;
        let right = horizontal && x >= lx + lw - band;
        let top = vertical && y < ly + band;
        let bottom = vertical && y >= ly + lh - band;
        match (left, right, top, bottom) {
            (true, _, true, _) => return Hit::TopLeft,
            (_, true, true, _) => return Hit::TopRight,
            (true, _, _, true) => return Hit::BottomLeft,
            (_, true, _, true) => return Hit::BottomRight,
            (true, ..) => return Hit::Left,
            (_, true, ..) => return Hit::Right,
            (_, _, true, _) => return Hit::Top,
            (.., true) => return Hit::Bottom,
            _ => {}
        }
    }
    if x >= lx && x < lx + lw && y >= ly && y < ly + lh {
        Hit::Client
    } else {
        Hit::Outside
    }
}

/// How far a press may move and still be a long press, at `drag` (the system's drag
/// threshold): that, or [`TOUCH_SLOP_DIP`] for a finger or a pen when that is more.
pub fn press_threshold(drag: (i32, i32), touch: bool, dpi: u32) -> (i32, i32) {
    if !touch {
        return drag;
    }
    let slop = px_of_dip(TOUCH_SLOP_DIP, dpi);
    (drag.0.max(slop), drag.1.max(slop))
}

/// A circle in a window's pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circle {
    pub cx: f32,
    pub cy: f32,
    pub radius: f32,
}

impl Circle {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (dx, dy) = (x - self.cx, y - self.cy);
        dx * dx + dy * dy <= self.radius * self.radius
    }

    /// The square of whole pixels around it: left, top, right and bottom.
    pub fn bounds(&self) -> (i32, i32, i32, i32) {
        (
            (self.cx - self.radius).floor() as i32,
            (self.cy - self.radius).floor() as i32,
            (self.cx + self.radius).ceil() as i32,
            (self.cy + self.radius).ceil() as i32,
        )
    }
}

/// The More button of a window of `width` by `height` pixels on a monitor of `dpi`: a circle
/// of [`MORE_BUTTON_DIP`] in its top right corner, [`MORE_BUTTON_MARGIN_DIP`] from the edges.
/// A widget too small for it gets a smaller one that still fits.
pub fn more_button(width: i32, height: i32, dpi: u32) -> Circle {
    let side = width.min(height).max(1) as f32;
    let diameter = (px_of_dip(MORE_BUTTON_DIP, dpi) as f32).min((side - 2.0).max(1.0));
    let room = ((side - diameter) / 2.0).max(0.0);
    let margin = (px_of_dip(MORE_BUTTON_MARGIN_DIP, dpi) as f32).min(room);
    let radius = diameter / 2.0;
    Circle {
        cx: width as f32 - margin - radius,
        cy: margin + radius,
        radius,
    }
}

/// Whether the pixel at `x`, `y` is on the More button of a window of `width` by `height`:
/// inside its circle, not merely its square.
pub fn on_more_button(x: i32, y: i32, width: i32, height: i32, dpi: u32) -> bool {
    more_button(width, height, dpi).contains(x as f32 + 0.5, y as f32 + 0.5)
}

/// Whether `circle` lies wholly inside a window of `width` by `height` whose corners are
/// rounded to `radius`, as the widget's own image is.
pub fn inside_rounded_window(circle: &Circle, width: i32, height: i32, radius: f32) -> bool {
    let (w, h, rho) = (width as f32, height as f32, circle.radius);
    let edges = circle.cx - rho >= 0.0
        && circle.cy - rho >= 0.0
        && circle.cx + rho <= w
        && circle.cy + rho <= h;
    if rho >= radius {
        return edges;
    }
    // A rounded rectangle is the rectangle `radius` inside it, grown by `radius`.
    let dx = (radius - circle.cx).max(circle.cx - (w - radius)).max(0.0);
    let dy = (radius - circle.cy).max(circle.cy - (h - radius)).max(0.0);
    edges && (dx * dx + dy * dy).sqrt() + rho <= radius + 0.01
}

/// The size of the More button's glyph box for a window of `width` by `height` on `dpi`.
pub fn more_glyph_size(width: i32, height: i32, dpi: u32) -> usize {
    let circle = more_button(width, height, dpi);
    let wanted = px_of_dip(MORE_GLYPH_DIP, dpi) as f32;
    (wanted.min(circle.radius * 2.0 * 0.6).round() as usize).max(1)
}

/// A rectangle in screen pixels: left, top, right and bottom.
pub type ScreenRect = (i32, i32, i32, i32);

/// Where a window of `width` by `height` pixels asked for at `x`, `y` goes so the user can
/// reach it: where asked when enough of it is on a monitor's work area, else moved into the
/// first work area, as after a monitor was unplugged.
pub fn reachable_origin(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    areas: &[ScreenRect],
) -> (i32, i32) {
    const ENOUGH: i32 = 48;
    let on_screen = areas.iter().any(|&(left, top, right, bottom)| {
        let w = (x + width).min(right) - x.max(left);
        let h = (y + height).min(bottom) - y.max(top);
        w >= ENOUGH.min(width) && h >= ENOUGH.min(height) && y >= top - ENOUGH / 2
    });
    if on_screen || areas.is_empty() {
        return (x, y);
    }
    let (left, top, right, bottom) = areas[0];
    (
        x.clamp(left, (right - width).max(left)),
        y.clamp(top, (bottom - height).max(top)),
    )
}

/// A point in a window of `window` pixels, in the pixels of a frame of `frame` pixels, which
/// differ while a resize has not reached Android yet.
pub fn to_frame(x: i32, y: i32, window: (i32, i32), frame: (u32, u32)) -> (f32, f32) {
    let scale = |v: i32, from: i32, to: u32| {
        if from <= 0 {
            v as f32
        } else {
            v as f32 * to as f32 / from as f32
        }
    };
    (
        scale(x, window.0, frame.0).clamp(0.0, frame.0 as f32),
        scale(y, window.1, frame.1).clamp(0.0, frame.1 as f32),
    )
}
