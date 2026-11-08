//! The guest display's refresh rate on macOS: the Mac's fastest display, capped at 60 Hz in
//! Low Power Mode.
//!
//! The agent runs in the user's login session, so it reads both itself: the displays' current
//! modes from Core Graphics, and Low Power Mode from `NSProcessInfo`. [`follow`] reads Low
//! Power Mode every [`SAVER_POLL`], so the cap follows within a second of the user turning it
//! on or off, and the displays every [`dsa_host_core::guest_refresh::MONITOR_POLL`]. Each goes
//! to the user's session, whose rule ([`dsa_host_core::guest_refresh::refresh_rate`]) sets the
//! rate the guest's display refreshes at. The Mac's battery state is not read here.

use std::time::Duration;

/// How often [`follow`] reads Low Power Mode: often enough that the guest's display follows
/// within a second. Reading it is a property of the process's info.
pub const SAVER_POLL: Duration = Duration::from_millis(500);

/// A display mode's refresh rate as Core Graphics gives it, in whole hertz. Core Graphics
/// gives 0 for a display whose rate it does not name, which is no known rate.
pub fn hz_from_mode_rate(rate: f64) -> u32 {
    if !rate.is_finite() || rate < 1.0 {
        return 0;
    }
    rate.round().min(f64::from(u32::MAX)) as u32
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::sync::Weak;

    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    use dsa_host_core::guest_refresh::{fastest, MONITOR_POLL};
    use dsa_host_core::session::Session;

    use super::{hz_from_mode_rate, SAVER_POLL};

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGDisplayCopyDisplayMode(display: u32) -> *mut c_void;
        fn CGDisplayModeGetRefreshRate(mode: *mut c_void) -> f64;
        fn CGDisplayModeRelease(mode: *mut c_void);
    }

    /// More displays than any Mac drives.
    const MAX_DISPLAYS: usize = 32;

    /// The refresh rate of the Mac's fastest display, in hertz, or `None` when Core Graphics
    /// names no display's rate.
    pub fn fastest_display_hz() -> Option<u32> {
        let mut displays = [0u32; MAX_DISPLAYS];
        let mut count = 0u32;
        // SAFETY: the buffer holds MAX_DISPLAYS IDs and the count is a valid pointer.
        let error = unsafe {
            CGGetActiveDisplayList(MAX_DISPLAYS as u32, displays.as_mut_ptr(), &mut count)
        };
        if error != 0 {
            return None;
        }
        let rates = displays[..(count as usize).min(MAX_DISPLAYS)]
            .iter()
            .map(|&display| {
                // SAFETY: a display ID Core Graphics just listed; the copied mode is released.
                unsafe {
                    let mode = CGDisplayCopyDisplayMode(display);
                    if mode.is_null() {
                        return 0;
                    }
                    let rate = CGDisplayModeGetRefreshRate(mode);
                    CGDisplayModeRelease(mode);
                    hz_from_mode_rate(rate)
                }
            });
        fastest(rates)
    }

    /// Whether Low Power Mode is on. False on a Mac or macOS without it.
    pub fn low_power_mode() -> bool {
        let Ok(class) = crate::objc::class(c"NSProcessInfo") else {
            return false;
        };
        // SAFETY: NSProcessInfo's shared instance and a BOOL property it has had since
        // macOS 12; one that does not respond is taken to be off.
        unsafe {
            let info: *mut AnyObject = msg_send![class, processInfo];
            if info.is_null() {
                return false;
            }
            let responds: bool = msg_send![
                info,
                respondsToSelector: objc2::sel!(isLowPowerModeEnabled)
            ];
            if !responds {
                return false;
            }
            let on: bool = msg_send![info, isLowPowerModeEnabled];
            on
        }
    }

    /// Hands Low Power Mode and the fastest display's rate to `session` until it is gone.
    /// Run it on a thread of its own.
    pub fn follow(session: Weak<Session>) {
        let displays_every = (MONITOR_POLL.as_millis() / SAVER_POLL.as_millis()).max(1) as u32;
        let mut tick = 0u32;
        loop {
            let saver = low_power_mode();
            let displays = (tick % displays_every == 0)
                .then(fastest_display_hz)
                .flatten();
            let Some(session) = session.upgrade() else {
                return;
            };
            session.set_battery_saver(saver);
            if let Some(hz) = displays {
                session.set_monitor_refresh(hz);
            }
            drop(session);
            tick = tick.wrapping_add(1);
            std::thread::sleep(SAVER_POLL);
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::{fastest_display_hz, follow, low_power_mode};

#[cfg(test)]
mod tests {
    use super::*;
    use dsa_host_core::guest_refresh::refresh_rate;

    #[test]
    fn fr10_a_display_mode_rate_is_whole_hertz_and_zero_is_unknown() {
        assert_eq!(hz_from_mode_rate(120.0), 120);
        assert_eq!(hz_from_mode_rate(59.94), 60);
        assert_eq!(hz_from_mode_rate(0.0), 0);
        assert_eq!(hz_from_mode_rate(f64::NAN), 0);
    }

    #[test]
    fn fr10_battery_saver_caps_the_guest_refresh_rate_in_low_power_mode() {
        let promotion = hz_from_mode_rate(120.0);
        assert_eq!(refresh_rate(promotion, true), 60);
        assert_eq!(refresh_rate(promotion, false), 120);
        assert_eq!(refresh_rate(hz_from_mode_rate(60.0), true), 60);
        assert!(SAVER_POLL < Duration::from_secs(1));
    }
}
