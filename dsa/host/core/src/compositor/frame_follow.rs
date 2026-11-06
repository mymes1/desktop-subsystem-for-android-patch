//! How a task window's size follows the frames its app draws, so the window is never shown at
//! a size other than the frame in it.
//!
//! A native window lays itself out within the resize, so its content always fits it. An
//! Android app needs tens to hundreds of milliseconds to draw at a new size, and anything the
//! window showed meanwhile (the last frame stretched, cropped, or with a margin filled in) did
//! not match the window. So the window takes a size only together with a frame of that size:
//!
//! - When the user drags an edge (or anything else proposes a size), the size goes to the
//!   guest, and the window stays at the size of the frame it shows, or takes the size of a
//!   newer frame that has arrived and is waiting ([`FrameFollow::size_for`]).
//! - A frame of the window's size is shown at once. A frame of another size is kept out of
//!   sight, and the window is told to take its size; the frame is shown in the same
//!   composition commit as the new window size ([`FrameFollow::view_taken`]).
//! - Between the window choosing a kept frame's size and taking it, that frame is claimed:
//!   a newer frame waits rather than replace it, so the size taken always has its frame.
//!
//! The window therefore follows the mouse as fast as the app draws, and the last size of a
//! drag, which the guest is asked for when the drag ends, arrives as a frame like any other.
//! It names no window system.

/// What to do with a frame that arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrival {
    /// It is the window's size: show it now.
    Show,
    /// Another size: keep it out of sight and have the window take its size.
    Keep,
    /// The window is taking the size of the frame kept before it: wait until it has.
    Wait,
}

/// One window's frames and size.
#[derive(Debug, Clone, Default)]
pub struct FrameFollow {
    /// The window's client size.
    view: (u32, u32),
    /// The size of the frame on the screen, once one is.
    shown: Option<(u32, u32)>,
    /// A frame that arrived at another size than the window's, not yet shown.
    kept: Option<(u32, u32)>,
    /// Whether the window has chosen the kept frame's size and not yet taken it.
    claimed: bool,
}

impl FrameFollow {
    /// A window whose client area is `view` pixels and that has shown nothing yet.
    pub fn new(view: (u32, u32)) -> Self {
        FrameFollow {
            view,
            ..Self::default()
        }
    }

    /// The window's client size.
    pub fn view(&self) -> (u32, u32) {
        self.view
    }

    /// The size of the frame on the screen.
    pub fn shown(&self) -> Option<(u32, u32)> {
        self.shown
    }

    /// The size of a frame kept out of sight for the window to take.
    pub fn kept(&self) -> Option<(u32, u32)> {
        self.kept
    }

    /// Whether a frame that arrives now must wait.
    pub fn claimed(&self) -> bool {
        self.claimed
    }

    /// A frame of `size` arrived.
    pub fn frame_arrived(&mut self, size: (u32, u32)) -> Arrival {
        if self.claimed {
            return Arrival::Wait;
        }
        if size == self.view {
            self.shown = Some(size);
            self.kept = None;
            Arrival::Show
        } else {
            self.kept = Some(size);
            Arrival::Keep
        }
    }

    /// Something proposes a client size for the window (a step of a drag, a maximise, a move
    /// to another monitor). Returns the size the window takes instead: a kept frame's, which
    /// is claimed until the window has taken it, or the size of the frame it shows. `None`
    /// before any frame has arrived, when the window takes what is proposed.
    pub fn size_for(&mut self, _proposed: (u32, u32)) -> Option<(u32, u32)> {
        if let Some(kept) = self.kept {
            self.claimed = true;
            return Some(kept);
        }
        self.shown
    }

    /// The window's client area is now `view`. Returns whether the kept frame is of that size
    /// and is to be shown with it. Ends any claim.
    pub fn view_taken(&mut self, view: (u32, u32)) -> bool {
        self.view = view;
        self.claimed = false;
        if self.kept == Some(view) {
            self.shown = self.kept.take();
            true
        } else {
            false
        }
    }

