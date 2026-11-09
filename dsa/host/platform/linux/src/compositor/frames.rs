//! The task frames the host renderer reads back, on their way to the windows.
//!
//! The renderer hands each display's newest frame over as RGBA rows; a Wayland shared memory
//! buffer takes `XRGB8888` (blue, green, red, then a byte the compositor ignores, in memory).
//! Only the newest frame of each display is kept until its window draws, so a window that
//! draws slowly shows the latest frame rather than falling behind.

use std::collections::BTreeMap;

/// One frame of a task, ready for a shared memory buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// `width * 4` bytes a row, `XRGB8888`.
    pub pixels: Vec<u8>,
}

/// The largest side of a frame taken, in pixels; a larger one comes from a broken guest.
pub const MAX_SIDE: u32 = 16384;

impl Frame {
    /// The frame `rgba` holds: `width` x `height` pixels, four bytes each, rows from the
    /// top. `None` when the sizes do not match the pixels.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Frame> {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return None;
        }
        let len = width as usize * height as usize * 4;
        if rgba.len() < len {
            return None;
        }
        Some(Frame {
            width,
            height,
            pixels: rgba_to_xrgb(&rgba[..len]),
        })
    }

    /// A frame of one color, `0xRRGGBB`, for a window before its task has drawn.
    pub fn solid(width: u32, height: u32, rgb: u32) -> Frame {
        let width = width.clamp(1, MAX_SIDE);
        let height = height.clamp(1, MAX_SIDE);
        let pixel = [rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8, 0xff];
        Frame {
            width,
            height,
            pixels: pixel.repeat(width as usize * height as usize),
        }
    }

    /// The frame drawn `to` pixels large, each pixel taken from the nearest of the frame's: a
    /// task that keeps its density on a monitor of another scale, in a window that cannot have
    /// the desktop scale it. The same frame when it is that size already.
    pub fn scaled(&self, to: (u32, u32)) -> Frame {
        let (width, height) = (to.0.clamp(1, MAX_SIDE), to.1.clamp(1, MAX_SIDE));
        if (width, height) == (self.width, self.height) {
            return self.clone();
        }
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        let columns: Vec<usize> = (0..width)
            .map(|x| {
                let from = (u64::from(x) * u64::from(self.width) / u64::from(width)) as usize;
                from.min(self.width.max(1) as usize - 1) * 4
            })
            .collect();
        for y in 0..height {
            let row = (u64::from(y) * u64::from(self.height) / u64::from(height)) as usize;
            let row = row.min(self.height.max(1) as usize - 1) * self.width as usize * 4;
            for column in &columns {
                let at = row + column;
                pixels.extend_from_slice(self.pixels.get(at..at + 4).unwrap_or(&[0, 0, 0, 0xff]));
            }
        }
        Frame {
            width,
            height,
            pixels,
        }
    }

    /// Draws `overlay` (premultiplied BGRA rows, `size` pixels) over the frame with its top
    /// left corner at `at`, clipped to the frame: the title bar's buttons over the app.
    pub fn blend(&mut self, overlay: &[u8], size: (u32, u32), at: (i32, i32)) {
        let (width, height) = (self.width as i32, self.height as i32);
        for row in 0..size.1 as i32 {
            let y = at.1 + row;
            if !(0..height).contains(&y) {
                continue;
            }
            for column in 0..size.0 as i32 {
                let x = at.0 + column;
                if !(0..width).contains(&x) {
                    continue;
                }
                let from = ((row * size.0 as i32 + column) * 4) as usize;
                let Some(source) = overlay.get(from..from + 4) else {
                    return;
                };
                let alpha = u32::from(source[3]);
                if alpha == 0 {
                    continue;
                }
                let to = ((y * width + x) * 4) as usize;
                let target = &mut self.pixels[to..to + 4];
                for (to, from) in target[..3].iter_mut().zip(&source[..3]) {
                    let under = u32::from(*to) * (255 - alpha) / 255;
                    *to = (u32::from(*from) + under).min(255) as u8;
                }
            }
        }
    }
}

