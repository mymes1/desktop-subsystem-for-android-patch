//! How a task window's buffers are sized, so that most steps of a resize do not remake them.
//!
//! Every buffer on a task's frame path (the guest's ColorBuffers, the window manager's shared
//! texture and the window host's swapchain) follows the size the guest gives its ColorBuffers,
//! which is the task's size rounded up in steps, and each frame fills only its top-left
//! region. A frame carries its content size next to the buffer, and the window shows only that
//! region. Making a buffer is the slow part of a resize (tens of milliseconds for a
//! swapchain's `ResizeBuffers` alone), so a size step that fits the buffers makes none.

/// The part of a buffer of `buffer` pixels that a frame of `content` pixels fills: the
/// content clipped to the buffer. A content size of 0 in either direction comes from a
/// sender that does not say it, whose frames fill the whole buffer.
pub fn content_region(content: (u32, u32), buffer: (u32, u32)) -> (u32, u32) {
    if content.0 == 0 || content.1 == 0 {
        return buffer;
    }
    (content.0.min(buffer.0), content.1.min(buffer.1))
}

/// What a window's swapchain needs before it shows a frame from a texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapchainChange {
    /// There is none yet: make one of this size.
    Create((u32, u32)),
    /// Its buffers are the wrong size for the texture: resize them to this.
    Resize((u32, u32)),
    /// It fits: only the region shown changes.
    Keep,
}

/// How many buffers a window's swapchain has. With two, the buffer a frame is copied into is
/// the one the desktop compositor is still showing, and the copy waits for its next vertical
/// blank; the renderer waits for the copy. The third is always free.
pub fn swapchain_buffers() -> u32 {
    3
}

/// What a swapchain whose buffers are `current` pixels (`None` when there is none) needs to
/// show frames from a texture of `texture` pixels. The buffers follow the texture, which is
/// the largest a frame can be, never a frame's own size, so a window resize changes nothing
/// here.
pub fn swapchain_change(current: Option<(u32, u32)>, texture: (u32, u32)) -> SwapchainChange {
    match current {
        None => SwapchainChange::Create(texture),
        Some(size) if size != texture => SwapchainChange::Resize(texture),
        Some(_) => SwapchainChange::Keep,
    }
}

/// Whether a shared texture of `current` pixels can take the frames of a ColorBuffer of
/// `wanted` pixels. The texture is remade only when the ColorBuffer itself changes size,
/// which the guest does only when the task passes a step of its size.
pub fn texture_fits(current: (u32, u32), wanted: (u32, u32)) -> bool {
    current == wanted
}

/// How a frame sits in its window: the visual's scale, the part of the client area the frame
/// covers, and whether the rest needs filling.
///
/// A frame is never scaled to the window, and never more in one direction than the other. Until
/// a frame of the window's size arrives (while the user drags an edge, or while the app lays
/// itself out again), the window shows the last frame pixel for pixel from its top left
/// corner: a window that grew shows the app's background colour where the frame does not
/// reach, and a window that shrank crops the frame rather than squashing it. A stretched frame
/// looked distorted for as long as an app took to draw at the new size, which is long for an
/// app such as Settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The visual's scale. DirectComposition stretches a swapchain's source region over the
    /// whole of its buffers, so this brings the frame back to its own size.
    pub scale: (f32, f32),
    /// The part of the client area the frame covers, from its top left corner: the frame,
    /// cropped to the window.
    pub covered: (u32, u32),
    /// Whether some of the client area is outside the frame and shows the fill colour.
    pub fill: bool,
}

/// Where a frame of `frame` pixels goes in a client area of `view` pixels when the swapchain's
/// buffers are `buffers` pixels: at 1:1, from the top left corner.
pub fn frame_placement(buffers: (u32, u32), frame: (u32, u32), view: (u32, u32)) -> Placement {
    let scale = |frame: u32, buffer: u32| {
        if frame == 0 || buffer == 0 {
            1.0
        } else {
            frame as f32 / buffer as f32
        }
    };
    Placement {
        scale: (scale(frame.0, buffers.0), scale(frame.1, buffers.1)),
        covered: (frame.0.min(view.0), frame.1.min(view.1)),
        fill: frame.0 < view.0 || frame.1 < view.1,
    }
}

/// How many pixels are sampled along each of a frame's right and bottom edges to find the
/// colour that fills the part of a window the frame does not reach.
pub const EDGE_SAMPLES: u32 = 8;

