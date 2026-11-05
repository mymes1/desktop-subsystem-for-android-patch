//! What the devices share for D-Bus: opening a proxy, and telling a refusal from a missing
//! service.

use zbus::blocking::{Connection, Proxy};

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
