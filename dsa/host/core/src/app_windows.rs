//! A user's window settings for each app: whether the app's windows may be resized, the size
//! they open at, and how the app follows its window to a monitor of another scale.
//!
//! The settings app's Apps page and `dsa app-window` change them through the local API; the
//! service keeps them beside the user's other settings, and the user's window manager reads
//! them when an app's window opens. An app with the defaults (resizable, opening at the size
//! Android gives its task, scaling its own way) has no entry.

use dsa_protocol::host_api::types;
use serde::{Deserialize, Serialize};

use crate::compositor::window_sizing::MIN_CLIENT_DP;

/// How an app follows its window to a monitor of another scale, as a user chooses it, the way
/// Windows lets a user choose who performs the high DPI scaling of a desktop program. The
/// default and [`DpiScaling::System`] never restart the app.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DpiScaling {
    /// [`DpiScaling::App`] for an app whose activity says it handles a change of density
    /// itself, [`DpiScaling::System`] for any other.
    #[default]
    Auto,
    /// The task takes the new monitor's density, and the app redraws at it: its text stays
    /// sharp. An app that does not say it handles a change of density is given it too: Android
    /// recreates its activity, which restores its saved state (scroll position, typed text),
    /// so it may redraw briefly when it moves, as a Windows program does with "application".
    App,
    /// The task keeps its density, and the window manager scales the window's picture to the
    /// new monitor, so the app keeps its size on the screen. Scaled up, text can look a little
    /// soft.
    System,
    /// The app is given the new density without a restart, even though it does not say it
    /// handles one. That needs a change to Android's framework that is not made yet, so until
    /// then it works as [`DpiScaling::System`] does ([`DpiScaling::FORCED_AVAILABLE`]).
    SystemForced,
}

/// What a task's window does on a monitor of another scale, once the user's choice and the
/// app's own say are taken together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalingMode {
    /// The task takes each monitor's density.
    App,
    /// The task keeps its density and the window is scaled.
    System,
}

impl DpiScaling {
    /// Whether [`DpiScaling::SystemForced`] does what it says yet. It needs a framework change
    /// that is not approved, so it does not.
    pub const FORCED_AVAILABLE: bool = false;

    /// Every choice, in the order the settings apps list them.
    pub const ALL: [DpiScaling; 4] = [
        DpiScaling::Auto,
        DpiScaling::App,
        DpiScaling::System,
        DpiScaling::SystemForced,
    ];

    /// The name `dsa app-window --dpi-scaling` takes and prints.
    pub fn name(self) -> &'static str {
        match self {
            DpiScaling::Auto => "auto",
            DpiScaling::App => "app",
            DpiScaling::System => "system",
            DpiScaling::SystemForced => "system-forced",
        }
    }

    /// The choice `name` names, as [`DpiScaling::name`] gives it.
    pub fn from_name(name: &str) -> Option<DpiScaling> {
        DpiScaling::ALL
            .into_iter()
            .find(|scaling| scaling.name() == name)
    }

    /// What a task's window does with this choice, for a task whose top activity does
    /// (`handles_density_change`) or does not say it handles a change of density itself.
    ///
    /// Android relaunches an activity that does not say it handles a change of density when
    /// its density changes, and the default must never restart an app. So
    /// [`DpiScaling::Auto`] gives another density only to an activity that says it handles
    /// one. [`DpiScaling::App`] is the user's explicit choice to stay sharp at the cost of
    /// that relaunch, so it gives the new density to any app. [`DpiScaling::SystemForced`]
    /// works as [`DpiScaling::System`] does until the framework change it needs is made.
    pub fn mode(self, handles_density_change: bool) -> ScalingMode {
        match self {
            DpiScaling::App => ScalingMode::App,
            DpiScaling::Auto if handles_density_change => ScalingMode::App,
            DpiScaling::SystemForced if DpiScaling::FORCED_AVAILABLE => ScalingMode::App,
            DpiScaling::Auto | DpiScaling::System | DpiScaling::SystemForced => ScalingMode::System,
        }
    }

    fn from_wire(value: i32) -> DpiScaling {
        match types::DpiScaling::try_from(value) {
            Ok(types::DpiScaling::App) => DpiScaling::App,
            Ok(types::DpiScaling::System) => DpiScaling::System,
            Ok(types::DpiScaling::SystemForced) => DpiScaling::SystemForced,
            Ok(types::DpiScaling::Auto) | Err(_) => DpiScaling::Auto,
        }
    }

    fn to_wire(self) -> i32 {
        match self {
            DpiScaling::Auto => types::DpiScaling::Auto,
            DpiScaling::App => types::DpiScaling::App,
            DpiScaling::System => types::DpiScaling::System,
            DpiScaling::SystemForced => types::DpiScaling::SystemForced,
        }
        .into()
    }
}

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
    /// How the app follows its window to a monitor of another scale.
    #[serde(default)]
    pub dpi_scaling: DpiScaling,
}

impl AppWindowSettings {
    /// Whether these are the defaults, which need no entry.
    pub fn is_default(&self) -> bool {
        !self.fixed_size && self.start_size_dp.is_none() && self.dpi_scaling == DpiScaling::Auto
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
            dpi_scaling: DpiScaling::from_wire(app.dpi_scaling),
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
            dpi_scaling: self.dpi_scaling.to_wire(),
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
            ..AppWindowSettings::default()
        }
    }

    #[test]
    fn fr4_an_explicit_app_choice_always_takes_the_density_and_forced_works_as_system_for_now() {
        assert_eq!(DpiScaling::Auto.mode(true), ScalingMode::App);
        assert_eq!(DpiScaling::Auto.mode(false), ScalingMode::System);
        assert_eq!(DpiScaling::App.mode(true), ScalingMode::App);
        // The user chose to stay sharp: Android recreates the activity and restores its state.
        assert_eq!(DpiScaling::App.mode(false), ScalingMode::App);
        assert_eq!(DpiScaling::System.mode(false), ScalingMode::System);
        assert_eq!(DpiScaling::System.mode(true), ScalingMode::System);
        // It needs a framework change that is not made yet.
        const { assert!(!DpiScaling::FORCED_AVAILABLE) };
        assert_eq!(DpiScaling::SystemForced.mode(false), ScalingMode::System);
        assert_eq!(DpiScaling::SystemForced.mode(true), ScalingMode::System);
    }

    #[test]
    fn fr4_the_dpi_scaling_choice_is_stored_with_the_other_window_settings() {
        for scaling in DpiScaling::ALL {
            assert_eq!(DpiScaling::from_name(scaling.name()), Some(scaling));
            let app = AppWindowSettings {
                dpi_scaling: scaling,
                ..notes(false, None)
            };
            assert_eq!(AppWindowSettings::from_wire(&app.to_wire()), app);
            let stored = serde_json::to_string(&app).unwrap();
            assert_eq!(
                serde_json::from_str::<AppWindowSettings>(&stored).unwrap(),
                app
            );
            // Any choice but the default is an entry of its own.
            assert_eq!(app.is_default(), scaling == DpiScaling::Auto);
        }
        assert_eq!(DpiScaling::from_name("enhanced"), None);
        // Settings stored before the choice existed read as auto.
        let old: AppWindowSettings =
            serde_json::from_str(r#"{"package_name":"com.example.notes","fixed_size":true}"#)
                .unwrap();
        assert_eq!(old.dpi_scaling, DpiScaling::Auto);
        assert_eq!(
            serde_json::to_value(DpiScaling::SystemForced).unwrap(),
            "system-forced"
        );
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
