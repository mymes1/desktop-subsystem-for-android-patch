//! The window manager's side of a task's shared frame surface: a frame is written only once
//! the window host has copied the last one out, and a frame that arrives meanwhile waits,
//! replaced by any newer one, so the window always catches up to the latest frame.

/// One task's frames on their way to its window host.
#[derive(Debug, Default)]
pub struct FrameGate<F> {
    /// The sequence number of the frame the window host has not answered yet.
    in_flight: Option<u64>,
    /// The newest frame that waits for the window host's answer.
    waiting: Option<F>,
    next_sequence: u64,
}

impl<F> FrameGate<F> {
    pub fn new() -> FrameGate<F> {
        FrameGate {
            in_flight: None,
            waiting: None,
            next_sequence: 1,
        }
    }

    /// A new frame: `Some((sequence, frame))` to write and announce now, or `None` when it
    /// waits for the window host.
    pub fn offer(&mut self, frame: F) -> Option<(u64, F)> {
        if self.in_flight.is_some() {
            self.waiting = Some(frame);
            return None;
        }
        Some(self.send(frame))
    }

    fn send(&mut self, frame: F) -> (u64, F) {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.in_flight = Some(sequence);
        (sequence, frame)
    }

    /// The window host copied frame `sequence` out: the waiting frame, if any, goes next. An
    /// answer for any other frame (one from before the window host reconnected) is ignored.
    pub fn presented(&mut self, sequence: u64) -> Option<(u64, F)> {
        if self.in_flight != Some(sequence) {
            return None;
        }
        self.in_flight = None;
        let frame = self.waiting.take()?;
        Some(self.send(frame))
    }

    /// The window host went away or the surface was remade: nothing is in flight any more.
    pub fn reset(&mut self) {
        self.in_flight = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr3_a_frame_goes_at_once_when_the_window_host_is_ready() {
        let mut gate = FrameGate::new();
        assert_eq!(gate.offer("a"), Some((1, "a")));
    }

    #[test]
    fn pr3_frames_wait_for_the_window_host_and_only_the_newest_is_kept() {
        let mut gate = FrameGate::new();
        gate.offer("a");
        assert_eq!(gate.offer("b"), None);
        assert_eq!(gate.offer("c"), None);
        assert_eq!(gate.presented(1), Some((2, "c")));
        assert_eq!(gate.presented(2), None);
        assert_eq!(gate.offer("d"), Some((3, "d")));
    }

    #[test]
    fn nfr5_an_answer_for_another_frame_is_ignored() {
        let mut gate = FrameGate::new();
        gate.offer("a");
        gate.offer("b");
        assert_eq!(gate.presented(7), None);
        assert_eq!(gate.presented(1), Some((2, "b")));
        gate.reset();
        assert_eq!(gate.offer("c"), Some((3, "c")));
    }
}
