//! The pictures of a widget window, drawn on the CPU as premultiplied BGRA rows (the layout of
//! a Wayland `ARGB8888` buffer, an X11 32-bit image and a little-endian `CGImage` with its
//! alpha first): the widget's image scaled to its window, its More button, and a widget being
//! edited, lifted over a soft shadow with a border and its handles.
//!
//! The Windows host draws the same parts with its Fluent canvas; the Linux and macOS hosts draw
//! them here, in their desktop's colors ([`Look`]).

use super::geometry::{
    edit_layout, edit_margin, more_button, more_glyph_size, px_of_dip, EDIT_BORDER_DIP,
    EDIT_RADIUS_DIP, LIFT_DIP,
};

/// A color as `0xAARRGGBB`, its alpha not premultiplied.
pub type Argb = u32;

/// An image: `width` by `height` premultiplied BGRA pixels, rows from the top.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Image {
    /// An image of straight RGBA pixels (as PNG decodes them), premultiplied into BGRA.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Image {
        let mut pixels = Vec::with_capacity(rgba.len());
        for pixel in rgba.chunks_exact(4) {
            let alpha = u32::from(pixel[3]);
            let times = |channel: u8| ((u32::from(channel) * alpha + 127) / 255) as u8;
            pixels.extend_from_slice(&[
                times(pixel[2]),
                times(pixel[1]),
                times(pixel[0]),
                pixel[3],
            ]);
        }
        Image {
            width,
            height,
            pixels,
        }
    }

    /// The image scaled to `width` by `height`, each pixel the average of the pixels it
    /// covers, or the nearest one when it grows.
    pub fn scaled(&self, width: u32, height: u32) -> Image {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.width, self.height) {
            return self.clone();
        }
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        if self.width == 0
            || self.height == 0
            || self.pixels.len() < (self.width * self.height * 4) as usize
        {
            return Image {
                width,
                height,
                pixels,
            };
        }
        let (sw, sh) = (self.width as usize, self.height as usize);
        for y in 0..height as usize {
            let top = y * sh / height as usize;
            let bottom = ((y + 1) * sh).div_ceil(height as usize).clamp(top + 1, sh);
            for x in 0..width as usize {
                let left = x * sw / width as usize;
                let right = ((x + 1) * sw).div_ceil(width as usize).clamp(left + 1, sw);
                let mut sum = [0u32; 4];
                for sy in top..bottom {
                    for sx in left..right {
                        let at = (sy * sw + sx) * 4;
                        for (total, value) in sum.iter_mut().zip(&self.pixels[at..at + 4]) {
                            *total += u32::from(*value);
                        }
                    }
                }
                let count = ((bottom - top) * (right - left)) as u32;
                let at = (y * width as usize + x) * 4;
                for (out, total) in pixels[at..at + 4].iter_mut().zip(sum) {
                    *out = ((total + count / 2) / count) as u8;
                }
            }
        }
        Image {
            width,
            height,
            pixels,
        }
    }
}

/// The colors of a widget's More button and of a widget being edited, in the desktop's own
/// style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    /// The button's fill, and the overlays while the pointer is on it and presses it.
    pub button_fill: Argb,
    pub button_hover: Argb,
    pub button_pressed: Argb,
    /// The button's hairline edge and its glyph.
    pub button_border: Argb,
    pub glyph: Argb,
    /// The border and the handles' rims of a widget being edited: the desktop's accent.
    pub accent: Argb,
    /// The inside of the handles.
    pub handle: Argb,
    /// The shadow under the lifted widget, at its darkest.
    pub shadow: Argb,
}

impl Look {
    /// The look on a light or a dark desktop, with `accent` its accent color.
    pub fn new(dark: bool, accent: Argb) -> Look {
        if dark {
            Look {
                button_fill: 0xB32D_2D2D,
                button_hover: 0x14FF_FFFF,
                button_pressed: 0x0AFF_FFFF,
                button_border: 0x24FF_FFFF,
                glyph: 0xFFFF_FFFF,
                accent,
                handle: 0xFF2D_2D2D,
                shadow: 0x5C00_0000,
            }
        } else {
            Look {
                button_fill: 0xB3FF_FFFF,
                button_hover: 0x0A00_0000,
                button_pressed: 0x0600_0000,
                button_border: 0x24000000,
                glyph: 0xE400_0000,
                accent,
                handle: 0xFFFF_FFFF,
                shadow: 0x3800_0000,
            }
        }
    }
}

