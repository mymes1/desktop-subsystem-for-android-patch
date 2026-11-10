//! What a press on a widget window is doing: a tap or a drag that is the widget's own, a long
//! press that lifts it for editing, a click on its More button, a move of its window. And how
//! much of its More button shows as it fades in and out.

use std::time::Instant;

use dsa_protocol::host_api::types::PointerAction;

use super::geometry::{FADE, HOLD_TO_MOVE};

/// How much of the More button shows, from 0 (none) to 1, while it fades from `from` to `to`
/// since `since`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fade {
    pub from: f32,
    pub to: f32,
    pub since: Instant,
}

impl Fade {
    /// A button that is not shown.
    pub fn hidden(now: Instant) -> Fade {
        Fade {
            from: 0.0,
            to: 0.0,
            since: now,
        }
    }

    pub fn level(&self, now: Instant) -> f32 {
        let done = now.saturating_duration_since(self.since).as_secs_f32() / FADE.as_secs_f32();
        self.from + (self.to - self.from) * done.clamp(0.0, 1.0)
    }

    pub fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.since) >= FADE
    }

    /// The fade towards `shown` from where this one is at `now`; this one when it goes
    /// there already.
    pub fn toward(self, shown: bool, now: Instant) -> Fade {
        if (self.to > 0.5) == shown {
            return self;
        }
        let to = if shown { 1.0 } else { 0.0 };
        Fade {
            from: self.level(now),
            to,
            since: now,
        }
    }
}

/// Whether the More button shows: while the pointer is over the widget, while the window has
/// the keyboard, while its menu is open, and while it is pressed or moving the window, but
/// never while the widget is being edited, whose border and handles take its place.
pub fn more_button_shown(
    hover: bool,
    focused: bool,
    menu_open: bool,
    editing: bool,
    gesture: &Gesture,
) -> bool {
    !editing
        && (hover
            || focused
            || menu_open
            || matches!(gesture, Gesture::Button { .. } | Gesture::Moving { .. }))
}

/// What a press on a widget window is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    Idle,
    /// Pressed inside the widget, at `down` on the screen, `since` then; Android has the
    /// press. Once it `slid` past the drag threshold it is the widget's own drag.
    Widget {
        down: (i32, i32),
        since: Instant,
        slid: bool,
    },
    /// Pressed on the More button, at `down` on the screen.
    Button {
        down: (i32, i32),
    },
    /// A long press: held still for [`HOLD_TO_MOVE`], so the widget is lifted for editing and
    /// Android's press was cancelled. The pointer is at `at` on the screen, `grab` pixels
    /// from the window's top left corner; dragging past the threshold moves the widget, and
    /// letting go opens its menu.
    Lifted {
        at: (i32, i32),
        grab: (i32, i32),
    },
    /// Pressed on a widget being edited, at `down` on the screen: Android never has it. A
    /// drag moves the widget and a click opens its menu.
    Editing {
        down: (i32, i32),
    },
    /// Moving the window, the pointer `grab` pixels from its top left corner.
    Moving {
        grab: (i32, i32),
    },
}

/// What a step of a gesture asks of the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Nothing,
    /// The pointer goes to Android as this action.
    Android(PointerAction),
    /// The More button is pressed, or no longer pressed.
    Button,
    /// The More button was clicked: its menu opens under it.
    OpenMenu,
    /// The press was held long enough: Android's press is cancelled, so it never becomes a
    /// click, and the widget is lifted for editing.
    Lift,
    /// A long press, or a click on a widget being edited, ended where it began: the menu of
    /// the widget opens under it.
    EditMenu,
    /// The window starts moving, to this top left corner.
    StartMove {
        to: (i32, i32),
    },
    /// The window moves to this top left corner.
    MoveTo((i32, i32)),
    /// The move ended: the service is told where the window is, and an edit ends.
    EndMove,
}

/// Whether `at` is past the drag threshold (`threshold` pixels each way) from `down`.
fn beyond(down: (i32, i32), at: (i32, i32), threshold: (i32, i32)) -> bool {
    (at.0 - down.0).abs() > threshold.0 || (at.1 - down.1).abs() > threshold.1
}