/// A frame the renderer left in one of a display's GPU images (a dmabuf): which image, and
/// the top-left region of it the frame fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuFrame {
    pub slot: u32,
    pub width: u32,
    pub height: u32,
}

/// One GPU image of a display, as the renderer exported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuImage {
    pub display_id: u32,
    pub slot: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub offset: u32,
    pub fourcc: u32,
    pub modifier: u64,
}

/// The linear layout's modifier, the only one the CPU can read as rows.
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;
/// The modifier of an image whose layout the driver chose without naming it.
pub const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// The four-character code of a format, as DRM numbers it.
pub const fn fourcc(code: &[u8; 4]) -> u32 {
    (code[0] as u32) | ((code[1] as u32) << 8) | ((code[2] as u32) << 16) | ((code[3] as u32) << 24)
}

/// RGBA bytes as `XRGB8888` (blue, green, red, then an opaque byte, in memory).
pub fn rgba_to_xrgb(rgba: &[u8]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(rgba.len());
    for from in rgba.chunks_exact(4) {
        pixels.extend_from_slice(&[from[2], from[1], from[0], 0xff]);
    }
    pixels
}

/// The rows of a GPU frame the title bar's band fill samples, from the top: the middle of a
/// `band` pixels high band and the row just under it. `None` when the frame, `height` rows,
/// has nothing under the band or there is no band. Only these two rows are read back, so
/// filling the band never reads a whole frame.
pub fn band_probe_rows(band: u32, height: u32) -> Option<[u32; 2]> {
    (band > 0 && band < height).then_some([band / 2, band])
}

/// The rows of a frame `height` rows high that a screenshot takes: every row under the title
/// bar's `band`, from the top. Empty when nothing is under it.
pub fn app_rows(band: u32, height: u32) -> std::ops::Range<u32> {
    band.min(height)..height
}

/// `rows` as runs of rows next to each other: (first row, how many), in order.
pub fn row_runs(rows: &[u32]) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &row in rows {
        match runs.last_mut() {
            Some((top, count)) if *top + *count == row => *count += 1,
            _ => runs.push((row, 1)),
        }
    }
    runs
}

/// What a display has to show next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    /// A frame read back to memory.
    Pixels(Frame),
    /// A frame in a GPU image the window already holds.
    Gpu(GpuFrame),
}

/// The newest frame of each display, waiting for its window.
#[derive(Debug, Default)]
pub struct LatestFrames {
    frames: BTreeMap<u32, Shown>,
}

impl LatestFrames {
    pub fn new() -> LatestFrames {
        LatestFrames::default()
    }

    /// `display_id` has a new frame; one that was still waiting is dropped.
    pub fn put(&mut self, display_id: u32, frame: Frame) {
        self.frames.insert(display_id, Shown::Pixels(frame));
    }

    /// `display_id` has a new frame in a GPU image.
    pub fn put_gpu(&mut self, display_id: u32, frame: GpuFrame) {
        self.frames.insert(display_id, Shown::Gpu(frame));
    }

