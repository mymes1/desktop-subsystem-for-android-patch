//! Whether a user's Android keeps running once nothing uses it.
//!
//! There are two run modes, as the settings app offers them:
//!
//! - "As needed" ([`RunMode::AsNeeded`], the default): once the last app window has closed and
//!   no app runs in the background and no widget is placed, the session stops after
//!   [`RunModePolicy::stop_after`] (five minutes unless the user picks another time). Opening an
//!   app in that time cancels the stop. Whether an app with notifications shown keeps Android
//!   running is the user's choice ([`RunModePolicy::keep_for_notifications`]). The next app
//!   launch, or the next action on a notification, starts Android again.
//! - "Continuous" ([`RunMode::Continuous`]): Android is never stopped for being unused. It goes
//!   into Doze, as it always has.
//!
//! Doze stays the idle state in both modes, and freezing stays the user's opt-in: stopping in
//! "As needed" comes after Doze, never instead of it.
//!
//! [`IdleStop`] is the policy on its own, with no clock or thread: the session tells it what it
//! sees ([`Usage`]) and when, and asks it whether the stop is due.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// How long "As needed" waits, once nothing uses Android, before it stops it.
pub const DEFAULT_STOP_AFTER: Duration = Duration::from_secs(5 * 60);

/// The shortest wait a user may choose, so a closed window that is opened again at once does
/// not cost a cold start.
pub const MIN_STOP_AFTER: Duration = Duration::from_secs(10);

/// The longest wait a user may choose: one day.
pub const MAX_STOP_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether Android stops once nothing uses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    /// Stop once nothing has used Android for a while. The default.
    #[default]
    AsNeeded,
    /// Keep Android running in Doze.
    Continuous,
}

impl RunMode {
    /// The name settings files and the command line use.
    pub fn as_str(self) -> &'static str {
        match self {
            RunMode::AsNeeded => "as-needed",
            RunMode::Continuous => "continuous",
        }
    }

    /// Reads a name [`RunMode::as_str`] writes.
    pub fn parse(text: &str) -> Result<RunMode, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "as-needed" | "as_needed" | "asneeded" => Ok(RunMode::AsNeeded),
            "continuous" => Ok(RunMode::Continuous),
            other => Err(format!(
                "{other:?} is not a run mode; use as-needed or continuous"
            )),
        }
    }
}

/// The user's run mode settings, as the session applies them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunModePolicy {
    pub mode: RunMode,
    /// How long nothing may use Android in "As needed" before it stops.
    pub stop_after: Duration,
    /// Whether an app with a notification shown keeps Android running in "As needed".
    pub keep_for_notifications: bool,
}

impl Default for RunModePolicy {
    fn default() -> RunModePolicy {
        RunModePolicy {
            mode: RunMode::AsNeeded,
            stop_after: DEFAULT_STOP_AFTER,
            keep_for_notifications: false,
        }
    }
}

impl RunModePolicy {
    /// The policy of a user's settings: their mode, their wait (0 is the default), and whether
    /// notifications keep Android running.
    pub fn of(mode: RunMode, stop_after_secs: u32, keep_for_notifications: bool) -> RunModePolicy {
        RunModePolicy {
            mode,
            stop_after: Duration::from_secs(normalise_stop_after_secs(stop_after_secs).into()),
            keep_for_notifications,
        }
    }
}

/// Keeps a stop wait the user chose within [`MIN_STOP_AFTER`] and [`MAX_STOP_AFTER`]; 0 means
/// the default.
pub fn normalise_stop_after_secs(secs: u32) -> u32 {
    if secs == 0 {
        return DEFAULT_STOP_AFTER.as_secs() as u32;
    }
    secs.clamp(MIN_STOP_AFTER.as_secs() as u32, MAX_STOP_AFTER.as_secs() as u32)
}

/// What uses Android at one moment, as the session sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    /// Whether any app window is shown.
    pub windows_shown: bool,
    /// How many apps run in the background with a foreground service.
    pub background_apps: usize,
    /// How many widgets are placed.
    pub widgets: usize,
    /// How many notifications from Android are shown on the host.
    pub notifications: usize,
}

impl Usage {
    /// Whether this usage keeps Android running under `policy`.
    pub fn keeps_running(&self, policy: &RunModePolicy) -> bool {
        policy.mode == RunMode::Continuous
            || self.windows_shown
            || self.background_apps > 0
            || self.widgets > 0
            || (policy.keep_for_notifications && self.notifications > 0)
    }
}

/// The stop of "As needed": when nothing has used Android since, and so when it is due.
#[derive(Debug, Clone, Default)]
pub struct IdleStop {
    unused_since: Option<Instant>,
}

