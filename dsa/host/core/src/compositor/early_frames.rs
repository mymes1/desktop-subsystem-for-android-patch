//! The frames a task draws before its window host is there to show them.
//!
//! A task's window belongs to its app's window host, a program of its own that the window
//! manager starts when the task appears and that connects a moment later. The guest does not
//! wait for it: its first frames, the splash screen above all, are drawn at once and are often
//! the only ones for a while, since a still picture is composed once. Dropped, they leave the
//! window empty until the app draws again. So the window manager takes a task's frames from
//! the moment the task has a window, keeps the newest one in the task's texture, and hands
//! both over when the window host connects.

/// What the window manager does with one task's texture and frames, before and after its
/// window host is connected.
#[derive(Debug, Default)]
pub struct EarlyFrames {
    linked: bool,
    /// Whether the window host has been told of the texture the frames are copied into.
    announced: bool,
    /// The content size of the newest frame the window host has not been sent.
    unsent: Option<(u32, u32)>,
}

/// What goes to a window host when it connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Handover {
    /// Tell it of the texture.
    pub texture: bool,
    /// Then show this frame, of this content size, from the texture.
    pub frame: Option<(u32, u32)>,
}

impl EarlyFrames {
    /// A task whose window host is not connected yet.
    pub fn waiting() -> EarlyFrames {
        EarlyFrames::default()
    }

    /// A task whose window host is connected.
    pub fn linked() -> EarlyFrames {
        EarlyFrames {
            linked: true,
            ..EarlyFrames::default()
        }
    }

    pub fn is_linked(&self) -> bool {
        self.linked
    }

    /// A new texture was made for the task's frames. Returns whether the window host is told
    /// of it now; otherwise it is told when it connects.
    pub fn texture_made(&mut self) -> bool {
        self.announced = self.linked;
        self.unsent = None;
        self.linked
    }

    /// A frame of `content` pixels is in the texture. Returns whether it goes to the window
    /// host now; otherwise it is kept, replacing an older one, until the window host connects.
    pub fn frame(&mut self, content: (u32, u32)) -> bool {
        if self.linked && self.announced {
            return true;
        }
        self.unsent = Some(content);
        false
    }

    /// The window host connected. `has_texture` says whether a texture was made before.
    pub fn link(&mut self, has_texture: bool) -> Handover {
        if self.linked {
            return Handover::default();
        }
        self.linked = true;
        if !has_texture {
            self.unsent = None;
            return Handover::default();
        }
        self.announced = true;
        Handover {
            texture: true,
            frame: self.unsent.take(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr3_a_splash_drawn_before_the_window_host_connects_is_shown_when_it_does() {
        let mut frames = EarlyFrames::waiting();
        assert!(!frames.texture_made());
        // The splash screen, then nothing more for a while.
        assert!(!frames.frame((800, 600)));
        assert_eq!(
            frames.link(true),
            Handover {
                texture: true,
                frame: Some((800, 600)),
            }
        );
        // From then on every frame goes at once.
        assert!(frames.frame((800, 600)));
    }

    #[test]
    fn fr3_only_the_newest_early_frame_is_handed_over() {
        let mut frames = EarlyFrames::waiting();
        frames.texture_made();
        frames.frame((800, 600));
        frames.frame((640, 480));
        assert_eq!(frames.link(true).frame, Some((640, 480)));
    }

    #[test]
    fn fr3_a_window_host_connected_before_any_frame_gets_nothing_extra() {
        let mut frames = EarlyFrames::waiting();
        assert_eq!(frames.link(false), Handover::default());
        assert!(frames.texture_made());
        assert!(frames.frame((800, 600)));
        // Connecting twice hands nothing over twice.
        assert_eq!(frames.link(true), Handover::default());
    }

    #[test]
    fn fr3_a_connected_window_host_gets_every_frame_at_once() {
        let mut frames = EarlyFrames::linked();
        assert!(frames.texture_made());
        assert!(frames.frame((10, 20)));
        assert!(frames.frame((12, 20)));
    }

    #[test]
    fn fr3_a_frame_in_a_texture_the_window_host_was_not_told_of_waits_for_it() {
        let mut frames = EarlyFrames::waiting();
        frames.texture_made();
        frames.frame((10, 20));
        // A new texture before the window host connects: the frame in the old one is gone.
        frames.texture_made();
        assert_eq!(
            frames.link(true),
            Handover {
                texture: true,
                frame: None,
            }
        );
    }
}
