//! The pages of the app, in the order the sidebar lists them. They are the pages of the
//! Windows settings app, so each platform's app shows the same items.

/// One page: what the sidebar calls it and its symbolic icon from the Adwaita icon theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Memory,
    Apps,
    Background,
    Widgets,
    Display,
    Identity,
    Network,
    /// DSA itself and the Android image: the version that runs, the version it follows, its
    /// update and the version choice.
    Updates,
    Advanced,
    Logs,
    About,
}

impl Page {
    pub const ALL: [Page; 12] = [
        Page::General,
        Page::Memory,
        Page::Apps,
        Page::Background,
        Page::Widgets,
        Page::Display,
        Page::Identity,
        Page::Network,
        Page::Updates,
        Page::Advanced,
        Page::Logs,
        Page::About,
    ];

    /// The page's title, in English; the sidebar shows it through gettext.
    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Memory => "Memory",
            Page::Apps => "Apps",
            Page::Background => "Running in the background",
            Page::Widgets => "Widgets",
            Page::Display => "Display and GPU",
            Page::Updates => "Updates",
            Page::Identity => "Device identity",
            Page::Network => "Network",
            Page::Advanced => "Advanced",
            Page::Logs => "Logs",
            Page::About => "About",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Page::General => "go-home-symbolic",
            Page::Memory => "drive-harddisk-solidstate-symbolic",
            Page::Apps => "view-app-grid-symbolic",
            Page::Background => "media-playback-start-symbolic",
            Page::Widgets => "view-grid-symbolic",
            Page::Display => "video-display-symbolic",
            Page::Updates => "software-update-available-symbolic",
            Page::Identity => "contact-new-symbolic",
            Page::Network => "network-wired-symbolic",
            Page::Advanced => "preferences-system-symbolic",
            Page::Logs => "text-x-generic-symbolic",
            Page::About => "help-about-symbolic",
        }
    }

    /// The name `--page` takes, to open the app on this page.
    pub fn id(self) -> &'static str {
        match self {
            Page::General => "general",
            Page::Memory => "memory",
            Page::Apps => "apps",
            Page::Background => "background",
            Page::Widgets => "widgets",
            Page::Display => "display",
            Page::Updates => "updates",
            Page::Identity => "identity",
            Page::Network => "network",
            Page::Advanced => "advanced",
            Page::Logs => "logs",
            Page::About => "about",
        }
    }

    pub fn from_id(id: &str) -> Option<Page> {
        // The Android page is part of Updates now; its old name still opens it.
        if id == "android" {
            return Some(Page::Updates);
        }
        Page::ALL.into_iter().find(|page| page.id() == id)
    }
}

/// The app `--app` names in `args`, whose row the Apps page opens; `None` when there is none
/// or it names no package. A task window's App info starts the app as
/// `dsa-settings --page apps --app com.example.notes`.
pub fn app_of(args: &[String]) -> Option<String> {
    let at = args.iter().position(|arg| arg == "--app")?;
    let package = args.get(at + 1)?;
    dsa_host_core::compositor::window_hosts::is_valid_package(package).then(|| package.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn fr22_app_info_opens_the_apps_page_at_the_app() {
        let start = args(&["--page", "apps", "--app", "com.example.notes"]);
        assert_eq!(Page::from_id("apps"), Some(Page::Apps));
        assert_eq!(app_of(&start), Some("com.example.notes".into()));
        assert_eq!(app_of(&args(&["--app"])), None);
        assert_eq!(app_of(&args(&["--app", "../evil"])), None);
        assert_eq!(app_of(&args(&["--app", "notes"])), None);
        assert_eq!(app_of(&args(&["--page", "apps"])), None);
    }

    #[test]
    fn fr17_updates_holds_android_and_sits_directly_above_advanced() {
        let updates = Page::ALL.iter().position(|p| *p == Page::Updates).unwrap();
        assert_eq!(Page::ALL[updates + 1], Page::Advanced);
        assert_eq!(Page::ALL[updates - 1], Page::Network);
        assert_eq!(Page::from_id("android"), Some(Page::Updates));
    }

    #[test]
    fn fr17_the_log_page_is_called_logs_and_sits_directly_above_about() {
        assert_eq!(Page::Logs.title(), "Logs");
        let logs = Page::ALL.iter().position(|p| *p == Page::Logs).unwrap();
        assert_eq!(Page::ALL[logs + 1], Page::About);
        assert_eq!(Page::ALL.last(), Some(&Page::About));
    }
}