impl IdleStop {
    /// Records what uses Android at `now`. Anything that keeps it running cancels the wait; the
    /// wait starts the first time nothing does.
    pub fn observe(&mut self, policy: &RunModePolicy, usage: Usage, now: Instant) {
        if usage.keeps_running(policy) {
            self.unused_since = None;
        } else if self.unused_since.is_none() {
            self.unused_since = Some(now);
        }
    }

    /// Starts over, as when Android starts or stops.
    pub fn reset(&mut self) {
        self.unused_since = None;
    }

    /// Since when nothing has used Android, while that is so.
    pub fn unused_since(&self) -> Option<Instant> {
        self.unused_since
    }

    /// Whether Android is to stop now.
    pub fn due(&self, policy: &RunModePolicy, now: Instant) -> bool {
        policy.mode == RunMode::AsNeeded
            && self
                .unused_since
                .is_some_and(|since| now.duration_since(since) >= policy.stop_after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_secs(300);

    fn policy(mode: RunMode, keep_for_notifications: bool) -> RunModePolicy {
        RunModePolicy {
            mode,
            stop_after: WAIT,
            keep_for_notifications,
        }
    }

    fn unused() -> Usage {
        Usage::default()
    }

    #[test]
    fn fr31_as_needed_is_the_default_with_five_minutes() {
        let default = RunModePolicy::default();
        assert_eq!(default.mode, RunMode::AsNeeded);
        assert_eq!(default.stop_after, Duration::from_secs(300));
        assert!(!default.keep_for_notifications);
        assert_eq!(RunModePolicy::of(RunMode::AsNeeded, 0, false), default);
    }

    #[test]
    fn fr31_as_needed_stops_after_the_wait_once_nothing_uses_android() {
        let policy = policy(RunMode::AsNeeded, false);
        let start = Instant::now();
        let mut stop = IdleStop::default();
        stop.observe(&policy, unused(), start);
        assert!(!stop.due(&policy, start + WAIT - Duration::from_secs(1)));
        assert!(stop.due(&policy, start + WAIT));
    }

    #[test]
    fn fr31_an_app_opened_during_the_wait_cancels_the_stop() {
        let policy = policy(RunMode::AsNeeded, false);
        let start = Instant::now();
        let mut stop = IdleStop::default();
        stop.observe(&policy, unused(), start);
        let opened = start + WAIT / 2;
        stop.observe(
            &policy,
            Usage {
                windows_shown: true,
                ..unused()
            },
            opened,
        );
        assert!(!stop.due(&policy, start + WAIT * 2));
        // Closed again, the whole wait starts over.
        let closed = start + WAIT * 2;
        stop.observe(&policy, unused(), closed);
        assert!(!stop.due(&policy, closed + WAIT / 2));
        assert!(stop.due(&policy, closed + WAIT));
    }

    #[test]
    fn fr31_background_apps_and_widgets_keep_android_running() {
        let policy = policy(RunMode::AsNeeded, false);
        for usage in [
            Usage {
                background_apps: 1,
                ..unused()
            },
            Usage {
                widgets: 1,
                ..unused()
            },
        ] {
            assert!(usage.keeps_running(&policy), "{usage:?}");
            let mut stop = IdleStop::default();
            let start = Instant::now();
            stop.observe(&policy, usage, start);
            assert!(!stop.due(&policy, start + WAIT * 10));
        }
    }

    #[test]
    fn fr31_notifications_keep_android_running_only_when_chosen() {
        let shown = Usage {
            notifications: 2,
            ..unused()
        };
        assert!(!shown.keeps_running(&policy(RunMode::AsNeeded, false)));
        assert!(shown.keeps_running(&policy(RunMode::AsNeeded, true)));
    }

    #[test]
    fn fr31_continuous_never_stops() {
        let policy = policy(RunMode::Continuous, false);
        let start = Instant::now();
        let mut stop = IdleStop::default();
        stop.observe(&policy, unused(), start);
        assert!(!stop.due(&policy, start + WAIT * 100));
    }

    #[test]
    fn fr31_the_wait_is_kept_between_ten_seconds_and_a_day() {
        assert_eq!(normalise_stop_after_secs(0), 300);
        assert_eq!(normalise_stop_after_secs(1), 10);
        assert_eq!(normalise_stop_after_secs(600), 600);
        assert_eq!(normalise_stop_after_secs(u32::MAX), 86_400);
    }

    #[test]
    fn fr31_run_modes_are_named_as_the_command_line_takes_them() {
        for mode in [RunMode::AsNeeded, RunMode::Continuous] {
            assert_eq!(RunMode::parse(mode.as_str()), Ok(mode));
        }
        assert!(RunMode::parse("sometimes").is_err());
    }
}
