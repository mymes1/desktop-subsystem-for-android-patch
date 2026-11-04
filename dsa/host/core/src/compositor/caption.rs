//! The title bar a task window draws over the top of its app.
//!
//! A task window has no title bar of its own above the app: its client area covers the whole
//! window, the app draws to the very top, and the window draws only its caption buttons
//! (minimize, maximize or restore, close) over the app's top right corner. The guest gives
//! the task a top inset of the title bar's height, so the app keeps its own toolbar below
//! the buttons and draws its status bar background behind them.
//!
//! This module holds the rules the platform's window carries out:
//!
//! - [`CaptionLayout::hit`]: what a point in the window belongs to. The whole band of the
//!   title bar belongs to the window (its buttons, its top resize edge, and the rest a drag
//!   area), and only the points below it reach the app. An app receives the band as a system
//!   bar inset and keeps nothing it can be touched on there; sharing the band would make the
//!   same point sometimes drag the window and sometimes press the app.
//! - [`caption_tone`]: whether the buttons' glyphs are dark or light, from what the app told
//!   the system about its status bar, so they stay readable over it.
//! - [`paint_buttons`]: the buttons as an image, in the tone and state they are in.

use crate::tasks::StatusBarAppearance;

/// Whether a window's caption glyphs are dark (over a light status bar) or light.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CaptionTone {
    /// Dark glyphs, for a light status bar. A window whose app said nothing starts so,
    /// like a window with a light title bar.
    #[default]
    DarkGlyphs,
    /// Light glyphs, for a dark status bar.
    LightGlyphs,
}

/// The tone of a task window's caption glyphs: dark over a light status bar, light over a
/// dark one. What the app said about its status bar's icons decides; when it said nothing the
/// agent could read, the status bar's color does, if it set one; otherwise the glyphs are
/// dark.
pub fn caption_tone(appearance: StatusBarAppearance, status_bar_color: u32) -> CaptionTone {
    match appearance {
        StatusBarAppearance::Light => CaptionTone::DarkGlyphs,
        StatusBarAppearance::Dark => CaptionTone::LightGlyphs,
        StatusBarAppearance::Unspecified => {
            let alpha = status_bar_color >> 24;
            if alpha < 0x80 {
                return CaptionTone::DarkGlyphs;
            }
            if relative_luminance(status_bar_color) >= 0.5 {
                CaptionTone::DarkGlyphs
            } else {
                CaptionTone::LightGlyphs
            }
        }
    }
}

/// The luminance of an 0xAARRGGBB color, from 0 (black) to 1 (white), with the sRGB weights.
fn relative_luminance(argb: u32) -> f64 {
    let channel = |shift: u32| {
        let value = f64::from((argb >> shift) & 0xff) / 255.0;
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

/// One of the caption buttons, from left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptionButton {
    Minimize,
    Maximize,
    Close,
}

impl CaptionButton {
    /// The buttons in the order they stand, left to right.
    pub const ALL: [CaptionButton; 3] = [
        CaptionButton::Minimize,
        CaptionButton::Maximize,
        CaptionButton::Close,
    ];

    fn index(self) -> u32 {
        match self {
            CaptionButton::Minimize => 0,
            CaptionButton::Maximize => 1,
            CaptionButton::Close => 2,
        }
    }
}

/// What a point of a task window belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptionHit {
    /// The app: the point goes to the guest.
    Client,
    /// The title bar's drag area: dragging moves the window, a double click maximizes or
    /// restores it, and a right click opens the system menu.
    Caption,
    /// A caption button.
    Button(CaptionButton),
    /// The top resize edge, and its two corners.
    Top,
    TopLeft,
    TopRight,
}

/// The width of a caption button at 100%, in pixels.
pub const BUTTON_WIDTH_AT_100: u32 = 46;

/// The side of a caption glyph at 100%, in pixels.
const GLYPH_AT_100: f64 = 10.0;

