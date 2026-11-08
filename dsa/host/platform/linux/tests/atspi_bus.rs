//! The windows' AT-SPI objects over a real D-Bus: a client on the bus reads a Settings
//! window's tree and activates its nodes and its title bar's buttons.
//!
//! Needs a session bus, so it runs only with the `atspi-bus-tests` feature, which CI turns on
//! under `dbus-run-session`. The session bus stands in for the accessibility bus, and a fake
//! registry answers the provider's embedding as at-spi2-registryd would.

#![cfg(all(target_os = "linux", feature = "atspi-bus-tests"))]

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dsa_host_core::accessibility::{Node, NodeAction, NodeStates, TreeUpdate};
use dsa_host_core::tasks::Bounds;
use dsa_host_linux::accessibility::bus::serve_on;
use dsa_host_linux::accessibility::objects::{
    window_facts, Access, Object, TitleAction, TitleButton, TitleCommand,
};
use dsa_host_linux::accessibility::watch::Shared;
use dsa_host_linux::compositor::title_bar::TitleBar;
use dsa_host_linux::compositor::toplevel::Toplevel;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

const TASK: i32 = 7;
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";

fn node(id: u32, children: &[u32], class_name: &str, text: &str, top: i32) -> Node {
    Node {
        id,
        children: children.to_vec(),
        class_name: class_name.into(),
        text: text.into(),
        bounds: Bounds {
            left: 0,
            top,
            right: 400,
            bottom: top + 80,
        },
        states: NodeStates {
            enabled: true,
            clickable: text.is_empty(),
            ..NodeStates::default()
        },
        actions: if text.is_empty() {
            vec![NodeAction::Click]
        } else {
            Vec::new()
        },
        ..Node::default()
    }
}

fn settings_screen() -> TreeUpdate {
    TreeUpdate {
        task_id: TASK,
        epoch: 1,
        full: true,
        roots: vec![3],
        nodes: vec![
            node(3, &[4], "android.widget.LinearLayout", "", 0),
            node(4, &[], "android.widget.TextView", "Network & internet", 0),
        ],
        removed: vec![],
        focused: None,
    }
}

/// A registry that embeds every application, as at-spi2-registryd does.
fn fake_registry() {
    let registry = Connection::session().expect("a session bus");
    registry
        .request_name("org.a11y.atspi.Registry")
        .expect("the registry's name");
    std::thread::spawn(move || {
        for message in MessageIterator::from(&registry) {
            let Ok(message) = message else { continue };
            let header = message.header();
            if header.message_type() != MessageType::MethodCall {
                continue;
            }
            match header.member().map(|m| m.as_str().to_string()).as_deref() {
                Some("Embed") => {
                    let desktop = (
                        "org.a11y.atspi.Registry".to_string(),
                        OwnedObjectPath::try_from("/org/a11y/atspi/accessible/root").unwrap(),
                    );
                    let _ = registry.reply(&header, &(desktop,));
                }
                Some("GetRegisteredEvents") => {
                    let none: Vec<(String, String)> = Vec::new();
                    let _ = registry.reply(&header, &(none,));
                }
                _ => {
                    let _ = registry.reply(&header, &());
                }
            }
        }
    });
}

fn call<B>(
    client: &Connection,
    name: &str,
    object: Object,
    iface: &str,
    member: &str,
    body: &B,
) -> zbus::Message
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    client
        .call_method(
            Some(name),
            object.path().as_str(),
            Some(iface),
            member,
            body,
        )
        .unwrap_or_else(|e| panic!("{iface}.{member} on {}: {e}", object.path()))
}

fn property(client: &Connection, name: &str, object: Object, property: &str) -> OwnedValue {
    call(
        client,
        name,
        object,
        "org.freedesktop.DBus.Properties",
        "Get",
        &(ACCESSIBLE, property),
    )
    .body()
    .deserialize()
    .unwrap()
}

