//! The menu bar of an app's window host: the app menu and the Window menu.
//!
//! A Mac app's window commands live in its Window menu, so that is where the items of an
//! Android desktop window's window menu go. After macOS's own items (Minimize, Zoom) and a
//! separator come New Window (only for an app that supports several windows), Screenshot,
//! Aspect Ratio, Open by Default and App Info, then another separator, Bring All to Front,
//! and the list of the app's windows, which AppKit keeps. The same items open from a
//! secondary click on the band of a window whose app draws its own title bar, where the
//! window has no title of macOS's own to click.
//!
//! Each Android item acts on the app's front window. The window host takes the screenshot
//! itself, since it has the window's frames; every other item goes to the window manager as
//! `FromHost::MenuChosen`.
//!
//! The words are Korean when the user's first preferred language is Korean, else English.

use dsa_host_core::compositor::window_hosts::MenuAction;

/// One entry of the Window menu, before the window list AppKit adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowMenuItem {
    Separator,
    Minimize,
    Zoom,
    NewWindow,
    Screenshot,
    AspectRatio,
    OpenByDefault,
    AppInfo,
    BringAllToFront,
}

/// The Window menu's entries, in order. New Window is left out for an app that does not
/// support several windows, since its second task would only bring the first to the front.
pub fn window_menu(new_window: bool) -> Vec<WindowMenuItem> {
    let mut items = vec![
        WindowMenuItem::Minimize,
        WindowMenuItem::Zoom,
        WindowMenuItem::Separator,
    ];
    items.extend(android_items(new_window));
    items.extend([WindowMenuItem::Separator, WindowMenuItem::BringAllToFront]);
    items
}

/// The Android desktop window's items alone, as the band's secondary click shows them after
/// Minimize and Zoom.
pub fn android_items(new_window: bool) -> Vec<WindowMenuItem> {
    let mut items = Vec::new();
    if new_window {
        items.push(WindowMenuItem::NewWindow);
    }
    items.extend([
        WindowMenuItem::Screenshot,
        WindowMenuItem::AspectRatio,
        WindowMenuItem::OpenByDefault,
        WindowMenuItem::AppInfo,
    ]);
    items
}

/// The menu item tag that names an Android entry when it is chosen; 0 for macOS's own entries
/// and the separator, which AppKit carries out itself.
pub fn tag(item: WindowMenuItem) -> isize {
    match item {
        WindowMenuItem::NewWindow => 1,
        WindowMenuItem::Screenshot => 2,
        WindowMenuItem::AspectRatio => 3,
        WindowMenuItem::OpenByDefault => 4,
        WindowMenuItem::AppInfo => 5,
        _ => 0,
    }
}

/// The Android entry a chosen menu item's tag names.
pub fn from_tag(tag: isize) -> Option<WindowMenuItem> {
    [
        WindowMenuItem::NewWindow,
        WindowMenuItem::Screenshot,
        WindowMenuItem::AspectRatio,
        WindowMenuItem::OpenByDefault,
        WindowMenuItem::AppInfo,
    ]
    .into_iter()
    .find(|item| self::tag(*item) == tag)
}

/// What the window manager does for an entry; `None` for Screenshot, which the window host
/// takes, and for macOS's own entries.
pub fn action(item: WindowMenuItem) -> Option<MenuAction> {
    match item {
        WindowMenuItem::NewWindow => Some(MenuAction::NewWindow),
        WindowMenuItem::AspectRatio => Some(MenuAction::AspectRatio),
        WindowMenuItem::OpenByDefault => Some(MenuAction::OpenByDefault),
        WindowMenuItem::AppInfo => Some(MenuAction::AppInfo),
        _ => None,
    }
}

/// The AppKit action macOS's own entries send down the responder chain; `None` for the
/// separator and the Android entries.
pub fn system_action(item: WindowMenuItem) -> Option<&'static str> {
    match item {
        WindowMenuItem::Minimize => Some("performMiniaturize:"),
        WindowMenuItem::Zoom => Some("performZoom:"),
        WindowMenuItem::BringAllToFront => Some("arrangeInFront:"),
        _ => None,
    }
}

/// The key that chooses an entry with Command, as every Mac app has it; empty for none.
pub fn key_equivalent(item: WindowMenuItem) -> &'static str {
    match item {
        WindowMenuItem::Minimize => "m",
        _ => "",
    }
}

/// The menu bar's words in one language. `{app}` in an entry stands for the app's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuText {
    pub window: &'static str,
    pub minimize: &'static str,
    pub zoom: &'static str,
    pub new_window: &'static str,
    pub screenshot: &'static str,
    pub aspect_ratio: &'static str,
    pub open_by_default: &'static str,
    pub app_info: &'static str,
    pub bring_all_to_front: &'static str,
    pub hide_app: &'static str,
    pub hide_others: &'static str,
    pub show_all: &'static str,
    pub quit_app: &'static str,
}