/// Where a task window's title bar and its buttons are, in client pixels. The client area
/// covers the whole window, so its top row is the window's top row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptionLayout {
    /// The client area's width.
    pub width: u32,
    /// The title bar's height, which the app receives as its top inset.
    pub height: u32,
    /// The width of one caption button.
    pub button_width: u32,
    /// How far down from the top the window can be resized by its top edge; 0 while the
    /// window is maximized, which has no edge to drag.
    pub resize_border: u32,
}

impl CaptionLayout {
    /// The layout of a window `width` pixels wide, at `dpi` (96 is 100%), with a title bar
    /// `height` pixels high and a top resize edge `resize_border` pixels deep.
    pub fn new(width: u32, height: u32, dpi: u32, resize_border: u32) -> CaptionLayout {
        CaptionLayout {
            width,
            height,
            button_width: scaled(BUTTON_WIDTH_AT_100, dpi),
            resize_border,
        }
    }

    /// Where `button` is: its left edge, top edge, width and height, in client pixels. The
    /// buttons stand at the right end of the title bar, close rightmost. A window too narrow
    /// for all three keeps them in place and lets the left ones run off its left edge.
    pub fn button_rect(&self, button: CaptionButton) -> (i32, i32, u32, u32) {
        let from_right = 3 - button.index();
        let left = self.width as i64 - i64::from(from_right * self.button_width);
        (left as i32, 0, self.button_width, self.height)
    }

    /// The left edge of the buttons, in client pixels; may be negative in a narrow window.
    pub fn buttons_left(&self) -> i32 {
        self.button_rect(CaptionButton::Minimize).0
    }

    /// What the client point (`x`, `y`) belongs to. The top resize edge comes first, so the
    /// window can be resized above its buttons as at any other point of its top edge.
    pub fn hit(&self, x: i32, y: i32) -> CaptionHit {
        if y < 0 || y >= self.height as i32 || x < 0 || x >= self.width as i32 {
            return CaptionHit::Client;
        }
        if y < self.resize_border as i32 {
            let corner = (self.resize_border * 2) as i32;
            return if x < corner {
                CaptionHit::TopLeft
            } else if x >= self.width as i32 - corner {
                CaptionHit::TopRight
            } else {
                CaptionHit::Top
            };
        }
        for button in CaptionButton::ALL {
            let (left, _, width, _) = self.button_rect(button);
            if x >= left && x < left + width as i32 {
                return CaptionHit::Button(button);
            }
        }
        CaptionHit::Caption
    }
}

/// `length` at 100%, at `dpi`, rounded to the nearest pixel.
pub fn scaled(length: u32, dpi: u32) -> u32 {
    let dpi = if dpi == 0 { 96 } else { dpi };
    (u64::from(length) * u64::from(dpi) + 48) as u32 / 96
}

/// The colors the caption buttons are drawn in, each 0xAARRGGBB (not premultiplied).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptionPalette {
    /// The background of a button at rest: transparent, so the app shows through, except
    /// where a high contrast theme asks for its own colors.
    pub rest: u32,
    /// The glyphs at rest.
    pub glyph: u32,
    /// The glyphs of a button the pointer is over or presses, other than close.
    pub glyph_highlighted: u32,
    /// The glyphs while the window is not the active one.
    pub glyph_inactive: u32,
    /// The background of a button the pointer is over, and of one being pressed.
    pub hover: u32,
    pub pressed: u32,
    /// The close button's background and glyph while the pointer is over it or presses it.
    pub close_hover: u32,
    pub close_pressed: u32,
    pub close_glyph: u32,
}

