//! `dsa apps desktop-shortcut <package>`: a shortcut to an app on the user's desktop, as the
//! desktop operating system makes them, which the settings apps' "Create desktop shortcut"
//! runs. It starts the app as its Start menu entry (launcher entry, Launchpad bundle) does,
//! and it goes when the app is uninstalled.

use std::path::PathBuf;

use dsa_host_core::tasks::AppInfo;

/// Puts a shortcut to `app` on the desktop; returns where it is.
pub fn place_on_desktop(app: &AppInfo) -> Result<PathBuf, String> {
    let _ = app;
    Err("desktop shortcuts are not made on this computer's system".into())
}
