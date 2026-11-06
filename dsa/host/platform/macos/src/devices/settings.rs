//! The Mac's system settings that each user's Android follows: time zone, display languages,
//! regional format, 24-hour time, first day of the week, dark mode and accent color.

use dsa_host_core::system_settings::SystemSettings;

/// What macOS says, before it is turned into the guest's terms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MacSettings {
    /// `NSTimeZone.localTimeZone.name`.
    pub time_zone: String,
    /// `AppleLanguages`, preferred first.
    pub languages: Vec<String>,
    /// `NSLocale.currentLocale.localeIdentifier`, such as `en_US` or `en_US@rg=gbzzzz`.
    pub locale: String,
    /// The hour format `NSDateFormatter` picks for the locale's `j` template.
    pub hour_format: String,
    /// `NSCalendar.currentCalendar.firstWeekday`: 1 is Sunday.
    pub first_weekday: i64,
    /// `AppleInterfaceStyle` is `Dark`.
    pub dark: bool,
    /// `AppleAccentColor`: `None` when the user keeps the default (multicolor).
    pub accent: Option<i64>,
}

/// A macOS locale identifier as a BCP 47 tag: `en_US` becomes `en-US`, and a region
/// override (`@rg=gbzzzz`, which the regional format setting adds) becomes the region.
pub fn language_tag(identifier: &str) -> String {
    let (base, keywords) = identifier.split_once('@').unwrap_or((identifier, ""));
    let mut parts: Vec<String> = base
        .split(['_', '-'])
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    let region = keywords
        .split(';')
        .find_map(|kw| kw.strip_prefix("rg="))
        .filter(|rg| rg.len() >= 2 && rg.is_char_boundary(2))
        .map(|rg| rg[..2].to_ascii_uppercase());
    if let Some(region) = region {
        // The region is the last part with two letters or three digits after the language.
        match parts.iter().skip(1).rposition(|p| {
            (p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic()))
                || (p.len() == 3 && p.chars().all(|c| c.is_ascii_digit()))
        }) {
            Some(at) => parts[at + 1] = region,
            None => parts.push(region),
        }
    }
    parts.join("-")
}

/// The guest's first day of the week (1 Monday through 7 Sunday) for macOS's (1 Sunday
/// through 7 Saturday); 0 when macOS says something else.
pub fn first_day_of_week(first_weekday: i64) -> u32 {
    match first_weekday {
        1 => 7,
        2..=7 => (first_weekday - 1) as u32,
        _ => 0,
    }
}

/// Whether an `NSDateFormatter` hour format is a 24-hour clock.
pub fn is_24_hour(hour_format: &str) -> bool {
    // Pattern letters outside quotes: H and k are 24-hour; h and K are 12-hour.
    let mut quoted = false;
    for c in hour_format.chars() {
        match c {
            '\'' => quoted = !quoted,
            'H' | 'k' if !quoted => return true,
            'h' | 'K' if !quoted => return false,
            _ => {}
        }
    }
    false
}

/// The accent color as 0xAARRGGBB for macOS's `AppleAccentColor`. The default (multicolor)
/// is blue, as apps show it.
pub fn accent_color(accent: Option<i64>) -> u32 {
    match accent {
        Some(-1) => 0xFF8E_8E93, // graphite
        Some(0) => 0xFFFF_3B30,  // red
        Some(1) => 0xFFFF_9500,  // orange
        Some(2) => 0xFFFF_CC00,  // yellow
        Some(3) => 0xFF28_CD41,  // green
        Some(5) => 0xFFAF_52DE,  // purple
        Some(6) => 0xFFFF_2D55,  // pink
        _ => 0xFF00_7AFF,        // blue
    }
}

/// What the guest is given for `mac`, checked.
pub fn system_settings(mac: &MacSettings) -> SystemSettings {
    let locales: Vec<String> = mac.languages.iter().map(|l| language_tag(l)).collect();
    let format = language_tag(&mac.locale);
    let regional_format = if locales.first() == Some(&format) {
        String::new()
    } else {
        format
    };
    SystemSettings {
        time_zone: mac.time_zone.clone(),
        locales,
        regional_format,
        time_24_hour: is_24_hour(&mac.hour_format),
        first_day_of_week: first_day_of_week(mac.first_weekday),
        dark_mode: mac.dark,
        accent_color: accent_color(mac.accent),
    }
    .checked()
}

#[cfg(target_os = "macos")]
pub use source::MacSettingsSource;

#[cfg(target_os = "macos")]
mod source {
    use std::time::{Duration, Instant};

    use dsa_host_core::system_settings::{SystemSettings, SystemSettingsSource};
    use objc2::msg_send;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;

    use super::{system_settings, MacSettings};
    use crate::objc::{class, ns_string, string_of};

    /// How often a change is looked for: macOS posts its notices of these changes to the
    /// distributed notification center of a run loop the service does not run.
    const POLL: Duration = Duration::from_secs(1);

