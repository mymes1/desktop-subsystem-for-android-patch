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
        let mut pixels = vec![0u8; len];
        for (to, from) in pixels.chunks_exact_mut(4).zip(rgba[..len].chunks_exact(4)) {
            to[0] = from[2];
            to[1] = from[1];
            to[2] = from[0];
            to[3] = 0xff;
        }
        Some(Frame {
            width,
            height,
            pixels,
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
        assert_eq!(frame.pixels, vec![0x33, 0x22, 0x11, 0xff, 0xcc, 0xbb, 0xaa, 0xff]);
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
    fn fr22_the_title_bar_buttons_blend_over_the_app_and_clip_to_the_frame() {
        let mut frame = Frame::solid(2, 1, 0x000000);
        // An opaque white pixel and a transparent one, drawn from x = 1: only the first
        // lands, on the second pixel of the frame.
        let overlay = [0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0];
        frame.blend(&overlay, (2, 1), (1, 0));
        assert_eq!(frame.pixels, vec![0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff]);
    }
}
