//! The window host's menu bar in AppKit: the app menu and the Window menu, whose entries
//! [`super::window_menu`] lays out, and the same window entries as the context menu of the
//! band of a window whose app draws its own title bar.
//!
//! macOS's own entries go down the responder chain to the front window and the application,
//! which carry them out. The Android entries go to the one target this module makes, which
//! hands each choice to the function the window host gave [`install`].

use std::cell::RefCell;
use std::sync::OnceLock;

use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::sel;

use super::title_bar::is_korean;
use super::window_menu::{
    android_items, app_menu, from_tag, key_equivalent, label, menu_text, system_action, tag,
    window_menu, with_app, AppMenuItem, MenuText, WindowMenuItem,
};
use crate::objc::{class, ns_string, string_of};

/// `NSEventModifierFlagOption` and `NSEventModifierFlagCommand`.
const FLAG_OPTION: usize = 1 << 19;
const FLAG_COMMAND: usize = 1 << 20;

/// What the window host does with the menus' Android entries.
#[derive(Clone, Copy)]
struct Handlers {
    /// The user chose an entry.
    chosen: fn(WindowMenuItem),
    /// Whether a task window is in front, for the entries to act on.
    available: fn() -> bool,
}

static HANDLERS: OnceLock<Handlers> = OnceLock::new();

struct MenuBar {
    window_menu: Retained<AnyObject>,
    target: Retained<AnyObject>,
    text: MenuText,
    new_window: bool,
}

thread_local! {
    static MENU: RefCell<Option<MenuBar>> = const { RefCell::new(None) };
}

/// Whether the user's first preferred language is Korean.
pub fn korean() -> bool {
    unsafe {
        let Ok(locale) = class(c"NSLocale") else {
            return false;
        };
        let languages: *mut AnyObject = msg_send![locale, preferredLanguages];
        if languages.is_null() {
            return false;
        }
        let first: *mut AnyObject = msg_send![languages, firstObject];
        is_korean(&string_of(first))
    }
}

unsafe extern "C-unwind" fn choose(_this: &AnyObject, _: Sel, sender: *mut AnyObject) {
    if sender.is_null() {
        return;
    }
    let tag: isize = msg_send![sender, tag];
    if let (Some(item), Some(handlers)) = (from_tag(tag), HANDLERS.get()) {
        // A handler that panics must not unwind into AppKit.
        let _ = std::panic::catch_unwind(|| (handlers.chosen)(item));
    }
}

unsafe extern "C-unwind" fn validate(_this: &AnyObject, _: Sel, _item: *mut AnyObject) -> Bool {
    let available = HANDLERS
        .get()
        .and_then(|handlers| std::panic::catch_unwind(handlers.available).ok())
        .unwrap_or(false);
    Bool::new(available)
}

fn target_class() -> Result<&'static AnyClass, String> {
    static CLASS: OnceLock<Result<&'static AnyClass, String>> = OnceLock::new();
    CLASS
        .get_or_init(|| unsafe {
            let superclass = class(c"NSObject")?;
            let mut builder = ClassBuilder::new(c"DSAWindowMenuTarget", superclass)
                .ok_or("the window menu's target class is already defined")?;
            builder.add_method(
                sel!(chooseAndroidItem:),
                choose as unsafe extern "C-unwind" fn(_, _, _),
            );
            builder.add_method(
                sel!(validateMenuItem:),
                validate as unsafe extern "C-unwind" fn(_, _, _) -> _,
            );
            Ok(builder.register())
        })
        .clone()
}

/// The selector of one of macOS's own entries.
fn system_selector(item: WindowMenuItem) -> Option<Sel> {
    Some(match system_action(item)? {
        "performMiniaturize:" => sel!(performMiniaturize:),
        "performZoom:" => sel!(performZoom:),
        "arrangeInFront:" => sel!(arrangeInFront:),
        _ => return None,
    })
}

