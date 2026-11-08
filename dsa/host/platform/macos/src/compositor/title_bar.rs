//! What a task window's title bar does on a Mac, apart from AppKit: the band an app that
//! draws its own title bar is told about, which points of that band drag the window, what a
//! double click on it does, and the back button's keys and name.
//!
//! macOS draws a task window's title bar itself, with the standard window buttons (close,
//! minimize, zoom) at its leading edge, the app's name in the middle and no icon: a Mac
//! window's title bar shows an icon only for a document, and the Dock and the menu bar
//! already say which app a window belongs to. While the task can go back, a back button
//! (a `chevron.backward` symbol) stands after the window buttons.
//!
//! An app that draws its own title bar gets a transparent, full-size content title bar: its
//! frames run under the band, and only the window buttons and the back button stay over it.
//! The guest is told how high the band is and how far those buttons reach in from each of
//! its ends ([`caption_band`]), so the app keeps its own controls clear of them. In the band,
//! the app's controls take the pointer and the rest drags the window ([`band_hit`]).

use dsa_host_core::compositor::caption::CaptionBand;
use dsa_host_core::keys::{keycode, KeyAction, KeyEvent};
use dsa_host_core::tasks::Bounds;

/// The space left after the window's own buttons in the band, in points, so an app's
/// controls never touch them. A Mac app's toolbar starts about this far after the zoom
/// button.
pub const ITEM_GAP_POINTS: f64 = 8.0;

/// The band an app drawing its own title bar is told about: `height` points high, in a
/// window `width` points wide, at `scale` pixels a point. `items` are the left and right
/// edges (points from the window's left) of what macOS keeps over the band: the window
/// buttons and the back button. An item in the left half of the band counts toward
/// `start_width`, how far in from the band's left end it reaches; one in the right half
/// toward `end_width`, from the right end. The window buttons stand at the left on a Mac (at
/// the right in a right-to-left language), the reverse of Windows, so the guest is told the
/// sides as they are. A window that shows no band (full screen hides it) gets an empty one.
pub fn caption_band(width: f64, height: f64, items: &[(f64, f64)], scale: f64) -> CaptionBand {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    if !(width.is_finite() && width > 0.0 && height.is_finite() && height > 0.0) {
        return CaptionBand::default();
    }
    let middle = width / 2.0;
    let (mut start, mut end) = (0.0f64, 0.0f64);
    for &(left, right) in items {
        if !(left.is_finite() && right.is_finite()) || right <= left {
            continue;
        }
        if (left + right) / 2.0 < middle {
            start = start.max(right.max(0.0) + ITEM_GAP_POINTS);
        } else {
            end = end.max((width - left).max(0.0) + ITEM_GAP_POINTS);
        }
    }
    let pixels = |points: f64| (points.min(width) * scale).ceil() as u32;
    CaptionBand {
        height: (height * scale).round() as u32,
        start_width: pixels(start),
        end_width: pixels(end),
    }
}

/// What a press in a task window's content goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BandHit {
    /// The app: the press goes to the guest.
    App,
    /// The title bar band outside the app's controls: the press drags the window, a double
    /// click does what the user set for a title bar, and a secondary click opens the window's
    /// menu.
    Drag,
}

/// What the client pixel `point` (from the content's top left) goes to, in a window whose
/// app draws its own title bar over the top `band_height` pixels with its controls at
/// `controls` (client pixels). Below the band, and with no band, everything is the app's.
pub fn band_hit(point: (f64, f64), band_height: u32, controls: &[Bounds]) -> BandHit {
    let (x, y) = point;
    if !(y >= 0.0 && y < f64::from(band_height)) {
        return BandHit::App;
    }
    let on_control = controls.iter().any(|c| {
        x >= f64::from(c.left)
            && x < f64::from(c.right)
            && y >= f64::from(c.top)
            && y < f64::from(c.bottom)
    });
    if on_control {
        BandHit::App
    } else {
        BandHit::Drag
    }
}

/// What a double click on a title bar does, as the user chose in System Settings (Desktop &
/// Dock, "Double-click a window's title bar to").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DoubleClick {
    Zoom,
    Minimize,
    Nothing,
}

/// The double click action for the global `AppleActionOnDoubleClick` default (`Maximize`,
/// `Minimize`, `None`, or `Fill` on newer macOS), falling back to the older
/// `AppleMiniaturizeOnDoubleClick` when it is not set. Zoom is macOS's default.
pub fn double_click_action(action: Option<&str>, minimize_legacy: bool) -> DoubleClick {
    match action.map(str::trim) {
        Some(a) if a.eq_ignore_ascii_case("minimize") => DoubleClick::Minimize,
        Some(a) if a.eq_ignore_ascii_case("none") => DoubleClick::Nothing,
        Some(a) if !a.is_empty() => DoubleClick::Zoom,
        _ if minimize_legacy => DoubleClick::Minimize,
        _ => DoubleClick::Zoom,
    }
}

/// The key events a click on the back button sends the task: Back pressed and released, at
/// `time_ms` of the host's clock.
pub fn back_keys(task_id: i32, time_ms: u32) -> [KeyEvent; 2] {
    let key = |action| KeyEvent {
        task_id,
        action,
        key_code: keycode::BACK,
        repeat_count: 0,
        meta_state: 0,
        event_time_ms: time_ms,
        canceled: false,
    };
    [key(KeyAction::Down), key(KeyAction::Up)]
}