#[test]
fn fr18_a_client_reads_the_settings_window_and_activates_its_nodes_and_buttons() {
    fake_registry();
    let commands: Arc<Mutex<Vec<TitleCommand>>> = Arc::default();
    let shared = {
        let commands = Arc::clone(&commands);
        Shared::new(
            Access::new(false, Duration::from_secs(30)),
            None,
            Arc::new(move |command: TitleCommand| commands.lock().unwrap().push(command)),
            Arc::new(|line: &str| eprintln!("{line}")),
        )
    };
    let model = Toplevel::new(TASK, "com.android.settings", "Settings", (400, 600));
    let mut bar = TitleBar::default();
    bar.set_back(true);
    shared.sync(TASK, window_facts(&model, &bar));
    shared.lock().apply(TASK, settings_screen()).unwrap();
    let (actions, requests) = mpsc::channel();
    shared.lock().attach_actions(TASK, actions);

    let provider = Connection::session().unwrap();
    let name = provider.unique_name().unwrap().to_string();
    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || serve_on(provider, shared));
    }
    let until = Instant::now() + Duration::from_secs(10);
    while !shared.connected() {
        assert!(Instant::now() < until, "the provider embedded itself");
        std::thread::sleep(Duration::from_millis(20));
    }

    let client = Connection::session().unwrap();
    let children: Vec<(String, OwnedObjectPath)> = call(
        &client,
        &name,
        Object::Application,
        ACCESSIBLE,
        "GetChildren",
        &(),
    )
    .body()
    .deserialize()
    .unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].1.as_str(), Object::Window(TASK).path());
    let frame = Object::Window(TASK);
    assert_eq!(
        String::try_from(property(&client, &name, frame, "Name")).unwrap(),
        "Settings"
    );
    let role: u32 = call(&client, &name, frame, ACCESSIBLE, "GetRole", &())
        .body()
        .deserialize()
        .unwrap();
    assert_eq!(role, 23);
    let children: Vec<(String, OwnedObjectPath)> =
        call(&client, &name, frame, ACCESSIBLE, "GetChildren", &())
            .body()
            .deserialize()
            .unwrap();
    let paths: Vec<&str> = children.iter().map(|(_, path)| path.as_str()).collect();
    let back = Object::Button(TASK, TitleButton::Back);
    let row = Object::Node {
        task_id: TASK,
        epoch: 1,
        id: 3,
    };
    assert_eq!(
        paths,
        [
            back.path(),
            Object::Button(TASK, TitleButton::Minimize).path(),
            Object::Button(TASK, TitleButton::Maximize).path(),
            Object::Button(TASK, TitleButton::Close).path(),
            row.path(),
        ]
    );
    assert_eq!(
        i32::try_from(property(&client, &name, frame, "ChildCount")).unwrap(),
        5
    );

    // The back button is a push button named Back, and activating it goes back.
    let role: u32 = call(&client, &name, back, ACCESSIBLE, "GetRole", &())
        .body()
        .deserialize()
        .unwrap();
    assert_eq!(role, 43);
    assert_eq!(
        String::try_from(property(&client, &name, back, "Name")).unwrap(),
        "Back"
    );
    let done: bool = call(
        &client,
        &name,
        back,
        "org.a11y.atspi.Action",
        "DoAction",
        &(0i32,),
    )
    .body()
    .deserialize()
    .unwrap();
    assert!(done);
    assert_eq!(
        commands.lock().unwrap().as_slice(),
        &[TitleCommand {
            task_id: TASK,
            action: TitleAction::Back,
        }]
    );

    // The list row: its role, name and place, and its default action presses it.
    let role: u32 = call(&client, &name, row, ACCESSIBLE, "GetRole", &())
        .body()
        .deserialize()
        .unwrap();
    assert_eq!(role, 32);
    assert_eq!(
        String::try_from(property(&client, &name, row, "Name")).unwrap(),
        "Network & internet"
    );
    let extents: (i32, i32, i32, i32) = call(
        &client,
        &name,
        row,
        "org.a11y.atspi.Component",
        "GetExtents",
        &(0u32,),
    )
    .body()
    .deserialize()
    .unwrap();
    assert_eq!(extents, (0, 0, 400, 80));
    let states: Vec<u32> = call(&client, &name, row, ACCESSIBLE, "GetState", &())
        .body()
        .deserialize()
        .unwrap();
    assert_eq!(states.len(), 2);
    assert_ne!(states[0] & (1 << 8), 0, "enabled");
    let done: bool = call(
        &client,
        &name,
        row,
        "org.a11y.atspi.Action",
        "DoAction",
        &(0i32,),
    )
    .body()
    .deserialize()
    .unwrap();
    assert!(done);
    let request = requests.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!((request.node_id, request.action), (3, NodeAction::Click));

    // A node that is gone is an unknown object.
    let gone = Object::Node {
        task_id: TASK,
        epoch: 0,
        id: 3,
    };
    assert!(client
        .call_method(
            Some(name.as_str()),
            gone.path().as_str(),
            Some(ACCESSIBLE),
            "GetRole",
            &(),
        )
        .is_err());

    // A change reaches a listening client as an AT-SPI event.
    client
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "AddMatch",
            &("type='signal',interface='org.a11y.atspi.Event.Object'",),
        )
        .unwrap();
    let events = MessageIterator::from(&client);
    let mut model = model;
    model.states.maximized = true;
    shared.sync(TASK, window_facts(&model, &bar));
    let (sender, received) = mpsc::channel();
    std::thread::spawn(move || {
        for message in events {
            let Ok(message) = message else { continue };
            let header = message.header();
            if header.member().map(|m| m.as_str()) == Some("PropertyChange") {
                let path = header.path().map(|p| p.as_str().to_string());
                let body: Result<(String, i32, i32, OwnedValue, HashMap<String, OwnedValue>), _> =
                    message.body().deserialize();
                if let Ok((minor, _, _, value, _)) = body {
                    let _ = sender.send((path, minor, String::try_from(value).ok()));
                }
            }
        }
    });
    let (path, minor, value) = received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        path.as_deref(),
        Some(Object::Button(TASK, TitleButton::Maximize).path().as_str())
    );
    assert_eq!(minor, "accessible-name");
    assert_eq!(value.as_deref(), Some("Restore"));
}
