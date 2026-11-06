//! When an app's drag out of its window is over, as macOS file promises make it.
//!
//! A drag out of a task window carries each file as a file promise
//! (`NSFilePromiseProvider`). Finder asks for a promise to be written after the drop, often
//! after the dragging session has ended, so the drag cannot end in the guest when the session
//! does. [`PromiseTracker`] says when it can: at once for a cancelled drag, and for a dropped
//! one once every promise it carries has been written or has failed.

/// How far the promises of one drag have got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromiseTracker {
    files: usize,
    settled: usize,
    session: Option<bool>,
}

impl PromiseTracker {
    /// A drag that carries `files` file promises.
    pub fn new(files: usize) -> PromiseTracker {
        PromiseTracker {
            files,
            settled: 0,
            session: None,
        }
    }

    /// One promise was written, or failed.
    pub fn settled(&mut self) {
        self.settled += 1;
    }

    /// The dragging session ended, `dropped` on a target or not.
    pub fn session_ended(&mut self, dropped: bool) {
        self.session = Some(dropped);
    }

    /// Whether the drag can end in the guest now, and how: `Some(false)` when it was
    /// cancelled, `Some(true)` when it was dropped and every promise has settled.
    pub fn outcome(&self) -> Option<bool> {
        match self.session {
            Some(false) => Some(false),
            Some(true) if self.settled >= self.files => Some(true),
            _ => None,
        }
    }

    /// Whether the session has ended, whatever the promises.
    pub fn session_over(&self) -> bool {
        self.session.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr29_a_dropped_drag_ends_once_every_promised_file_is_written() {
        let mut tracker = PromiseTracker::new(2);
        tracker.settled();
        assert_eq!(tracker.outcome(), None, "the session has not ended");
        tracker.session_ended(true);
        assert_eq!(tracker.outcome(), None, "one file is still being written");
        tracker.settled();
        assert_eq!(tracker.outcome(), Some(true));
    }

    #[test]
    fn fr29_a_cancelled_drag_ends_at_once() {
        let mut tracker = PromiseTracker::new(3);
        tracker.session_ended(false);
        assert_eq!(tracker.outcome(), Some(false));
        assert!(tracker.session_over());
    }

    #[test]
    fn fr29_a_text_drag_ends_with_its_session() {
        let mut tracker = PromiseTracker::new(0);
        tracker.session_ended(true);
        assert_eq!(tracker.outcome(), Some(true));
    }
}