/// Whether the user's first preferred language (`NSLocale.preferredLanguages`, such as
/// `ko-KR`) is Korean, the one language besides English the host's words come in.
pub fn is_korean(language: &str) -> bool {
    let language = language.trim();
    language.eq_ignore_ascii_case("ko")
        || language
            .get(..3)
            .is_some_and(|p| p.eq_ignore_ascii_case("ko-") || p.eq_ignore_ascii_case("ko_"))
}

/// The back button's name for VoiceOver and its tooltip, in the user's language.
pub fn back_label(korean: bool) -> &'static str {
    if korean {
        "뒤로"
    } else {
        "Back"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr22_the_band_reaches_past_the_window_buttons_from_the_left() {
        // The three window buttons from 7 to 67 points, in a 600 point window at 2x.
        let band = caption_band(600.0, 28.0, &[(7.0, 21.0), (27.0, 41.0), (47.0, 61.0)], 2.0);
        assert_eq!(band.height, 56);
        assert_eq!(band.start_width, ((61.0 + ITEM_GAP_POINTS) * 2.0) as u32);
        assert_eq!(band.end_width, 0);
    }

    #[test]
    fn fr22_the_back_button_widens_the_bands_left_end() {
        let buttons = [(7.0, 21.0), (27.0, 41.0), (47.0, 61.0)];
        let without = caption_band(600.0, 28.0, &buttons, 1.0);
        let mut with_back = buttons.to_vec();
        with_back.push((68.0, 96.0));
        let with = caption_band(600.0, 28.0, &with_back, 1.0);
        assert_eq!(with.height, without.height);
        assert_eq!(with.start_width, (96.0 + ITEM_GAP_POINTS) as u32);
        assert!(with.start_width > without.start_width);
    }

    #[test]
    fn fr22_right_to_left_buttons_count_from_the_right_end() {
        let band = caption_band(600.0, 28.0, &[(539.0, 553.0), (579.0, 593.0)], 1.0);
        assert_eq!(band.start_width, 0);
        assert_eq!(band.end_width, (600.0 - 539.0 + ITEM_GAP_POINTS) as u32);
    }

    #[test]
    fn fr22_a_window_without_a_band_tells_the_app_none() {
        assert_eq!(
            caption_band(600.0, 0.0, &[(7.0, 21.0)], 2.0),
            CaptionBand::default()
        );
        assert_eq!(
            caption_band(600.0, f64::NAN, &[(7.0, 21.0)], 2.0),
            CaptionBand::default()
        );
        // A bad scale counts as 1x rather than as no band.
        assert_eq!(caption_band(600.0, 28.0, &[], 0.0).height, 28);
    }

    #[test]
    fn fr22_the_apps_controls_take_the_pointer_and_the_rest_of_the_band_drags() {
        let tab = Bounds {
            left: 200,
            top: 0,
            right: 400,
            bottom: 56,
        };
        assert_eq!(band_hit((300.0, 20.0), 56, &[tab]), BandHit::App);
        assert_eq!(band_hit((450.0, 20.0), 56, &[tab]), BandHit::Drag);
        // Below the band the app has everything.
        assert_eq!(band_hit((450.0, 56.0), 56, &[tab]), BandHit::App);
        // No band: nothing drags.
        assert_eq!(band_hit((450.0, 0.0), 0, &[]), BandHit::App);
    }

    #[test]
    fn fr22_a_double_click_on_the_band_does_what_the_user_set() {
        assert_eq!(double_click_action(None, false), DoubleClick::Zoom);
        assert_eq!(
            double_click_action(Some("Maximize"), true),
            DoubleClick::Zoom
        );
        assert_eq!(double_click_action(Some("Fill"), false), DoubleClick::Zoom);
        assert_eq!(
            double_click_action(Some("Minimize"), false),
            DoubleClick::Minimize
        );
        assert_eq!(
            double_click_action(Some("None"), false),
            DoubleClick::Nothing
        );
        assert_eq!(double_click_action(None, true), DoubleClick::Minimize);
    }

    #[test]
    fn fr22_the_back_button_sends_the_task_back_down_and_up() {
        let [down, up] = back_keys(7, 1234);
        assert_eq!((down.task_id, down.key_code), (7, keycode::BACK));
        assert_eq!(down.action, KeyAction::Down);
        assert_eq!(up.action, KeyAction::Up);
        assert_eq!(up.key_code, keycode::BACK);
        assert_eq!((down.event_time_ms, up.event_time_ms), (1234, 1234));
        assert!(!down.canceled && !up.canceled);
    }

    #[test]
    fn fr22_the_back_button_is_named_in_the_users_language() {
        assert!(is_korean("ko-KR"));
        assert!(is_korean("ko"));
        assert!(is_korean("ko_KR"));
        assert!(!is_korean("en-US"));
        assert!(!is_korean("kok"));
        assert_eq!(back_label(true), "뒤로");
        assert_eq!(back_label(false), "Back");
    }
}