    unsafe fn read_mac() -> MacSettings {
        let mut mac = MacSettings::default();
        let Ok(defaults_class) = class(c"NSUserDefaults") else {
            return mac;
        };
        // A fresh read of the global domain, not the values cached at the process start.
        let defaults: *mut AnyObject = msg_send![defaults_class, standardUserDefaults];
        let _: bool = msg_send![defaults, synchronize];
        if let Ok(tz) = class(c"NSTimeZone") {
            let _: () = msg_send![tz, resetSystemTimeZone];
            let zone: *mut AnyObject = msg_send![tz, localTimeZone];
            mac.time_zone = string_of(msg_send![zone, name]);
        }
        let languages: *mut AnyObject =
            msg_send![defaults, stringArrayForKey: &*ns_string("AppleLanguages")];
        if !languages.is_null() {
            let count: usize = msg_send![languages, count];
            for i in 0..count {
                mac.languages
                    .push(string_of(msg_send![languages, objectAtIndex: i]));
            }
        }
        if let Ok(locale_class) = class(c"NSLocale") {
            let locale: *mut AnyObject = msg_send![locale_class, autoupdatingCurrentLocale];
            mac.locale = string_of(msg_send![locale, localeIdentifier]);
            if let Ok(formatter) = class(c"NSDateFormatter") {
                let format: *mut AnyObject = msg_send![
                    formatter,
                    dateFormatFromTemplate: &*ns_string("j"),
                    options: 0usize,
                    locale: locale
                ];
                mac.hour_format = string_of(format);
            }
        }
        if let Ok(calendar_class) = class(c"NSCalendar") {
            let calendar: *mut AnyObject = msg_send![calendar_class, autoupdatingCurrentCalendar];
            let first: usize = msg_send![calendar, firstWeekday];
            mac.first_weekday = first as i64;
        }
        let style: *mut AnyObject =
            msg_send![defaults, stringForKey: &*ns_string("AppleInterfaceStyle")];
        mac.dark = string_of(style).eq_ignore_ascii_case("dark");
        let accent_key = ns_string("AppleAccentColor");
        let accent: *mut AnyObject = msg_send![defaults, objectForKey: &*accent_key];
        if !accent.is_null() {
            let value: isize = msg_send![defaults, integerForKey: &*accent_key];
            mac.accent = Some(value as i64);
        }
        mac
    }

    /// Reads the Mac's settings for the platform-neutral settings bridge.
    #[derive(Default)]
    pub struct MacSettingsSource {
        last: Option<SystemSettings>,
    }

    impl MacSettingsSource {
        pub fn new() -> MacSettingsSource {
            MacSettingsSource::default()
        }
    }

    impl SystemSettingsSource for MacSettingsSource {
        fn read(&mut self) -> SystemSettings {
            let settings = autoreleasepool(|_| system_settings(&unsafe { read_mac() }));
            self.last = Some(settings.clone());
            settings
        }

        fn wait_for_change(&mut self, timeout: Duration) -> bool {
            let until = Instant::now() + timeout;
            loop {
                let now = autoreleasepool(|_| system_settings(&unsafe { read_mac() }));
                if self.last.as_ref() != Some(&now) {
                    return true;
                }
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return false;
                }
                std::thread::sleep(left.min(POLL));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr30_locale_identifiers_read_as_language_tags() {
        assert_eq!(language_tag("en_US"), "en-US");
        assert_eq!(language_tag("ko-KR"), "ko-KR");
        assert_eq!(language_tag("zh-Hans-CN"), "zh-Hans-CN");
        assert_eq!(language_tag("en_US@rg=gbzzzz"), "en-GB");
        assert_eq!(language_tag("en@rg=dezzzz"), "en-DE");
        assert_eq!(language_tag("es_419"), "es-419");
    }

    #[test]
    fn fr30_the_week_starts_on_the_same_day_as_on_the_mac() {
        assert_eq!(first_day_of_week(1), 7);
        assert_eq!(first_day_of_week(2), 1);
        assert_eq!(first_day_of_week(7), 6);
        assert_eq!(first_day_of_week(0), 0);
    }

    #[test]
    fn fr30_the_clock_follows_the_locales_hour_format() {
        assert!(is_24_hour("HH"));
        assert!(!is_24_hour("h a"));
        assert!(!is_24_hour("'H' h a"));
        assert!(is_24_hour("k"));
    }

    #[test]
    fn fr30_the_accent_color_follows_the_macs() {
        assert_eq!(accent_color(None), 0xFF00_7AFF);
        assert_eq!(accent_color(Some(0)), 0xFFFF_3B30);
        assert_eq!(accent_color(Some(-1)), 0xFF8E_8E93);
        assert_eq!(accent_color(Some(42)), 0xFF00_7AFF);
    }

    #[test]
    fn fr30_mac_settings_reach_the_guest_in_its_terms() {
        let settings = system_settings(&MacSettings {
            time_zone: "Asia/Seoul".into(),
            languages: vec!["ko-KR".into(), "en-US".into()],
            locale: "ko_KR".into(),
            hour_format: "a h시".into(),
            first_weekday: 1,
            dark: true,
            accent: Some(5),
        });
        assert_eq!(settings.time_zone, "Asia/Seoul");
        assert_eq!(settings.locales, vec!["ko-KR", "en-US"]);
        // The format is the first language's, so it is not repeated.
        assert_eq!(settings.regional_format, "");
        assert!(!settings.time_24_hour);
        assert_eq!(settings.first_day_of_week, 7);
        assert!(settings.dark_mode);
        assert_eq!(settings.accent_color, 0xFFAF_52DE);

        let british = system_settings(&MacSettings {
            languages: vec!["en-US".into()],
            locale: "en_US@rg=gbzzzz".into(),
            ..MacSettings::default()
        });
        assert_eq!(british.regional_format, "en-GB");
    }
}
