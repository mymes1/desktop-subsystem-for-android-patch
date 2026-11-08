//! Each task window's Android UI as an AT-SPI tree, for Orca, Accerciser and every other
//! client on the accessibility bus.
//!
//! The compositor's windows are one AT-SPI application. Each window is a frame whose children
//! are its title bar's buttons (Back while the title bar shows the back arrow, then Minimize,
//! Maximize or Restore, and Close, named in the user's first preferred language) and the
//! task's accessibility nodes, with their roles, names, values, states and places. A node's
//! default action, focus, scrolling and new text go to the node's accessibility action in
//! Android; a button's action does what a click on it does.
//!
//! Nothing runs for a user without a screen reader: the provider joins the accessibility bus
//! only while the session says accessibility is on, and a window asks Android for its tree
//! only once a client reads into it, until no client has asked for a while and none listens
//! for events.
//!
//! - [`atspi`]: AT-SPI's roles and states, and a node's.
//! - [`objects`]: the objects, what a client can ask of them, and the events a change raises.
//! - [`text`]: a node's text by character, word, sentence and line.
//! - [`watch`]: what the threads share, and the watch of each window's tree.
//! - [`bus`]: the objects on the accessibility bus (Linux only).

pub mod atspi;
#[cfg(target_os = "linux")]
pub mod bus;
pub mod objects;
pub mod text;
pub mod watch;

/// Starts the provider for the compositor's windows: their trees are watched through
/// `connect`, the title bar buttons' actions go to `commands`. The windows are told to it with
/// [`watch::Shared::sync`] and [`watch::Shared::close`].
#[cfg(target_os = "linux")]
pub fn start(
    connect: watch::ApiConnect,
    commands: watch::Commands,
    log: watch::Log,
) -> std::sync::Arc<watch::Shared> {
    use dsa_host_core::accessibility::PRESENCE_GRACE;
    use objects::{prefers_korean, Access};
    use std::sync::Arc;
    use watch::Shared;

    let korean = prefers_korean(|name| std::env::var(name).ok());
    let shared = Shared::new(
        Access::new(korean, PRESENCE_GRACE),
        Some(connect),
        commands,
        log,
    );
    bus::start(Arc::clone(&shared));
    shared
}