/// `color` with its alpha scaled by `level` (0 to 1).
pub fn faded(color: Argb, level: f32) -> Argb {
    let alpha = (((color >> 24) as f32) * level.clamp(0.0, 1.0)).round() as u32;
    (alpha.min(255) << 24) | (color & 0x00FF_FFFF)
}

/// A canvas over premultiplied BGRA rows.
pub struct Canvas<'a> {
    pub width: usize,
    pub height: usize,
    pub pixels: &'a mut [u8],
}

/// How far the point (`px`, `py`) is outside the rectangle at `x`, `y` of `w` by `h` with its
/// corners rounded to `r`: negative inside.
fn rounded_distance(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let qx = (px - cx).abs() - (w / 2.0 - r);
    let qy = (py - cy).abs() - (h / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

impl Canvas<'_> {
    /// Lays `color` over the pixel at `x`, `y`, covering `coverage` (0 to 1) of it.
    fn blend(&mut self, x: usize, y: usize, color: Argb, coverage: f32) {
        let alpha = (color >> 24) as f32 / 255.0 * coverage.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return;
        }
        let at = (y * self.width + x) * 4;
        let Some(pixel) = self.pixels.get_mut(at..at + 4) else {
            return;
        };
        let channel = |shift: u32| ((color >> shift) & 0xff) as f32 * alpha;
        let top = [channel(0), channel(8), channel(16), alpha * 255.0];
        let keep = 1.0 - alpha;
        for (out, over) in pixel.iter_mut().zip(top) {
            *out = (f32::from(*out) * keep + over).round().clamp(0.0, 255.0) as u8;
        }
    }

    /// The pixels a shape at `x`, `y` of `w` by `h`, grown by `grow`, may touch.
    fn span(&self, x: f32, y: f32, w: f32, h: f32, grow: f32) -> (usize, usize, usize, usize) {
        let left = (x - grow).floor().max(0.0) as usize;
        let top = (y - grow).floor().max(0.0) as usize;
        let right = ((x + w + grow).ceil().max(0.0) as usize).min(self.width);
        let bottom = ((y + h + grow).ceil().max(0.0) as usize).min(self.height);
        (left, top, right, bottom)
    }

    /// Fills the rectangle at `x`, `y` of `w` by `h` with its corners rounded to `radius`,
    /// its edge smoothed over a pixel.
    pub fn fill_rounded(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Argb) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (left, top, right, bottom) = self.span(x, y, w, h, 1.0);
        for py in top..bottom {
            for px in left..right {
                let d = rounded_distance(px as f32 + 0.5, py as f32 + 0.5, x, y, w, h, radius);
                let coverage = (0.5 - d).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    self.blend(px, py, color, coverage);
                }
            }
        }
    }

    /// Strokes the edge of the rectangle `rect` (left, top, width and height) with its corners
    /// rounded to `radius`, `thickness` wide, inside it.
    pub fn stroke_rounded(
        &mut self,
        rect: (f32, f32, f32, f32),
        radius: f32,
        thickness: f32,
        color: Argb,
    ) {
        let (x, y, w, h) = rect;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (left, top, right, bottom) = self.span(x, y, w, h, 1.0);
        let (ix, iy, iw, ih) = (
            x + thickness,
            y + thickness,
            w - 2.0 * thickness,
            h - 2.0 * thickness,
        );
        let inner_radius = (radius - thickness).max(0.0);
        for py in top..bottom {
            for px in left..right {
                let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
                let outer = (0.5 - rounded_distance(cx, cy, x, y, w, h, radius)).clamp(0.0, 1.0);
                let inner = if iw > 0.0 && ih > 0.0 {
                    (0.5 - rounded_distance(cx, cy, ix, iy, iw, ih, inner_radius)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let coverage = (outer - inner).max(0.0);
                if coverage > 0.0 {
                    self.blend(px, py, color, coverage);
                }
            }
        }
    }

    /// Lays `image` over the canvas with its top left corner at `x`, `y`.
    pub fn draw_image(&mut self, image: &Image, x: i32, y: i32) {
        let (iw, ih) = (image.width as usize, image.height as usize);
        for row in 0..ih {
            let ty = y + row as i32;
            if ty < 0 || ty as usize >= self.height {
                continue;
            }
            for column in 0..iw {
                let tx = x + column as i32;
                if tx < 0 || tx as usize >= self.width {
                    continue;
                }
                let from = (row * iw + column) * 4;
                let Some(source) = image.pixels.get(from..from + 4) else {
                    continue;
                };
                let to = (ty as usize * self.width + tx as usize) * 4;
                let keep = 255 - u32::from(source[3]);
                for (out, over) in self.pixels[to..to + 4].iter_mut().zip(source) {
                    let under = u32::from(*out) * keep / 255;
                    *out = (u32::from(*over) + under).min(255) as u8;
                }
            }
        }
    }
}

/// The state the More button is drawn in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ButtonState {
    /// How much of it shows, from 0 to 1, as it fades.
    pub level: f32,
    pub hover: bool,
    pub pressed: bool,
}