impl CaptionPalette {
    /// The colors Windows 11 draws its own caption buttons in, for glyphs of `tone`.
    pub fn of(tone: CaptionTone) -> CaptionPalette {
        let close = CaptionPalette {
            rest: 0,
            glyph: 0,
            glyph_highlighted: 0,
            glyph_inactive: 0,
            hover: 0,
            pressed: 0,
            close_hover: 0xFFC4_2B1C,
            close_pressed: 0xE6C4_2B1C,
            close_glyph: 0xFFFF_FFFF,
        };
        match tone {
            CaptionTone::DarkGlyphs => CaptionPalette {
                glyph: 0xE400_0000,
                glyph_highlighted: 0xE400_0000,
                glyph_inactive: 0x5C00_0000,
                hover: 0x0900_0000,
                pressed: 0x0600_0000,
                ..close
            },
            CaptionTone::LightGlyphs => CaptionPalette {
                glyph: 0xFFFF_FFFF,
                glyph_highlighted: 0xFFFF_FFFF,
                glyph_inactive: 0x5DFF_FFFF,
                hover: 0x0FFF_FFFF,
                pressed: 0x0AFF_FFFF,
                ..close
            },
        }
    }
}

/// What the caption buttons show besides their tone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptionState {
    /// The button the pointer is over, if any.
    pub hovered: Option<CaptionButton>,
    /// The button being pressed, if any.
    pub pressed: Option<CaptionButton>,
    /// Whether the window is maximized, so the middle button restores it.
    pub maximized: bool,
    /// Whether the window is the active one.
    pub active: bool,
}

/// The three caption buttons side by side, as rows of premultiplied BGRA pixels,
/// `3 * layout.button_width` wide and `layout.height` high. Where a button has no
/// background the pixels are transparent, so the app shows through.
pub fn paint_buttons(
    layout: &CaptionLayout,
    dpi: u32,
    palette: &CaptionPalette,
    state: &CaptionState,
) -> Vec<u8> {
    let width = 3 * layout.button_width as usize;
    let height = layout.height as usize;
    let mut pixels = vec![0u8; width * height * 4];
    let scale = f64::from(if dpi == 0 { 96 } else { dpi }) / 96.0;
    let stroke = scale.round().max(1.0);
    for button in CaptionButton::ALL {
        let left = (button.index() * layout.button_width) as usize;
        let close = button == CaptionButton::Close;
        let pressed = state.pressed == Some(button);
        let highlighted = pressed || (state.hovered == Some(button) && state.pressed.is_none());
        let background = match (highlighted, pressed, close) {
            (false, _, _) => palette.rest,
            (true, true, true) => palette.close_pressed,
            (true, true, false) => palette.pressed,
            (true, false, true) => palette.close_hover,
            (true, false, false) => palette.hover,
        };
        let glyph = match (highlighted, close) {
            (true, true) => palette.close_glyph,
            (true, false) => palette.glyph_highlighted,
            (false, _) if state.active => palette.glyph,
            (false, _) => palette.glyph_inactive,
        };
        let center = (
            left as f64 + f64::from(layout.button_width) / 2.0,
            f64::from(layout.height) / 2.0,
        );
        let segments = glyph_segments(button, state.maximized, center, scale, stroke);
        for y in 0..height {
            for x in left..left + layout.button_width as usize {
                let point = (x as f64 + 0.5, y as f64 + 0.5);
                let distance = segments
                    .iter()
                    .map(|segment| distance_to_segment(point, *segment))
                    .fold(f64::INFINITY, f64::min);
                let coverage = (stroke / 2.0 + 0.5 - distance).clamp(0.0, 1.0);
                let pixel = over(premultiply(background, 1.0), premultiply(glyph, coverage));
                let at = (y * width + x) * 4;
                pixels[at..at + 4].copy_from_slice(&pixel);
            }
        }
    }
    pixels
}

type Segment = ((f64, f64), (f64, f64));