/// The menu bar's words: Korean, as macOS's own menus say them, or English.
pub fn menu_text(korean: bool) -> MenuText {
    if korean {
        MenuText {
            window: "윈도우",
            minimize: "최소화",
            zoom: "확대/축소",
            new_window: "새 창",
            screenshot: "스크린샷",
            aspect_ratio: "화면 비율\u{2026}",
            open_by_default: "링크 열기 설정\u{2026}",
            app_info: "앱 정보",
            bring_all_to_front: "모두 앞으로 가져오기",
            hide_app: "{app} 가리기",
            hide_others: "기타 가리기",
            show_all: "모두 보기",
            quit_app: "{app} 종료",
        }
    } else {
        MenuText {
            window: "Window",
            minimize: "Minimize",
            zoom: "Zoom",
            new_window: "New Window",
            screenshot: "Screenshot",
            aspect_ratio: "Aspect Ratio\u{2026}",
            open_by_default: "Open by Default\u{2026}",
            app_info: "App Info",
            bring_all_to_front: "Bring All to Front",
            hide_app: "Hide {app}",
            hide_others: "Hide Others",
            show_all: "Show All",
            quit_app: "Quit {app}",
        }
    }
}

/// The label of a Window menu entry; `None` for the separator.
pub fn label(text: &MenuText, item: WindowMenuItem) -> Option<&'static str> {
    Some(match item {
        WindowMenuItem::Separator => return None,
        WindowMenuItem::Minimize => text.minimize,
        WindowMenuItem::Zoom => text.zoom,
        WindowMenuItem::NewWindow => text.new_window,
        WindowMenuItem::Screenshot => text.screenshot,
        WindowMenuItem::AspectRatio => text.aspect_ratio,
        WindowMenuItem::OpenByDefault => text.open_by_default,
        WindowMenuItem::AppInfo => text.app_info,
        WindowMenuItem::BringAllToFront => text.bring_all_to_front,
    })
}

/// One entry of the app menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppMenuItem {
    Separator,
    /// Its label, its AppKit action, its key with Command, and whether Option goes with it.
    Item(&'static str, &'static str, &'static str, bool),
}

/// The app menu: Hide, Hide Others, Show All and Quit, as every Mac app has them. Quitting
/// the window host closes the app's windows, which ends their tasks, as closing them does.
pub fn app_menu(text: &MenuText) -> Vec<AppMenuItem> {
    vec![
        AppMenuItem::Item(text.hide_app, "hide:", "h", false),
        AppMenuItem::Item(text.hide_others, "hideOtherApplications:", "h", true),
        AppMenuItem::Item(text.show_all, "unhideAllApplications:", "", false),
        AppMenuItem::Separator,
        AppMenuItem::Item(text.quit_app, "terminate:", "q", false),
    ]
}

/// `label` with the app's name in the place of `{app}`.
pub fn with_app(label: &str, app: &str) -> String {
    label.replace("{app}", app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr22_the_window_menu_carries_the_android_window_menu() {
        use WindowMenuItem::*;
        assert_eq!(
            window_menu(true),
            vec![
                Minimize,
                Zoom,
                Separator,
                NewWindow,
                Screenshot,
                AspectRatio,
                OpenByDefault,
                AppInfo,
                Separator,
                BringAllToFront,
            ]
        );
    }

    #[test]
    fn fr22_new_window_is_left_out_for_an_app_with_one_window() {
        assert!(!window_menu(false).contains(&WindowMenuItem::NewWindow));
        assert!(!android_items(false).contains(&WindowMenuItem::NewWindow));
        assert_eq!(
            android_items(false).first(),
            Some(&WindowMenuItem::Screenshot)
        );
    }

    #[test]
    fn fr22_each_android_item_is_carried_out_where_it_belongs() {
        for item in android_items(true) {
            assert_eq!(from_tag(tag(item)), Some(item));
        }
        assert_eq!(from_tag(0), None);
        assert_eq!(
            action(WindowMenuItem::NewWindow),
            Some(MenuAction::NewWindow)
        );
        assert_eq!(
            action(WindowMenuItem::AspectRatio),
            Some(MenuAction::AspectRatio)
        );
        assert_eq!(
            action(WindowMenuItem::OpenByDefault),
            Some(MenuAction::OpenByDefault)
        );
        assert_eq!(action(WindowMenuItem::AppInfo), Some(MenuAction::AppInfo));
        // The window host takes the screenshot itself.
        assert_eq!(action(WindowMenuItem::Screenshot), None);
        // macOS carries out its own entries.
        assert_eq!(
            system_action(WindowMenuItem::Minimize),
            Some("performMiniaturize:")
        );
        assert_eq!(tag(WindowMenuItem::Minimize), 0);
    }

    #[test]
    fn fr22_the_menus_speak_the_users_language() {
        let english = menu_text(false);
        let korean = menu_text(true);
        for item in window_menu(true) {
            if item == WindowMenuItem::Separator {
                assert_eq!(label(&english, item), None);
                continue;
            }
            let (en, ko) = (
                label(&english, item).unwrap(),
                label(&korean, item).unwrap(),
            );
            assert!(!en.is_empty() && !ko.is_empty() && en != ko, "{item:?}");
        }
        assert_eq!(label(&korean, WindowMenuItem::AppInfo), Some("앱 정보"));
        assert_eq!(with_app(english.quit_app, "Notes"), "Quit Notes");
        assert_eq!(with_app(korean.hide_app, "메모"), "메모 가리기");
    }
}