fn app_selector(action: &str) -> Option<Sel> {
    Some(match action {
        "hide:" => sel!(hide:),
        "hideOtherApplications:" => sel!(hideOtherApplications:),
        "unhideAllApplications:" => sel!(unhideAllApplications:),
        "terminate:" => sel!(terminate:),
        _ => return None,
    })
}

unsafe fn new_menu(title: &str) -> Result<Retained<AnyObject>, String> {
    let allocated: Allocated<AnyObject> = msg_send![class(c"NSMenu")?, alloc];
    let menu: Option<Retained<AnyObject>> = msg_send![allocated, initWithTitle: &*ns_string(title)];
    menu.ok_or_else(|| "AppKit could not make a menu".to_string())
}

unsafe fn new_item(
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Result<Retained<AnyObject>, String> {
    let allocated: Allocated<AnyObject> = msg_send![class(c"NSMenuItem")?, alloc];
    let item: Option<Retained<AnyObject>> = msg_send![
        allocated,
        initWithTitle: &*ns_string(title),
        action: action,
        keyEquivalent: &*ns_string(key)
    ];
    item.ok_or_else(|| "AppKit could not make a menu item".to_string())
}

unsafe fn separator() -> Result<Retained<AnyObject>, String> {
    let item: Option<Retained<AnyObject>> = msg_send![class(c"NSMenuItem")?, separatorItem];
    item.ok_or_else(|| "AppKit could not make a menu separator".to_string())
}

/// The menu item for a Window menu entry.
unsafe fn window_item(
    text: &MenuText,
    item: WindowMenuItem,
    target: &AnyObject,
) -> Result<Retained<AnyObject>, String> {
    let Some(title) = label(text, item) else {
        return separator();
    };
    match system_selector(item) {
        Some(action) => new_item(title, Some(action), key_equivalent(item)),
        None => {
            let menu_item = new_item(title, Some(sel!(chooseAndroidItem:)), "")?;
            let _: () = msg_send![&*menu_item, setTarget: target];
            let _: () = msg_send![&*menu_item, setTag: tag(item)];
            Ok(menu_item)
        }
    }
}

/// The app's name, as the menu bar shows it.
unsafe fn app_name() -> String {
    let name = class(c"NSRunningApplication")
        .ok()
        .map(|running| {
            let current: *mut AnyObject = msg_send![running, currentApplication];
            if current.is_null() {
                String::new()
            } else {
                string_of(msg_send![current, localizedName])
            }
        })
        .unwrap_or_default();
    if !name.is_empty() {
        return name;
    }
    match class(c"NSProcessInfo") {
        Ok(info) => {
            let info: *mut AnyObject = msg_send![info, processInfo];
            string_of(msg_send![info, processName])
        }
        Err(_) => String::new(),
    }
}

/// Makes the app's menu bar: the app menu and the Window menu, with New Window when
/// `new_window`. `chosen` carries out an Android entry for the front window, and `available`
/// says whether there is one. On the main thread, before the application runs.
pub fn install(
    new_window: bool,
    chosen: fn(WindowMenuItem),
    available: fn() -> bool,
) -> Result<(), String> {
    let _ = HANDLERS.set(Handlers { chosen, available });
    unsafe {
        let text = menu_text(korean());
        let target: Option<Retained<AnyObject>> = msg_send![target_class()?, new];
        let target = target.ok_or("no target for the window menu could be made")?;
        let name = app_name();

        let main = new_menu("")?;
        let app_item = new_item(&name, None, "")?;
        let app = new_menu(&name)?;
        for entry in app_menu(&text) {
            let item = match entry {
                AppMenuItem::Separator => separator()?,
                AppMenuItem::Item(title, action, key, option) => {
                    let item = new_item(&with_app(title, &name), app_selector(action), key)?;
                    if option {
                        let _: () = msg_send![&*item, setKeyEquivalentModifierMask: FLAG_OPTION | FLAG_COMMAND];
                    }
                    item
                }
            };
            let _: () = msg_send![&*app, addItem: &*item];
        }
        let _: () = msg_send![&*app_item, setSubmenu: &*app];
        let _: () = msg_send![&*main, addItem: &*app_item];

        let windows_item = new_item(text.window, None, "")?;
        let windows = new_menu(text.window)?;
        for entry in window_menu(new_window) {
            let item = window_item(&text, entry, &target)?;
            let _: () = msg_send![&*windows, addItem: &*item];
        }
        let _: () = msg_send![&*windows_item, setSubmenu: &*windows];
        let _: () = msg_send![&*main, addItem: &*windows_item];

        let application: *mut AnyObject = msg_send![class(c"NSApplication")?, sharedApplication];
        let _: () = msg_send![application, setMainMenu: &*main];
        // AppKit lists the app's windows at the end of its Windows menu.
        let _: () = msg_send![application, setWindowsMenu: &*windows];
        MENU.with(|menu| {
            *menu.borrow_mut() = Some(MenuBar {
                window_menu: windows,
                target,
                text,
                new_window,
            })
        });
    }
    Ok(())
}

/// Shows New Window in the Window menu or takes it out, as the app starts or stops
/// supporting several windows. On the main thread.
pub fn set_new_window(new_window: bool) {
    MENU.with(|menu| {
        let mut menu = menu.borrow_mut();
        let Some(bar) = menu.as_mut() else {
            return;
        };
        if bar.new_window == new_window {
            return;
        }
        bar.new_window = new_window;
        unsafe {
            let menu = &*bar.window_menu;
            let existing: *mut AnyObject =
                msg_send![menu, itemWithTag: tag(WindowMenuItem::NewWindow)];
            if !new_window {
                if !existing.is_null() {
                    let _: () = msg_send![menu, removeItem: existing];
                }
                return;
            }
            if !existing.is_null() {
                return;
            }
            // New Window comes first among the Android entries, before Screenshot.
            let at: isize = msg_send![menu, indexOfItemWithTag: tag(WindowMenuItem::Screenshot)];
            match window_item(&bar.text, WindowMenuItem::NewWindow, &bar.target) {
                Ok(item) if at >= 0 => {
                    let _: () = msg_send![menu, insertItem: &*item, atIndex: at];
                }
                Ok(_) => {}
                Err(why) => eprintln!("dsa-app-window: {why}"),
            }
        }
    });
}

/// Opens the window's entries (Minimize, Zoom and the Android ones) as a context menu at
/// `event`, for a secondary click on the band of an app's own title bar. On the main thread.
pub fn pop_up(view: &AnyObject, event: *mut AnyObject) {
    // The menu is made first and the bar let go before it opens: the menu's own run loop may
    // run other main thread work, a change of the bar included.
    let made = MENU.with(|menu| -> Option<Result<Retained<AnyObject>, String>> {
        let menu = menu.borrow();
        let bar = menu.as_ref()?;
        Some(unsafe {
            (|| -> Result<Retained<AnyObject>, String> {
                let context = new_menu("")?;
                let mut entries = vec![
                    WindowMenuItem::Minimize,
                    WindowMenuItem::Zoom,
                    WindowMenuItem::Separator,
                ];
                entries.extend(android_items(bar.new_window));
                for entry in entries {
                    // A context menu has no key equivalents to show.
                    let item = window_item(&bar.text, entry, &bar.target)?;
                    let _: () = msg_send![&*item, setKeyEquivalent: &*ns_string("")];
                    let _: () = msg_send![&*context, addItem: &*item];
                }
                Ok(context)
            })()
        })
    });
    match made {
        Some(Ok(context)) => unsafe {
            let Ok(menu_class) = class(c"NSMenu") else {
                return;
            };
            let _: () = msg_send![
                menu_class,
                popUpContextMenu: &*context,
                withEvent: event,
                forView: view
            ];
        },
        Some(Err(why)) => eprintln!("dsa-app-window: the window's menu did not open: {why}"),
        None => {}
    }
}