/// The strokes of `button`'s glyph around `center`. Each stroke runs along the middle of a
/// row or column of pixels, so a stroke of whole pixels lands on whole pixels.
fn glyph_segments(
    button: CaptionButton,
    maximized: bool,
    center: (f64, f64),
    scale: f64,
    stroke: f64,
) -> Vec<Segment> {
    let half = (GLYPH_AT_100 * scale / 2.0).round();
    // The middle of a pixel when the stroke is odd, a pixel edge when it is even.
    let snap = |value: f64| {
        if stroke as i64 % 2 == 1 {
            value.floor() + 0.5
        } else {
            value.round()
        }
    };
    let (cx, cy) = (snap(center.0), snap(center.1));
    let rect = |left: f64, top: f64, right: f64, bottom: f64| {
        vec![
            ((left, top), (right, top)),
            ((right, top), (right, bottom)),
            ((right, bottom), (left, bottom)),
            ((left, bottom), (left, top)),
        ]
    };
    match button {
        CaptionButton::Minimize => vec![((cx - half, cy), (cx + half, cy))],
        CaptionButton::Maximize if !maximized => rect(cx - half, cy - half, cx + half, cy + half),
        CaptionButton::Maximize => {
            let offset = (2.0 * scale).round();
            let mut segments = rect(cx - half, cy - half + offset, cx + half - offset, cy + half);
            segments.extend([
                ((cx - half + offset, cy - half + offset), (cx - half + offset, cy - half)),
                ((cx - half + offset, cy - half), (cx + half, cy - half)),
                ((cx + half, cy - half), (cx + half, cy + half - offset)),
                ((cx + half, cy + half - offset), (cx + half - offset, cy + half - offset)),
            ]);
            segments
        }
        CaptionButton::Close => vec![
            ((cx - half, cy - half), (cx + half, cy + half)),
            ((cx + half, cy - half), (cx - half, cy + half)),
        ],
    }
}

fn distance_to_segment(point: (f64, f64), (a, b): Segment) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 {
        0.0
    } else {
        (((point.0 - a.0) * dx + (point.1 - a.1) * dy) / length).clamp(0.0, 1.0)
    };
    let (x, y) = (a.0 + t * dx, a.1 + t * dy);
    ((point.0 - x).powi(2) + (point.1 - y).powi(2)).sqrt()
}

/// An 0xAARRGGBB color at `coverage`, as premultiplied BGRA in 0..=1.
fn premultiply(argb: u32, coverage: f64) -> [f64; 4] {
    let alpha = f64::from(argb >> 24) / 255.0 * coverage;
    let channel = |shift: u32| f64::from((argb >> shift) & 0xff) / 255.0 * alpha;
    [channel(0), channel(8), channel(16), alpha]
}

