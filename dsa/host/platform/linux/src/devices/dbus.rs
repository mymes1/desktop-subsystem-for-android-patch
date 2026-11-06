//! What the devices share for D-Bus: opening a proxy, and telling a refusal from a missing
//! service.

use std::collections::HashMap;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// The desktop portal's service and object.
pub const PORTAL: &str = "org.freedesktop.portal.Desktop";
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const REQUEST: &str = "org.freedesktop.portal.Request";

/// The path of the request object the portal makes for `token`, so its answer is watched for
/// before the call that makes it.
pub fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/request/{sender}/{token}")
}

/// Calls a portal method that answers through a request object, and waits for the answer:
/// the response code (0 done, 1 canceled by the user, 2 failed) and the results. `call`
/// makes the call on `interface`, with `token` as its `handle_token`.
pub fn portal_request(
    connection: &Connection,
    interface: &'static str,
    method: &str,
    token: &str,
    call: impl FnOnce(&Proxy<'_>) -> zbus::Result<OwnedObjectPath>,
) -> Result<(u32, HashMap<String, OwnedValue>), String> {
    let unique = connection
        .unique_name()
        .map(|n| n.to_string())
        .ok_or("the session bus gave no name")?;
    let request = proxy(connection, PORTAL, request_path(&unique, token), REQUEST)?;
    let mut responses = request
        .receive_signal("Response")
        .map_err(|e| e.to_string())?;
    let portal = proxy(connection, PORTAL, PORTAL_PATH.into(), interface)?;
    call(&portal).map_err(|e| format!("{method}: {e}"))?;
    let message = responses
        .next()
        .ok_or_else(|| format!("{method}: the portal went away"))?;
    message
        .body()
        .deserialize::<(u32, HashMap<String, OwnedValue>)>()
        .map_err(|e| format!("{method}: the portal's answer is unreadable: {e}"))
}

/// A proxy for `interface` of the object `path` at `destination`.
pub fn proxy<'a>(
    connection: &Connection,
    destination: &'static str,
    path: String,
    interface: &'static str,
) -> Result<Proxy<'a>, String> {
    Proxy::new(connection, destination, path, interface)
        .map_err(|e| format!("{destination} cannot be reached: {e}"))
}

/// How a call failed, as the device tells the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The user, or the desktop's policy, does not let DSA use it.
    Denied,
    /// The service is not running or the computer has no such device.
    Unavailable,
}

/// What a D-Bus error's name says about the failure.
pub fn failure_of(error_text: &str) -> Failure {
    let denied = [
        "AccessDenied",
        "NotAuthorized",
        "PermissionDenied",
        "Access denied",
        "not allowed",
    ];
    if denied.iter().any(|word| error_text.contains(word)) {
        Failure::Denied
    } else {
        Failure::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr26_a_refusal_reads_as_denied_and_anything_else_as_unavailable() {
        assert_eq!(
            failure_of("org.freedesktop.DBus.Error.AccessDenied: 'dsa' disallowed"),
            Failure::Denied
        );
        assert_eq!(
            failure_of("org.freedesktop.DBus.Error.ServiceUnknown: not provided"),
            Failure::Unavailable
        );
    }
}