/// Draws the More button of a window of `width` by `height` pixels on a monitor of `dpi`
/// over its `pixels`: a subtle fill with a hairline edge, lighter or darker while the pointer
/// is on it or presses it, and three dots, the "More" glyph.
pub fn draw_more_button(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    dpi: u32,
    state: &ButtonState,
    look: &Look,
) {
    if state.level <= 0.0 {
        return;
    }
    let circle = more_button(width as i32, height as i32, dpi);
    let mut canvas = Canvas {
        width,
        height,
        pixels,
    };
    let (r, d) = (circle.radius, circle.radius * 2.0);
    let (x, y) = (circle.cx - r, circle.cy - r);
    canvas.fill_rounded(x, y, d, d, r, faded(look.button_fill, state.level));
    let overlay = if state.pressed {
        Some(look.button_pressed)
    } else if state.hover {
        Some(look.button_hover)
    } else {
        None
    };
    if let Some(overlay) = overlay {
        canvas.fill_rounded(x, y, d, d, r, faded(overlay, state.level));
    }
    let hairline = px_of_dip(1, dpi).max(1) as f32;
    let border = faded(look.button_border, state.level);
    canvas.stroke_rounded((x, y, d, d), r, hairline, border);
    let glyph = more_glyph_size(width as i32, height as i32, dpi) as f32;
    let dot = (glyph / 8.0).max(1.0);
    let step = glyph * 0.35;
    for at in [-1.0, 0.0, 1.0] {
        let cx = circle.cx + at * step;
        canvas.fill_rounded(
            cx - dot,
            circle.cy - dot,
            2.0 * dot,
            2.0 * dot,
            dot,
            faded(look.glyph, state.level),
        );
    }
}