    /// Every frame waiting, by display, leaving none.
    pub fn take(&mut self) -> BTreeMap<u32, Shown> {
        std::mem::take(&mut self.frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr3_a_frame_reaches_the_window_in_the_window_systems_pixel_order() {
        let rgba = [0x11, 0x22, 0x33, 0x80, 0xaa, 0xbb, 0xcc, 0xff];
        let frame = Frame::from_rgba(2, 1, &rgba).unwrap();
        assert_eq!(
            frame.pixels,
            vec![0x33, 0x22, 0x11, 0xff, 0xcc, 0xbb, 0xaa, 0xff]
        );
    }

    #[test]
    fn fr4_a_frame_is_drawn_at_its_windows_scale() {
        // Two pixels side by side, shown at two window pixels per task pixel.
        let frame = Frame {
            width: 2,
            height: 1,
            pixels: vec![1, 1, 1, 0xff, 2, 2, 2, 0xff],
        };
        let twice = frame.scaled((4, 2));
        assert_eq!((twice.width, twice.height), (4, 2));
        let row = [1, 1, 1, 0xff, 1, 1, 1, 0xff, 2, 2, 2, 0xff, 2, 2, 2, 0xff];
        assert_eq!(twice.pixels, [row, row].concat());
        assert_eq!(frame.scaled((2, 1)), frame);
        assert_eq!(twice.scaled((2, 1)), frame);
    }

    #[test]
    fn fr3_a_frame_whose_size_does_not_match_its_pixels_is_dropped() {
        assert_eq!(Frame::from_rgba(2, 2, &[0; 8]), None);
        assert_eq!(Frame::from_rgba(0, 2, &[]), None);
        assert_eq!(Frame::from_rgba(MAX_SIDE + 1, 1, &[0; 4]), None);
    }

    #[test]
    fn fr4_only_the_newest_frame_of_a_display_waits_for_its_window() {
        let mut frames = LatestFrames::new();
        frames.put(3, Frame::solid(1, 1, 0x000000));
        frames.put(3, Frame::solid(2, 2, 0xffffff));
        frames.put(5, Frame::solid(1, 1, 0x00ff00));
        frames.put_gpu(
            7,
            GpuFrame {
                slot: 1,
                width: 4,
                height: 4,
            },
        );
        let taken = frames.take();
        assert_eq!(taken.len(), 3);
        assert!(matches!(&taken[&3], Shown::Pixels(frame) if frame.width == 2));
        assert!(matches!(taken[&7], Shown::Gpu(GpuFrame { slot: 1, .. })));
        assert!(frames.take().is_empty());
    }

    #[test]
    fn fr22_a_gpu_frame_reads_back_only_the_rows_the_band_fill_samples() {
        // A 32 px band over a 600 px frame: its middle row and the row under it.
        assert_eq!(band_probe_rows(32, 600), Some([16, 32]));
        assert_eq!(band_probe_rows(64, 1200), Some([32, 64]));
        // No band (full screen), or nothing under it, reads nothing.
        assert_eq!(band_probe_rows(0, 600), None);
        assert_eq!(band_probe_rows(600, 600), None);
        assert_eq!(band_probe_rows(32, 20), None);
        // The two rows are read apart, the rows of a screenshot in one run.
        assert_eq!(row_runs(&[16, 32]), vec![(16, 1), (32, 1)]);
        assert_eq!(row_runs(&[32, 33, 34, 35]), vec![(32, 4)]);
        assert_eq!(row_runs(&[1, 2, 5, 6, 7]), vec![(1, 2), (5, 3)]);
        assert!(row_runs(&[]).is_empty());
    }

    #[test]
    fn fr22_a_screenshot_of_a_gpu_frame_reads_every_row_under_the_band() {
        assert_eq!(app_rows(32, 600), 32..600);
        assert_eq!(app_rows(0, 600), 0..600);
        assert!(app_rows(600, 600).is_empty());
        assert!(app_rows(700, 600).is_empty());
        let rows: Vec<u32> = app_rows(32, 40).collect();
        assert_eq!(row_runs(&rows), vec![(32, 8)]);
        // What the GPU reads comes as RGBA, and the windows take XRGB8888.
        assert_eq!(
            rgba_to_xrgb(&[0x11, 0x22, 0x33, 0x00]),
            vec![0x33, 0x22, 0x11, 0xff]
        );
        assert_eq!(fourcc(b"XR24"), 0x3432_5258);
    }

    #[test]
    fn fr22_the_title_bar_buttons_blend_over_the_app_and_clip_to_the_frame() {
        let mut frame = Frame::solid(2, 1, 0x000000);
        // An opaque white pixel and a transparent one, drawn from x = 1: only the first
        // lands, on the second pixel of the frame.
        let overlay = [0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0];
        frame.blend(&overlay, (2, 1), (1, 0));
        assert_eq!(frame.pixels, vec![0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff]);
    }
}
