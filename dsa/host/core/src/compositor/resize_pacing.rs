//! How fast the sizes of a drag go to the guest: no faster than the app draws them.
//!
//! While the user drags a window's edge, the window asks for a new size every few
//! milliseconds. An app that takes longer than that to lay itself out (Settings takes around a
//! hundred milliseconds) would otherwise be handed a new size before it has drawn the last one:
//! each size starts another layout and another wait for the window manager's sync, and the
//! frame the user is waiting for keeps being pushed back. So once a size of a drag has gone to
//! the guest, the next waits until a frame of that size has been drawn, or until
//! [`PATIENCE`] has passed, and then only the latest size goes; the sizes the user dragged past
//! meanwhile are never sent. The size that ends a drag is never held back.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::tasks::TaskRequest;

/// The longest a size of a drag waits for the app to draw the one before it. An app that never
/// draws exactly the size asked for (the guest keeps a smallest size) still follows the drag
/// this often.
pub const PATIENCE: Duration = Duration::from_millis(150);

/// Paces the sizes of a drag, task by task.
#[derive(Debug, Default)]
pub struct ResizePacer {
    /// The size last sent for each task during a drag and when, until a frame of it is drawn.
    in_flight: HashMap<i32, ((u32, u32), Instant)>,
}

impl ResizePacer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a size of a drag may go to the guest for `task_id` now.
    pub fn may_send(&self, task_id: i32, now: Instant) -> bool {
        match self.in_flight.get(&task_id) {
            None => true,
            Some((_, sent)) => now.saturating_duration_since(*sent) >= PATIENCE,
        }
    }

    /// A size of a drag went to the guest for `task_id`.
    pub fn sent(&mut self, task_id: i32, size: (u32, u32), now: Instant) {
        self.in_flight.insert(task_id, (size, now));
    }

    /// The drag of `task_id` ended, or its task went away: nothing of it is waited for.
    pub fn ended(&mut self, task_id: i32) {
        self.in_flight.remove(&task_id);
    }

    /// The app of `task_id` drew a frame of `size`. Returns whether that was the size waited
    /// for, so the next size may go at once.
    pub fn frame_drawn(&mut self, task_id: i32, size: (u32, u32)) -> bool {
        let drawn = self
            .in_flight
            .get(&task_id)
            .is_some_and(|(waited, _)| *waited == size);
        if drawn {
            self.in_flight.remove(&task_id);
        }
        drawn
    }

    /// Splits `requests`, already folded together, into those that go now and the sizes of a
    /// drag held back until their app has drawn the size before. Whatever goes is counted as
    /// sent. A size that ends a drag (not live) always goes, and ends the wait.
    pub fn pace(
        &mut self,
        requests: Vec<TaskRequest>,
        now: Instant,
    ) -> (Vec<TaskRequest>, Vec<TaskRequest>) {
        let mut go = Vec::new();
        let mut held = Vec::new();
        for request in requests {
            match &request {
                TaskRequest::Resize {
                    task_id,
                    width,
                    height,
                    live: true,
                } => {
                    if !self.may_send(*task_id, now) {
                        held.push(request);
                        continue;
                    }
                    self.sent(*task_id, (*width, *height), now);
                }
                TaskRequest::Resize { task_id, .. }
                | TaskRequest::SetDensity { task_id, .. }
                | TaskRequest::Close { task_id } => self.ended(*task_id),
                _ => {}
            }
            go.push(request);
        }
        (go, held)
    }

    /// How long until a held size of `task_id` may go anyway, if one is being waited for.
    pub fn wait_left(&self, task_id: i32, now: Instant) -> Option<Duration> {
        self.in_flight
            .get(&task_id)
            .map(|(_, sent)| PATIENCE.saturating_sub(now.saturating_duration_since(*sent)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr4_the_first_size_of_a_drag_goes_at_once() {
        let pacer = ResizePacer::new();
        assert!(pacer.may_send(7, Instant::now()));
    }

    #[test]
    fn fr4_the_next_size_waits_until_the_app_drew_the_last() {
        let start = Instant::now();
        let mut pacer = ResizePacer::new();
        pacer.sent(7, (900, 700), start);
        assert!(!pacer.may_send(7, start + Duration::from_millis(20)));
        // A frame of another size (an older one) does not count.
        assert!(!pacer.frame_drawn(7, (880, 700)));
        assert!(!pacer.may_send(7, start + Duration::from_millis(40)));
        assert!(pacer.frame_drawn(7, (900, 700)));
        assert!(pacer.may_send(7, start + Duration::from_millis(41)));
    }

    #[test]
    fn fr4_a_size_the_app_never_draws_holds_the_drag_back_only_so_long() {
        let start = Instant::now();
        let mut pacer = ResizePacer::new();
        pacer.sent(7, (100, 100), start);
        assert_eq!(
            pacer.wait_left(7, start + Duration::from_millis(50)),
            Some(PATIENCE - Duration::from_millis(50))
        );
        assert!(pacer.may_send(7, start + PATIENCE));
    }

    #[test]
    fn fr4_one_task_waiting_does_not_hold_back_another() {
        let start = Instant::now();
        let mut pacer = ResizePacer::new();
        pacer.sent(7, (900, 700), start);
        assert!(pacer.may_send(8, start));
        assert_eq!(pacer.wait_left(8, start), None);
    }

    fn live(task_id: i32, width: u32) -> TaskRequest {
        TaskRequest::Resize {
            task_id,
            width,
            height: 700,
            live: true,
        }
    }

    #[test]
    fn fr4_sizes_dragged_past_while_the_app_draws_are_held_and_the_end_goes_at_once() {
        let start = Instant::now();
        let mut pacer = ResizePacer::new();
        let (go, held) = pacer.pace(vec![live(7, 900)], start);
        assert_eq!((go, held), (vec![live(7, 900)], vec![]));
        let later = start + Duration::from_millis(30);
        let (go, held) = pacer.pace(vec![live(7, 960)], later);
        assert_eq!((go, held), (vec![], vec![live(7, 960)]));
        // The drag ends: its size goes even though the app has not drawn 900 yet.
        let end = TaskRequest::Resize {
            task_id: 7,
            width: 1000,
            height: 700,
            live: false,
        };
        let (go, held) = pacer.pace(vec![end.clone()], later);
        assert_eq!((go, held), (vec![end], vec![]));
        assert!(pacer.may_send(7, later));
    }

    #[test]
    fn fr4_a_held_size_goes_once_its_app_drew_the_last() {
        let start = Instant::now();
        let mut pacer = ResizePacer::new();
        pacer.pace(vec![live(7, 900)], start);
        pacer.frame_drawn(7, (900, 700));
        let (go, _) = pacer.pace(vec![live(7, 960)], start + Duration::from_millis(5));
        assert_eq!(go, vec![live(7, 960)]);
    }

    #[test]
    fn fr4_the_end_of_a_drag_clears_the_wait() {
        let start = Instant::now();
        let mut pacer = ResizePacer::new();
        pacer.sent(7, (900, 700), start);
        pacer.ended(7);
        assert!(pacer.may_send(7, start));
    }
}
