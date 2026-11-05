//! The session bus's notification service (`org.freedesktop.Notifications`), which every
//! Linux desktop runs.

use std::collections::HashMap;
use std::path::PathBuf;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

use super::notifications::Notify;

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// What the service says the user did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    ActionInvoked { id: u32, action: String },
    Closed { id: u32, reason: u32 },
}

/// A connection to the notification service.
pub struct Notifier {
    connection: Connection,
    /// Where a notification's icon is written for the service to read.
    icons: PathBuf,
}

impl Notifier {
    pub fn connect(icons: PathBuf) -> Result<Notifier, String> {
        let connection = Connection::session()
            .map_err(|error| format!("cannot reach the session bus: {error}"))?;
        Ok(Notifier { connection, icons })
    }

    /// Shows a notification; returns the number the service gave it.
    pub fn notify(&self, notify: &Notify, icon_png: &[u8]) -> Result<u32, String> {
        let mut hints: HashMap<&str, Value> = HashMap::new();
        hints.insert("urgency", Value::from(notify.urgency));
        hints.insert("desktop-entry", Value::from(notify.desktop_entry.as_str()));
        if notify.resident {
            hints.insert("resident", Value::from(true));
        }
        let icon = if icon_png.is_empty() {
            String::new()
        } else {
            let path = self.icons.join(format!("{}.png", notify.desktop_entry));
            let _ = std::fs::create_dir_all(&self.icons);
            match std::fs::write(&path, icon_png) {
                Ok(()) => path.to_string_lossy().into_owned(),
                Err(_) => String::new(),
            }
        };
        if !icon.is_empty() {
            hints.insert("image-path", Value::from(icon.as_str()));
        }
        let actions: Vec<&str> = notify.actions.iter().map(String::as_str).collect();
        let reply = self
            .connection
            .call_method(
                Some(SERVICE),
                PATH,
                Some(SERVICE),
                "Notify",
                &(
                    notify.app_name.as_str(),
                    notify.replaces_id,
                    icon.as_str(),
                    notify.summary.as_str(),
                    notify.body.as_str(),
                    actions,
                    hints,
                    -1i32,
                ),
            )
            .map_err(|error| format!("the notification service refused it: {error}"))?;
        reply
            .body()
            .deserialize::<u32>()
            .map_err(|error| format!("the notification service answered oddly: {error}"))
    }

    /// Closes the notification numbered `id`.
    pub fn close(&self, id: u32) {
        let _ = self.connection.call_method(
            Some(SERVICE),
            PATH,
            Some(SERVICE),
            "CloseNotification",
            &(id,),
        );
    }

    /// Calls `each` with what the user does with the notifications, until the connection
    /// ends.
    pub fn watch(&self, mut each: impl FnMut(Signal)) -> Result<(), String> {
        let proxy = Proxy::new(&self.connection, SERVICE, PATH, SERVICE)
            .map_err(|error| format!("cannot watch the notification service: {error}"))?;
        let signals = proxy
            .receive_all_signals()
            .map_err(|error| format!("cannot watch the notification service: {error}"))?;
        for message in signals {
            let header = message.header();
            let Some(member) = header.member() else {
                continue;
            };
            match member.as_str() {
                "ActionInvoked" => {
                    if let Ok((id, action)) = message.body().deserialize::<(u32, String)>() {
                        each(Signal::ActionInvoked { id, action });
                    }
                }
                "NotificationClosed" => {
                    if let Ok((id, reason)) = message.body().deserialize::<(u32, u32)>() {
                        each(Signal::Closed { id, reason });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}
