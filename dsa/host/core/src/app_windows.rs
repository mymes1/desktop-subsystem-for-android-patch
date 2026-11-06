//! A user's window settings for each app: whether the app's windows may be resized, and the
//! size they open at.
//!
//! The settings app's Apps page and `dsa app-window` change them through the local API; the
//! service keeps them beside the user's other settings, and the user's window manager reads
//! them when an app's window opens. An app with the defaults (resizable, opening at the size
//! Android gives its task) has no entry.

use dsa_protocol::host_api::types;
use serde::{Deserialize, Serialize};

use crate::compositor::window_sizing::MIN_CLIENT_DP;

/// The largest start size accepted, in density-independent pixels in each direction: more
/// than any monitor shows at 100%.
pub const MAX_START_DP: u32 = 8192;

/// The most apps with settings kept for one user.
pub const MAX_APPS: usize = 4096;

/// One app's window settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppWindowSettings {
    pub package_name: String,
    /// Whether the app's windows keep the size they open at.
    #[serde(default)]
    pub fixed_size: bool,
    /// The size the app's windows open at, in density-independent pixels; `None` opens them
    /// at the size Android gives the task.
    #[serde(default)]
    pub start_size_dp: Option<(u32, u32)>,
}

impl AppWindowSettings {
    /// Whether these are the defaults, which need no entry.
    pub fn is_default(&self) -> bool {
        !self.fixed_size && self.start_size_dp.is_none()
    }

    /// Checks the settings, as a client sent them: the package must be an Android package
    /// name, and a start size at least the smallest window and at most [`MAX_START_DP`].
    pub fn checked(self) -> Result<AppWindowSettings, String> {
        if !crate::compositor::window_hosts::is_valid_package(&self.package_name) {
            return Err(format!(
                "{:?} is not an Android package name",
                self.package_name
            ));
        }
        if let Some((width, height)) = self.start_size_dp {
            let fits = |length: u32| (MIN_CLIENT_DP..=MAX_START_DP).contains(&length);
            if !fits(width) || !fits(height) {
                return Err(format!(
                    "a start size of {width} x {height} is not between {MIN_CLIENT_DP} and \
                     {MAX_START_DP} in each direction"
                ));
            }
        }
        Ok(self)
    }

    /// The settings a local API message carries; a start size of 0 by 0 is none.
    pub fn from_wire(app: &types::AppWindowSettings) -> AppWindowSettings {
        AppWindowSettings {
            package_name: app.package_name.clone(),
            fixed_size: app.fixed_size,
            start_size_dp: (app.start_width_dp > 0 || app.start_height_dp > 0)
                .then_some((app.start_width_dp, app.start_height_dp)),
        }
    }

    /// The local API message that carries these settings.
    pub fn to_wire(&self) -> types::AppWindowSettings {
        let (start_width_dp, start_height_dp) = self.start_size_dp.unwrap_or((0, 0));
        types::AppWindowSettings {
            package_name: self.package_name.clone(),
            fixed_size: self.fixed_size,
            start_width_dp,
            start_height_dp,
        }
    }
}

/// `apps` with `app` in place of its package's entry: removed when `app` has the defaults,
/// otherwise added or replaced. The list stays sorted by package name.
pub fn replace(
    apps: &[AppWindowSettings],
    app: AppWindowSettings,
) -> Result<Vec<AppWindowSettings>, String> {
    let mut list: Vec<AppWindowSettings> = apps
        .iter()
        .filter(|known| known.package_name != app.package_name)
        .cloned()
        .collect();
    if !app.is_default() {
        if list.len() >= MAX_APPS {
            return Err(format!(
                "window settings are kept for {MAX_APPS} apps at most; reset another app's first"
            ));
        }
        list.push(app);
    }
    list.sort_by(|a, b| a.package_name.cmp(&b.package_name));
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes(fixed_size: bool, start: Option<(u32, u32)>) -> AppWindowSettings {
        AppWindowSettings {
            package_name: "com.example.notes".into(),
            fixed_size,
            start_size_dp: start,
        }
    }

    #[test]
    fn fr30_an_apps_settings_replace_its_entry_and_the_defaults_remove_it() {
        let list = replace(&[], notes(true, None)).unwrap();
        assert_eq!(list, vec![notes(true, None)]);
        let list = replace(&list, notes(false, Some((400, 700)))).unwrap();
        assert_eq!(list, vec![notes(false, Some((400, 700)))]);
        assert!(replace(&list, notes(false, None)).unwrap().is_empty());
    }

    #[test]
    fn fr30_a_start_size_must_be_one_a_window_can_have() {
        assert!(notes(false, Some((400, 700))).checked().is_ok());
        assert!(notes(false, Some((100, 700))).checked().is_err());
        assert!(notes(false, Some((400, 9000))).checked().is_err());
        let bad = AppWindowSettings {
            package_name: "..\\evil".into(),
            ..notes(true, None)
        };
        assert!(bad.checked().is_err());
    }

    #[test]
    fn fr30_a_start_size_of_zero_on_the_wire_is_none() {
        let wire = notes(true, None).to_wire();
        assert_eq!((wire.start_width_dp, wire.start_height_dp), (0, 0));
        assert_eq!(AppWindowSettings::from_wire(&wire), notes(true, None));
        let wire = notes(false, Some((500, 800))).to_wire();
        assert_eq!(
            AppWindowSettings::from_wire(&wire),
            notes(false, Some((500, 800)))
        );
    }
}