    /// The window settled without taking the size it chose (the window system did not apply
    /// it): frames no longer wait for it.
    pub fn release(&mut self) {
        self.claimed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window and its app, stepped by hand: what is on the screen after each step.
    struct Screen {
        follow: FrameFollow,
        /// What the frame on the screen is, as the compositor shows it.
        on_screen: Option<(u32, u32)>,
    }

    impl Screen {
        fn new(view: (u32, u32), first: (u32, u32)) -> Screen {
            let mut screen = Screen {
                follow: FrameFollow::new(view),
                on_screen: None,
            };
            screen.frame(first);
            screen
        }

        /// The window proposes `size`, takes what it is given, and shows the kept frame with
        /// it when that is its size.
        fn propose(&mut self, size: (u32, u32)) {
            let taken = self.follow.size_for(size).unwrap_or(size);
            if self.follow.view_taken(taken) {
                self.on_screen = self.follow.shown();
            }
            self.check();
        }

        /// The app drew a frame of `size`; a kept one has the window follow it at once.
        fn frame(&mut self, size: (u32, u32)) {
            match self.follow.frame_arrived(size) {
                Arrival::Show => self.on_screen = Some(size),
                Arrival::Keep => {
                    // The window is told, and takes the size of the kept frame.
                    let kept = self.follow.kept().expect("a kept frame");
                    self.propose(kept);
                }
                Arrival::Wait => panic!("no claim is open between steps"),
            }
            self.check();
        }

        fn check(&self) {
            if let Some(frame) = self.on_screen {
                assert_eq!(
                    self.follow.view(),
                    frame,
                    "the window is {:?} but shows a frame of {frame:?}",
                    self.follow.view()
                );
            }
        }
    }

    #[test]
    fn fr4_the_window_stays_at_its_frames_size_until_a_frame_of_the_new_size_arrives() {
        let mut screen = Screen::new((800, 600), (800, 600));
        // The drag proposes larger sizes; the app has drawn none of them yet.
        for width in [820, 860, 900] {
            screen.propose((width, 600));
            assert_eq!(screen.follow.view(), (800, 600));
        }
        // The app draws the size it was asked for: the window takes it with the frame.
        screen.frame((860, 600));
        assert_eq!(screen.follow.view(), (860, 600));
        assert_eq!(screen.on_screen, Some((860, 600)));
    }

    #[test]
    fn fr4_the_window_is_never_shown_at_a_size_other_than_its_frame() {
        // A drag out and back, with the app drawing every third size late.
        let mut screen = Screen::new((800, 600), (800, 600));
        let mut drawn = Vec::new();
        for step in 0..60u32 {
            let width = 800 + (step % 30) * 10 - if step >= 30 { (step - 30) * 20 } else { 0 };
            let size = (width.max(400), 600 + step % 7);
            screen.propose(size);
            drawn.push(size);
            if step % 3 == 2 {
                let late = drawn[drawn.len() / 2];
                screen.frame(late);
            }
        }
        screen.check();
    }

    #[test]
    fn fr4_the_last_size_of_a_drag_is_reached_exactly() {
        let mut screen = Screen::new((800, 600), (800, 600));
        for width in (810..=1000).step_by(10) {
            screen.propose((width, 700));
        }
        // The drag ended at 1000x700; the app draws an older size first, then that one.
        screen.frame((900, 700));
        screen.frame((1000, 700));
        assert_eq!(screen.follow.view(), (1000, 700));
        assert_eq!(screen.on_screen, Some((1000, 700)));
    }

    #[test]
    fn fr4_a_newer_frame_waits_while_the_window_takes_the_kept_one() {
        let mut follow = FrameFollow::new((800, 600));
        follow.frame_arrived((800, 600));
        assert_eq!(follow.frame_arrived((900, 600)), Arrival::Keep);
        assert_eq!(follow.size_for((950, 600)), Some((900, 600)));
        // Between choosing the size and taking it, the kept frame is not replaced.
        assert_eq!(follow.frame_arrived((950, 600)), Arrival::Wait);
        assert!(follow.view_taken((900, 600)));
        assert_eq!(follow.shown(), Some((900, 600)));
        assert_eq!(follow.frame_arrived((950, 600)), Arrival::Keep);
    }

    #[test]
    fn fr4_a_size_the_window_system_did_not_apply_lets_frames_go_again() {
        let mut follow = FrameFollow::new((800, 600));
        follow.frame_arrived((800, 600));
        follow.frame_arrived((900, 600));
        follow.size_for((900, 600));
        follow.release();
        assert_eq!(follow.frame_arrived((800, 600)), Arrival::Show);
    }

    #[test]
    fn fr4_before_any_frame_the_window_takes_what_is_proposed() {
        let mut follow = FrameFollow::new((800, 600));
        assert_eq!(follow.size_for((900, 700)), None);
        assert!(!follow.view_taken((900, 700)));
        assert_eq!(follow.frame_arrived((900, 700)), Arrival::Show);
    }
}