/// `top` over `bottom`, both premultiplied, as BGRA bytes.
fn over(bottom: [f64; 4], top: [f64; 4]) -> [u8; 4] {
    let mut out = [0u8; 4];
    for (index, value) in out.iter_mut().enumerate() {
        let mixed = top[index] + bottom[index] * (1.0 - top[3]);
        *value = (mixed * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window 800 pixels wide at 100%, with a title bar of 31 and a top edge of 8.
    fn at_100() -> CaptionLayout {
        CaptionLayout::new(800, 31, 96, 8)
    }

    #[test]
    fn fr22_glyphs_are_dark_on_a_light_status_bar_and_light_on_a_dark_one() {
        assert_eq!(
            caption_tone(StatusBarAppearance::Light, 0),
            CaptionTone::DarkGlyphs
        );
        assert_eq!(
            caption_tone(StatusBarAppearance::Dark, 0),
            CaptionTone::LightGlyphs
        );
        // What the app said wins over the color it set.
        assert_eq!(
            caption_tone(StatusBarAppearance::Light, 0xFF00_0000),
            CaptionTone::DarkGlyphs
        );
    }

    #[test]
    fn fr22_without_an_appearance_the_status_bar_color_decides() {
        let tone = |color| caption_tone(StatusBarAppearance::Unspecified, color);
        assert_eq!(tone(0xFFFF_FFFF), CaptionTone::DarkGlyphs);
        assert_eq!(tone(0xFF1F_1F1F), CaptionTone::LightGlyphs);
        assert_eq!(tone(0xFF67_50A4), CaptionTone::LightGlyphs, "a deep purple");
        assert_eq!(tone(0xFFFF_EB3B), CaptionTone::DarkGlyphs, "a yellow");
        // A transparent status bar says nothing about what is behind it.
        assert_eq!(tone(0x0000_0000), CaptionTone::DarkGlyphs);
        assert_eq!(tone(0x10FF_FFFF), CaptionTone::DarkGlyphs);
    }

    #[test]
    fn fr22_the_buttons_stand_at_the_right_end_of_the_title_bar() {
        let layout = at_100();
        assert_eq!(layout.button_width, 46);
        assert_eq!(layout.button_rect(CaptionButton::Close), (754, 0, 46, 31));
        assert_eq!(layout.button_rect(CaptionButton::Maximize), (708, 0, 46, 31));
        assert_eq!(layout.button_rect(CaptionButton::Minimize), (662, 0, 46, 31));
        assert_eq!(CaptionLayout::new(800, 62, 192, 16).button_width, 92);
        assert_eq!(CaptionLayout::new(800, 47, 144, 12).button_width, 69);
    }

    #[test]
    fn fr22_the_title_bar_band_belongs_to_the_window_and_the_rest_to_the_app() {
        let layout = at_100();
        assert_eq!(layout.hit(400, 20), CaptionHit::Caption);
        assert_eq!(layout.hit(0, 30), CaptionHit::Caption);
        assert_eq!(layout.hit(400, 31), CaptionHit::Client, "the first row below");
        assert_eq!(layout.hit(799, 500), CaptionHit::Client);
        assert_eq!(
            layout.hit(760, 20),
            CaptionHit::Button(CaptionButton::Close)
        );
        assert_eq!(
            layout.hit(753, 20),
            CaptionHit::Button(CaptionButton::Maximize)
        );
        assert_eq!(
            layout.hit(662, 20),
            CaptionHit::Button(CaptionButton::Minimize)
        );
        assert_eq!(layout.hit(661, 20), CaptionHit::Caption);
    }

    #[test]
    fn fr22_the_top_edge_resizes_the_window_unless_it_is_maximized() {
        let layout = at_100();
        assert_eq!(layout.hit(400, 0), CaptionHit::Top);
        assert_eq!(layout.hit(400, 7), CaptionHit::Top);
        assert_eq!(layout.hit(400, 8), CaptionHit::Caption);
        assert_eq!(layout.hit(3, 2), CaptionHit::TopLeft);
        assert_eq!(layout.hit(790, 2), CaptionHit::TopRight);
        assert_eq!(layout.hit(770, 2), CaptionHit::Top, "above the close button");
        let maximized = CaptionLayout::new(800, 31, 96, 0);
        assert_eq!(
            maximized.hit(799, 0),
            CaptionHit::Button(CaptionButton::Close),
            "a maximized window's corner closes it, as in Windows"
        );
    }

    #[test]
    fn fr22_at_every_scale_the_band_is_the_title_bar_height_and_no_more() {
        for (dpi, height, border) in [(96, 31, 8), (144, 46, 11), (192, 62, 16)] {
            let layout = CaptionLayout::new(1200, height, dpi, border);
            for y in border as i32..height as i32 {
                assert_ne!(layout.hit(10, y), CaptionHit::Client, "row {y} at {dpi}");
            }
            assert_eq!(layout.hit(10, height as i32), CaptionHit::Client);
        }
    }

    fn pixel(image: &[u8], layout: &CaptionLayout, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * 3 * layout.button_width + x) * 4) as usize;
        image[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn fr22_the_glyphs_are_drawn_in_the_tone_and_the_rest_is_transparent() {
        let layout = at_100();
        let state = CaptionState {
            active: true,
            ..CaptionState::default()
        };
        for (tone, value) in [(CaptionTone::DarkGlyphs, 0), (CaptionTone::LightGlyphs, 255)] {
            let image = paint_buttons(&layout, 96, &CaptionPalette::of(tone), &state);
            assert_eq!(image.len(), (138 * 31 * 4) as usize);
            // A corner of a button is clear, so the app shows through.
            assert_eq!(pixel(&image, &layout, 1, 1), [0, 0, 0, 0]);
            // The middle of the minimize glyph's stroke is drawn in the tone.
            let stroke = pixel(&image, &layout, 23, 15);
            assert!(stroke[3] > 200, "{tone:?}: {stroke:?}");
            let expected = (u32::from(value) * u32::from(stroke[3]) / 255) as u8;
            assert!(stroke[0].abs_diff(expected) <= 1, "{tone:?}: {stroke:?}");
        }
    }

    #[test]
    fn fr22_close_turns_red_under_the_pointer_with_a_white_glyph() {
        let layout = at_100();
        let state = CaptionState {
            hovered: Some(CaptionButton::Close),
            active: true,
            ..CaptionState::default()
        };
        let image = paint_buttons(
            &layout,
            96,
            &CaptionPalette::of(CaptionTone::DarkGlyphs),
            &state,
        );
        // BGRA of #C42B1C.
        assert_eq!(pixel(&image, &layout, 92 + 2, 2), [0x1C, 0x2B, 0xC4, 0xFF]);
        // The middle of the X is white.
        assert_eq!(pixel(&image, &layout, 92 + 23, 15), [255, 255, 255, 255]);
        // The other buttons have no background.
        assert_eq!(pixel(&image, &layout, 2, 2), [0, 0, 0, 0]);
    }

    #[test]
    fn fr22_a_hovered_button_has_a_faint_background_and_a_pressed_one_another() {
        let layout = at_100();
        let palette = CaptionPalette::of(CaptionTone::LightGlyphs);
        let hovered = CaptionState {
            hovered: Some(CaptionButton::Minimize),
            active: true,
            ..CaptionState::default()
        };
        let pressed = CaptionState {
            pressed: Some(CaptionButton::Minimize),
            ..hovered
        };
        let a = pixel(&paint_buttons(&layout, 96, &palette, &hovered), &layout, 2, 2);
        let b = pixel(&paint_buttons(&layout, 96, &palette, &pressed), &layout, 2, 2);
        assert_eq!(a[3], 0x0F);
        assert_eq!(b[3], 0x0A);
    }

    #[test]
    fn fr22_an_inactive_window_draws_fainter_glyphs() {
        let layout = at_100();
        let palette = CaptionPalette::of(CaptionTone::DarkGlyphs);
        let active = CaptionState {
            active: true,
            ..CaptionState::default()
        };
        let a = pixel(&paint_buttons(&layout, 96, &palette, &active), &layout, 23, 15);
        let i = pixel(
            &paint_buttons(&layout, 96, &palette, &CaptionState::default()),
            &layout,
            23,
            15,
        );
        assert!(i[3] < a[3] / 2, "{i:?} against {a:?}");
    }

    /// The top of a window `width` pixels wide whose app draws a status bar of `bar` (opaque
    /// 0xRRGGBB) under the title bar and a toolbar of `toolbar` below it, with the caption
    /// buttons drawn over it as the window draws them: what a light and a dark app window look
    /// like at the top. Rows of BGRA, `height` rows.
    fn window_top(
        width: u32,
        dpi: u32,
        tone: CaptionTone,
        state: &CaptionState,
        bar: u32,
        toolbar: u32,
    ) -> (Vec<u8>, u32) {
        let caption = scaled(31, dpi);
        let height = caption + scaled(56, dpi);
        let layout = CaptionLayout::new(width, caption, dpi, scaled(8, dpi));
        let buttons = paint_buttons(&layout, dpi, &CaptionPalette::of(tone), state);
        let strip = 3 * layout.button_width;
        let left = layout.buttons_left();
        let mut image = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let rgb = if y < caption { bar } else { toolbar };
                let below = [rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8, 255];
                let column = x as i32 - left;
                let pixel = if y < caption && column >= 0 && (column as u32) < strip {
                    let at = ((y * strip + column as u32) * 4) as usize;
                    let top = &buttons[at..at + 4];
                    let keep = 255 - u32::from(top[3]);
                    let mut mixed = [255u8; 4];
                    for (channel, value) in mixed.iter_mut().take(3).enumerate() {
                        let blended =
                            u32::from(top[channel]) + u32::from(below[channel]) * keep / 255;
                        *value = blended.min(255) as u8;
                    }
                    mixed
                } else {
                    below
                };
                image.extend_from_slice(&pixel);
            }
        }
        (image, height)
    }

    /// A 32-bit BMP file of rows of BGRA, top row first.
    fn bmp(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let size = 54 + pixels.len() as u32;
        let mut file = Vec::with_capacity(size as usize);
        file.extend_from_slice(b"BM");
        file.extend_from_slice(&size.to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&54u32.to_le_bytes());
        file.extend_from_slice(&40u32.to_le_bytes());
        file.extend_from_slice(&(width as i32).to_le_bytes());
        // A negative height: the rows run from the top.
        file.extend_from_slice(&(-(height as i32)).to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&32u16.to_le_bytes());
        file.extend_from_slice(&[0u8; 24]);
        file.extend_from_slice(pixels);
        file
    }

    /// Writes pictures of the top of a light and a dark app window, at 100% and 200%, at rest
    /// and with the pointer over close, into the folder `DSA_CAPTION_PICTURES` names. CI keeps
    /// them with the run; without the variable the pictures are only checked.
    #[test]
    fn fr22_pictures_of_a_light_and_a_dark_app_window() {
        let folder = std::env::var_os("DSA_CAPTION_PICTURES").map(std::path::PathBuf::from);
        let at_rest = CaptionState {
            active: true,
            ..CaptionState::default()
        };
        let close_hovered = CaptionState {
            hovered: Some(CaptionButton::Close),
            ..at_rest
        };
        let apps = [
            // A light app: white status bar, Material light toolbar; dark glyphs.
            ("light", StatusBarAppearance::Light, 0xFF_FFFF, 0xF3_EDF7),
            // A dark app: near-black status bar and toolbar; light glyphs.
            ("dark", StatusBarAppearance::Dark, 0x14_1218, 0x21_1F26),
        ];
        for (name, appearance, bar, toolbar) in apps {
            let tone = caption_tone(appearance, 0);
            for dpi in [96, 192] {
                for (state_name, state) in [("rest", at_rest), ("close-hover", close_hovered)] {
                    let width = scaled(480, dpi);
                    let (image, height) = window_top(width, dpi, tone, &state, bar, toolbar);
                    // The glyphs stand out from the bar: the middle of the minimize glyph
                    // differs from the bar by more than half the range.
                    let layout = CaptionLayout::new(width, scaled(31, dpi), dpi, 0);
                    let (left, _, button, caption) = layout.button_rect(CaptionButton::Minimize);
                    let x = left as u32 + button / 2;
                    let at = (((caption / 2) * width + x) * 4) as usize;
                    let shown = u32::from(image[at]);
                    assert!(
                        shown.abs_diff(bar & 0xff) > 128,
                        "{name} at {dpi}: glyph {shown} on bar {}",
                        bar & 0xff
                    );
                    if let Some(folder) = &folder {
                        std::fs::create_dir_all(folder).unwrap();
                        let percent = dpi * 100 / 96;
                        let path = folder.join(format!("{name}-app-{percent}-{state_name}.bmp"));
                        std::fs::write(path, bmp(width, height, &image)).unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn fr22_the_glyphs_scale_with_the_window() {
        // At 200% the minimize stroke is two pixels thick and twenty long.
        let layout = CaptionLayout::new(800, 62, 192, 16);
        let image = paint_buttons(
            &layout,
            192,
            &CaptionPalette::of(CaptionTone::DarkGlyphs),
            &CaptionState {
                active: true,
                ..CaptionState::default()
            },
        );
        let drawn = |x, y| pixel(&image, &layout, x, y)[3] > 128;
        assert!(drawn(46, 30) && drawn(46, 31));
        assert!(!drawn(46, 28) && !drawn(46, 33));
        assert!(drawn(37, 30) && drawn(54, 30));
        assert!(!drawn(34, 30) && !drawn(58, 30));
    }
}
