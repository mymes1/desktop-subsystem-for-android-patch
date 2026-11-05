//! Frames as the host renderer reads them back: RGBA rows from the top, no padding. A
//! `CAMetalLayer` takes BGRA, so the red and blue channels change places on the way.

/// A frame the renderer read back for one display, owned, BGRA, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedFrame {
    pub display_id: u32,
    pub width: u32,
    pub height: u32,
    /// BGRA, `width * 4` bytes a row.
    pub pixels: Vec<u8>,
}

/// Copies the RGBA pixels of `from` into `to` as BGRA. Only as many whole pixels as both
/// hold are copied.
pub fn swap_red_blue(from: &[u8], to: &mut [u8]) {
    for (to, from) in to.chunks_exact_mut(4).zip(from.chunks_exact(4)) {
        to[0] = from[2];
        to[1] = from[1];
        to[2] = from[0];
        to[3] = from[3];
    }
}

/// The frame in `rgba`, `width` by `height` pixels, as BGRA. `None` when `rgba` is shorter
/// than its size says or the size is empty.
pub fn bgra_from_rgba(display_id: u32, width: u32, height: u32, rgba: &[u8]) -> Option<OwnedFrame> {
    let len = (width as usize)
        .checked_mul(4)?
        .checked_mul(height as usize)?;
    if width == 0 || height == 0 || rgba.len() < len {
        return None;
    }
    let mut pixels = rgba[..len].to_vec();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(OwnedFrame {
        display_id,
        width,
        height,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr3_a_read_back_frame_is_shown_with_its_own_colors() {
        let rgba = [10, 20, 30, 255, 40, 50, 60, 128];
        let frame = bgra_from_rgba(4, 2, 1, &rgba).unwrap();
        assert_eq!(frame.pixels, vec![30, 20, 10, 255, 60, 50, 40, 128]);
        assert_eq!((frame.display_id, frame.width, frame.height), (4, 2, 1));
    }

    #[test]
    fn pr3_a_read_back_row_is_written_with_its_own_colors() {
        let mut to = [0u8; 8];
        swap_red_blue(&[10, 20, 30, 255, 40, 50, 60, 128], &mut to);
        assert_eq!(to, [30, 20, 10, 255, 60, 50, 40, 128]);
    }

    #[test]
    fn nfr5_a_frame_shorter_than_its_size_is_dropped() {
        assert_eq!(bgra_from_rgba(1, 4, 4, &[0; 8]), None);
        assert_eq!(bgra_from_rgba(1, 0, 4, &[]), None);
    }
}