/// The pixels of a frame of `region` pixels whose colour fills the rest of a larger window:
/// [`EDGE_SAMPLES`] down its last column, then as many along its last row. Those are the
/// pixels next to the strip being filled, and in most apps they are the window's background.
pub fn edge_points((width, height): (u32, u32)) -> Vec<(u32, u32)> {
    let (width, height) = (width.max(1), height.max(1));
    let along = |size: u32, i: u32| {
        ((2 * u64::from(i) + 1) * u64::from(size) / (2 * u64::from(EDGE_SAMPLES))) as u32
    };
    let column = (0..EDGE_SAMPLES).map(move |i| (width - 1, along(height, i)));
    let row = (0..EDGE_SAMPLES).map(move |i| (along(width, i), height - 1));
    column.chain(row).collect()
}

/// The colour that fills the part of a window its frame does not cover, from the frame's
/// `edge` pixels: the one most of them have (the first of those tied), opaque. `None` when
/// there are none.
pub fn fill_colour(edge: &[[u8; 4]]) -> Option<[u8; 4]> {
    let mut best: Option<([u8; 4], usize)> = None;
    for pixel in edge {
        let count = edge.iter().filter(|other| *other == pixel).count();
        match best {
            Some((_, most)) if most >= count => {}
            _ => best = Some((*pixel, count)),
        }
    }
    best.map(|([red, green, blue, _], _)| [red, green, blue, 255])
}

/// The fill colour before any frame was sampled: Android's light window background.
pub const DEFAULT_FILL: [u8; 4] = [255, 255, 255, 255];

/// How many pixels a side the resize trace samples of a frame to see if it is all one colour.
pub const SAMPLE_GRID: u32 = 8;

/// The pixels of a frame of `region` pixels the trace samples: the centres of an even grid,
/// row by row, [`SAMPLE_GRID`] to a side.
pub fn sample_points((width, height): (u32, u32)) -> Vec<(u32, u32)> {
    let along = |size: u32, i: u32| {
        ((2 * u64::from(i) + 1) * u64::from(size) / (2 * u64::from(SAMPLE_GRID))) as u32
    };
    (0..SAMPLE_GRID)
        .flat_map(|row| {
            (0..SAMPLE_GRID).map(move |column| (along(width, column), along(height, row)))
        })
        .collect()
}

