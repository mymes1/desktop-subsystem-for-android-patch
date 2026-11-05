//! What the desktop shows of the Android apps, and what it hands them.
//!
//! - [`desktop_entry`]: a launcher entry and an icon for each installed app, so the apps sit
//!   in the desktop's app menu and its dock as themselves.
//! - [`notifications`]: the guest's notifications as the desktop's
//!   (`org.freedesktop.Notifications`), and the user's clicks back.
//! - [`file_drop`]: files dropped on a task window (`text/uri-list`).
//! - [`clipboard`]: which clipboard formats carry text.
//! - [`dbus`]: the session bus side of the notifications (Linux only).

pub mod clipboard;
pub mod desktop_entry;
pub mod file_drop;
pub mod notifications;

#[cfg(target_os = "linux")]
pub mod dbus;