/// The picture of a widget being edited, `width` by `height` premultiplied BGRA pixels on a
/// monitor of `dpi`: a soft shadow under the lifted widget, `lifted` (its image at the lifted
/// size) over it, a border in the accent color and a handle on each edge `resize_mode` lets
/// it be resized along. Every pixel of the widget stays just visible enough to be clicked;
/// the room around it is clear but for the shadow.
pub fn draw_edit(
    lifted: &Image,
    width: usize,
    height: usize,
    dpi: u32,
    resize_mode: u32,
    look: &Look,
) -> Vec<u8> {
    let layout = edit_layout(width as i32, height as i32, dpi, resize_mode);
    let mut pixels = vec![0u8; width * height * 4];
    let mut canvas = Canvas {
        width,
        height,
        pixels: &mut pixels,
    };
    let (lx, ly, lw, lh) = layout.lifted;
    let radius = px_of_dip(EDIT_RADIUS_DIP, dpi) as f32;
    // The shadow: layers that spread from the widget into the room around it, each adding a
    // little, so it is darkest under the widget and fades out, a little below it as a raised
    // surface's is.
    if look.shadow >> 24 != 0 {
        const LAYERS: i32 = 6;
        let room = (edit_margin(dpi) - px_of_dip(LIFT_DIP, dpi)).max(1) as f32;
        let drop = px_of_dip(2, dpi) as f32;
        let alpha = ((look.shadow >> 24) as f32 / LAYERS as f32)
            .round()
            .max(1.0) as u32;
        let layer = (alpha.min(255) << 24) | (look.shadow & 0x00FF_FFFF);
        for step in 1..=LAYERS {
            let spread = room * step as f32 / LAYERS as f32;
            canvas.fill_rounded(
                lx as f32 - spread,
                ly as f32 - spread + drop,
                lw as f32 + 2.0 * spread,
                lh as f32 + 2.0 * spread,
                radius + spread,
                layer,
            );
        }
    }
    canvas.draw_image(lifted, lx, ly);
    // A widget's own clear pixels stay clickable.
    for row in ly.max(0)..(ly + lh).min(height as i32) {
        for column in lx.max(0)..(lx + lw).min(width as i32) {
            let at = (row as usize * width + column as usize) * 4 + 3;
            if canvas.pixels[at] == 0 {
                canvas.pixels[at] = 1;
            }
        }
    }
    let border = px_of_dip(EDIT_BORDER_DIP, dpi).max(1) as f32;
    let (x, y, w, h) = (lx as f32, ly as f32, lw as f32, lh as f32);
    canvas.stroke_rounded((x, y, w, h), radius, border, look.accent);
    let rim = (px_of_dip(3, dpi) as f32 / 2.0).max(1.0);
    for (_, handle) in &layout.handles {
        let (r, d) = (handle.radius, handle.radius * 2.0);
        let (left, top) = (handle.cx - r, handle.cy - r);
        canvas.fill_rounded(left, top, d, d, r, look.handle);
        canvas.stroke_rounded((left, top, d, d), r, rim, look.accent);
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::{RESIZE_HORIZONTAL, RESIZE_VERTICAL};

    fn alpha(pixels: &[u8], width: usize, x: usize, y: usize) -> u8 {
        pixels[(y * width + x) * 4 + 3]
    }

    #[test]
    fn fr12_a_picture_from_android_is_premultiplied_and_scaled_to_its_window() {
        // Half see-through red, straight RGBA, becomes premultiplied BGRA.
        let image = Image::from_rgba(1, 1, &[255, 0, 0, 128]);
        assert_eq!(image.pixels, vec![0, 0, 128, 128]);
        let wide = Image::from_rgba(2, 1, &[255, 255, 255, 255, 0, 0, 0, 0]);
        let half = wide.scaled(1, 1);
        assert_eq!(half.pixels, vec![128, 128, 128, 128]);
        let grown = image.scaled(3, 2);
        assert_eq!((grown.width, grown.height), (3, 2));
        assert!(grown.pixels.chunks_exact(4).all(|p| p == [0, 0, 128, 128]));
    }

    #[test]
    fn fr12_a_widget_being_edited_is_drawn_lifted_over_a_shadow_with_its_border_and_handles() {
        let dpi = 96;
        let look = Look::new(false, 0xFF35_84E4);
        let margin = edit_margin(dpi) as usize;
        let (width, height) = (180 + 2 * margin, 110 + 2 * margin);
        let layout = edit_layout(width as i32, height as i32, dpi, RESIZE_HORIZONTAL);
        let (_, _, lw, lh) = layout.lifted;
        let lifted = Image {
            width: lw as u32,
            height: lh as u32,
            pixels: vec![0; (lw * lh * 4) as usize],
        };
        let pixels = draw_edit(&lifted, width, height, dpi, RESIZE_HORIZONTAL, &look);
        // The corners of the room are clear, the shadow is under the widget, the widget's own
        // clear pixels can still be clicked, and the left handle is drawn in the accent.
        assert_eq!(alpha(&pixels, width, 0, 0), 0);
        assert!(alpha(&pixels, width, width / 2, height - 3) > 0);
        assert!(alpha(&pixels, width, width / 2, height / 2) >= 1);
        let (hit, handle) = layout.handles[0];
        assert_eq!(hit, super::super::geometry::Hit::Left);
        let at = (handle.cy as usize * width + handle.cx as usize) * 4;
        assert_eq!(pixels[at + 3], 255);
        // Only the edges it resizes along have handles.
        assert_eq!(layout.handles.len(), 2);
        let both = edit_layout(
            width as i32,
            height as i32,
            dpi,
            RESIZE_HORIZONTAL | RESIZE_VERTICAL,
        );
        assert_eq!(both.handles.len(), 4);
    }

    #[test]
    fn fr12_the_more_button_is_drawn_with_a_fill_a_hairline_and_three_dots() {
        let (width, height) = (180usize, 110usize);
        let mut pixels = vec![0u8; width * height * 4];
        let look = Look::new(true, 0xFF35_84E4);
        let hidden = ButtonState {
            level: 0.0,
            hover: false,
            pressed: false,
        };
        draw_more_button(&mut pixels, width, height, 96, &hidden, &look);
        assert!(pixels.iter().all(|&byte| byte == 0));
        let shown = ButtonState {
            level: 1.0,
            ..hidden
        };
        draw_more_button(&mut pixels, width, height, 96, &shown, &look);
        let circle = more_button(width as i32, height as i32, 96);
        // The middle dot is the glyph's color, white on a dark desktop.
        let at = (circle.cy as usize * width + circle.cx as usize) * 4;
        assert!(pixels[at] > 200 && pixels[at + 3] > 200);
        // Outside the circle nothing is drawn.
        assert_eq!(alpha(&pixels, width, 0, 0), 0);
    }
}