impl Gesture {
    /// The button went down at `at` on the screen, on the More button or not, on a widget
    /// being edited (`editing`) or not.
    pub fn down(&mut self, on_button: bool, editing: bool, at: (i32, i32), now: Instant) -> Step {
        if on_button && !editing {
            *self = Gesture::Button { down: at };
            Step::Button
        } else if editing {
            *self = Gesture::Editing { down: at };
            Step::Nothing
        } else {
            *self = Gesture::Widget {
                down: at,
                since: now,
                slid: false,
            };
            Step::Android(PointerAction::Down)
        }
    }

    /// The pointer moved to `at` on the screen, the window's top left corner at `origin`.
    pub fn moved(&mut self, at: (i32, i32), origin: (i32, i32), threshold: (i32, i32)) -> Step {
        match *self {
            Gesture::Idle => Step::Nothing,
            Gesture::Widget { down, since, slid } => {
                if !slid && beyond(down, at, threshold) {
                    *self = Gesture::Widget {
                        down,
                        since,
                        slid: true,
                    };
                }
                Step::Android(PointerAction::Move)
            }
            Gesture::Button { down } | Gesture::Editing { down } if beyond(down, at, threshold) => {
                let grab = (down.0 - origin.0, down.1 - origin.1);
                *self = Gesture::Moving { grab };
                Step::StartMove {
                    to: (at.0 - grab.0, at.1 - grab.1),
                }
            }
            Gesture::Button { .. } | Gesture::Editing { .. } => Step::Nothing,
            Gesture::Lifted { at: held, grab } if beyond(held, at, threshold) => {
                *self = Gesture::Moving { grab };
                Step::StartMove {
                    to: (at.0 - grab.0, at.1 - grab.1),
                }
            }
            Gesture::Lifted { .. } => Step::Nothing,
            Gesture::Moving { grab } => Step::MoveTo((at.0 - grab.0, at.1 - grab.1)),
        }
    }

    /// The press has been held until `now`, the pointer at `at` on the screen and the
    /// window's top left corner at `origin`: one held still for [`HOLD_TO_MOVE`] is a long
    /// press, which lifts the widget for editing.
    pub fn held(&mut self, at: (i32, i32), origin: (i32, i32), now: Instant) -> Step {
        let Gesture::Widget { since, slid, .. } = *self else {
            return Step::Nothing;
        };
        if slid || now.saturating_duration_since(since) < HOLD_TO_MOVE {
            return Step::Nothing;
        }
        let grab = (at.0 - origin.0, at.1 - origin.1);
        *self = Gesture::Lifted { at, grab };
        Step::Lift
    }

    /// The window grew or shrank by `by` pixels on its left and top as an edit began or
    /// ended under the press, so the pointer is that much further from its corner.
    pub fn regrab(&mut self, by: (i32, i32)) {
        if let Gesture::Lifted { grab, .. } | Gesture::Moving { grab } = self {
            *grab = (grab.0 + by.0, grab.1 + by.1);
        }
    }

    /// The button went up, on the More button or not.
    pub fn up(&mut self, on_button: bool) -> Step {
        match std::mem::replace(self, Gesture::Idle) {
            Gesture::Idle => Step::Nothing,
            Gesture::Widget { .. } => Step::Android(PointerAction::Up),
            Gesture::Button { .. } if on_button => Step::OpenMenu,
            Gesture::Button { .. } => Step::Button,
            Gesture::Lifted { .. } | Gesture::Editing { .. } => Step::EditMenu,
            Gesture::Moving { .. } => Step::EndMove,
        }
    }

    /// The window lost the mouse before the button went up.
    pub fn cancel(&mut self) -> Step {
        match std::mem::replace(self, Gesture::Idle) {
            Gesture::Idle | Gesture::Lifted { .. } | Gesture::Editing { .. } => Step::Nothing,
            Gesture::Widget { .. } => Step::Android(PointerAction::Cancel),
            Gesture::Button { .. } => Step::Button,
            Gesture::Moving { .. } => Step::EndMove,
        }
    }
}