/// The colour every one of `pixels` has, or `None` when they differ. An app's frame is hardly
/// ever one colour across a whole grid, and a frame nothing was drawn into always is.
pub fn uniform_colour(pixels: &[[u8; 4]]) -> Option<[u8; 4]> {
    let first = *pixels.first()?;
    pixels.iter().all(|pixel| *pixel == first).then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr4_the_samples_of_a_frame_spread_over_it_and_stay_inside() {
        let points = sample_points((900, 747));
        assert_eq!(points.len(), (SAMPLE_GRID * SAMPLE_GRID) as usize);
        assert!(points.iter().all(|&(x, y)| x < 900 && y < 747));
        assert!(points.iter().any(|&(x, _)| x < 100));
        assert!(points.iter().any(|&(x, _)| x > 800));
        assert!(points.iter().any(|&(_, y)| y < 100));
        assert!(points.iter().any(|&(_, y)| y > 650));
        // The smallest frame still names pixels it has.
        assert!(sample_points((1, 1)).iter().all(|&point| point == (0, 0)));
    }

    #[test]
    fn fr4_a_frame_all_one_colour_is_told_apart_from_a_drawn_one() {
        let black = [0, 0, 0, 255];
        assert_eq!(uniform_colour(&[black; 64]), Some(black));
        let mut drawn = [black; 64];
        drawn[63] = [255, 255, 255, 255];
        assert_eq!(uniform_colour(&drawn), None);
        assert_eq!(uniform_colour(&[]), None);
    }

    #[test]
    fn fr5_a_frame_never_waits_for_the_buffer_the_desktop_is_showing() {
        assert!(swapchain_buffers() >= 3);
    }

    /// How many window pixels one frame pixel takes, in each direction.
    fn on_screen(placement: Placement, buffers: (u32, u32), frame: (u32, u32)) -> (f32, f32) {
        (
            placement.scale.0 * buffers.0 as f32 / frame.0 as f32,
            placement.scale.1 * buffers.1 as f32 / frame.1 as f32,
        )
    }

    #[test]
    fn fr4_a_frame_is_never_scaled_non_uniformly_while_the_window_is_resized() {
        let buffers = (1920, 1088);
        // Wider and shorter, narrower and taller, both larger, both smaller, the same.
        for (frame, view) in [
            ((900, 700), (1300, 500)),
            ((900, 700), (600, 950)),
            ((900, 700), (1200, 900)),
            ((900, 700), (500, 400)),
            ((607, 1080), (607, 1080)),
        ] {
            let (x, y) = on_screen(frame_placement(buffers, frame, view), buffers, frame);
            assert!((x - 1.0).abs() < 1e-4, "{frame:?} in {view:?}: {x} across");
            assert!((y - 1.0).abs() < 1e-4, "{frame:?} in {view:?}: {y} down");
        }
    }

    #[test]
    fn fr4_a_window_larger_than_its_frame_is_filled_not_left_black() {
        let placement = frame_placement((1920, 1088), (900, 700), (1300, 700));
        assert_eq!(placement.covered, (900, 700));
        assert!(placement.fill);
        assert!(frame_placement((1920, 1088), (900, 700), (900, 760)).fill);
    }

    #[test]
    fn fr4_a_window_smaller_than_its_frame_crops_it_instead_of_squashing_it() {
        let placement = frame_placement((1920, 1088), (900, 700), (600, 500));
        assert_eq!(placement.covered, (600, 500));
        assert!(!placement.fill);
        // A frame of the window's size covers it exactly.
        let exact = frame_placement((1920, 1088), (600, 500), (600, 500));
        assert_eq!(exact.covered, (600, 500));
        assert!(!exact.fill);
    }

    #[test]
    fn fr4_a_frame_of_the_buffers_size_keeps_the_identity_scale() {
        assert_eq!(
            frame_placement((1280, 720), (1280, 720), (1280, 720)).scale,
            (1.0, 1.0)
        );
        assert_eq!(frame_placement((0, 0), (0, 0), (607, 1080)).scale, (1.0, 1.0));
    }

    #[test]
    fn fr4_the_fill_is_sampled_from_the_frames_right_and_bottom_edges() {
        let points = edge_points((900, 700));
        assert_eq!(points.len(), (2 * EDGE_SAMPLES) as usize);
        let (column, row) = points.split_at(EDGE_SAMPLES as usize);
        for &(x, y) in column {
            assert!(x == 899 && y < 700, "({x}, {y}) is not on the right edge");
        }
        for &(x, y) in row {
            assert!(y == 699 && x < 900, "({x}, {y}) is not on the bottom edge");
        }
        assert!(edge_points((0, 0)).iter().all(|&point| point == (0, 0)));
    }

    #[test]
    fn fr4_the_fill_is_the_colour_most_of_the_edge_has_and_is_opaque() {
        let background = [0xf6, 0xf2, 0xfa, 0xff];
        let text = [0x1d, 0x1b, 0x20, 0xff];
        let mut edge = [background; 16];
        edge[3] = text;
        edge[9] = text;
        assert_eq!(fill_colour(&edge), Some(background));
        assert_eq!(fill_colour(&[[10, 20, 30, 0]]), Some([10, 20, 30, 255]));
        assert_eq!(fill_colour(&[]), None);
        // Never black unless the app's own edge is black.
        assert_ne!(DEFAULT_FILL, [0, 0, 0, 255]);
    }

    #[test]
    fn fr4_a_frame_shows_only_the_region_its_content_fills() {
        assert_eq!(content_region((800, 600), (1920, 1080)), (800, 600));
        assert_eq!(content_region((1920, 1080), (1920, 1080)), (1920, 1080));
    }

    #[test]
    fn fr4_content_larger_than_the_buffer_is_clipped_to_it() {
        assert_eq!(content_region((2000, 600), (1920, 1080)), (1920, 600));
        assert_eq!(content_region((800, 1200), (1920, 1080)), (800, 1080));
    }

    #[test]
    fn fr4_a_frame_without_a_content_size_fills_its_buffer() {
        assert_eq!(content_region((0, 0), (1280, 720)), (1280, 720));
        assert_eq!(content_region((640, 0), (1280, 720)), (1280, 720));
    }

    #[test]
    fn fr4_resizing_a_window_keeps_its_swapchain() {
        let texture = (1920, 1080);
        assert_eq!(
            swapchain_change(None, texture),
            SwapchainChange::Create(texture)
        );
        // Frames of every size come from the same texture: nothing is resized.
        assert_eq!(
            swapchain_change(Some(texture), texture),
            SwapchainChange::Keep
        );
    }

    #[test]
    fn fr4_a_new_texture_size_resizes_the_swapchain_once() {
        assert_eq!(
            swapchain_change(Some((1920, 1080)), (2560, 1440)),
            SwapchainChange::Resize((2560, 1440))
        );
        assert_eq!(
            swapchain_change(Some((2560, 1440)), (2560, 1440)),
            SwapchainChange::Keep
        );
    }

    #[test]
    fn fr4_the_shared_texture_is_kept_while_the_color_buffer_keeps_its_size() {
        assert!(texture_fits((1920, 1080), (1920, 1080)));
        assert!(!texture_fits((1920, 1080), (2560, 1440)));
    }
}
