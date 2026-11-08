//! The accessibility bus: the compositor's objects as AT-SPI's D-Bus interfaces.
//!
//! The session bus's `org.a11y.Bus` says whether accessibility is on (`IsEnabled`) and whether
//! a screen reader runs (`ScreenReaderEnabled`). While either is on, the provider connects to
//! the accessibility bus that `org.a11y.Bus` names, embeds its application in the registry's
//! desktop and answers every call on its objects. It follows the status through
//! `PropertiesChanged` signals, so nothing polls while accessibility is off, and it follows
//! which clients registered for events through the registry's signals, which keeps a watched
//! tree for as long as a client listens.
//!
//! The objects are answered by hand from each incoming message rather than registered one by
//! one: a window's nodes come and go many times a second, and their paths name their tree's
//! epoch, so registering each would cost more than answering it. A call on an object that is
//! gone gets D-Bus's unknown object error; nothing a client sends can end the compositor.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use dsa_host_core::accessibility::NodeAction;
use serde::Serialize;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::message::{Header, Type as MessageType};
use zbus::zvariant::{DynamicType, ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::Message;

use super::atspi::{coord, layer, role, role_name, state};
use super::objects::{interface, Access, Event, Object, WindowEvent, APPLICATION_PATH, NULL_PATH};
use super::text::{self, Unit};
use super::watch::{EventSink, Shared};

const A11Y_BUS: &str = "org.a11y.Bus";
const A11Y_BUS_PATH: &str = "/org/a11y/bus";
const STATUS: &str = "org.a11y.Status";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const REGISTRY_PATH: &str = "/org/a11y/atspi/registry";
const SOCKET: &str = "org.a11y.atspi.Socket";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const INTROSPECTABLE: &str = "org.freedesktop.DBus.Introspectable";
const PEER: &str = "org.freedesktop.DBus.Peer";
const EVENT_OBJECT: &str = "org.a11y.atspi.Event.Object";
const EVENT_WINDOW: &str = "org.a11y.atspi.Event.Window";

/// An AT-SPI object reference: a bus name and an object path.
type Reference = (String, OwnedObjectPath);

/// How long to wait before reaching the session bus or the accessibility bus again.
const RETRY: Duration = Duration::from_secs(5);

/// Whether accessibility is on, which the bus thread waits for.
#[derive(Default)]
struct Enabled {
    on: Mutex<bool>,
    changed: Condvar,
}

impl Enabled {
    fn set(&self, on: bool) {
        *self.on.lock().unwrap_or_else(|e| e.into_inner()) = on;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut on = self.on.lock().unwrap_or_else(|e| e.into_inner());
        while !*on {
            on = self.changed.wait(on).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn get(&self) -> bool {
        *self.on.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Starts the provider: one thread follows the session's accessibility status, one serves the
/// accessibility bus while it is on.
pub fn start(shared: Arc<Shared>) {
    let enabled = Arc::new(Enabled::default());
    {
        let shared = Arc::clone(&shared);
        let enabled = Arc::clone(&enabled);
        let spawned = thread::Builder::new()
            .name("dsa-compositor-a11y-status".into())
            .spawn(move || follow_status(&shared, &enabled));
        if let Err(why) = spawned {
            shared.log(&format!("accessibility: cannot follow its status: {why}"));
            return;
        }
    }
    let spawned = {
        let shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("dsa-compositor-a11y-bus".into())
            .spawn(move || serve_while_enabled(&shared, &enabled))
    };
    if let Err(why) = spawned {
        shared.log(&format!("accessibility: cannot serve its bus: {why}"));
    }
}

/// Reads one of `org.a11y.Status`'s flags; off when the session has no accessibility bus.
fn read_flag(session: &Connection, name: &str) -> bool {
    session
        .call_method(
            Some(A11Y_BUS),
            A11Y_BUS_PATH,
            Some(PROPERTIES),
            "Get",
            &(STATUS, name),
        )
        .ok()
        .and_then(|reply| reply.body().deserialize::<OwnedValue>().ok())
        .and_then(|value| bool::try_from(value).ok())
        .unwrap_or(false)
}

fn follow_status(shared: &Shared, enabled: &Enabled) {
    let mut warned = false;
    loop {
        let session = match Connection::session() {
            Ok(session) => session,
            Err(why) => {
                if !warned {
                    shared.log(&format!(
                        "accessibility: cannot reach the session bus ({why}); screen readers \
                         cannot read the windows"
                    ));
                    warned = true;
                }
                thread::sleep(RETRY * 6);
                continue;
            }
        };
        // The proxy lives as long as its signals are read.
        let proxy = Proxy::new(&session, A11Y_BUS, A11Y_BUS_PATH, PROPERTIES);
        let changes = proxy.as_ref().map_err(|e| e.to_string()).and_then(|proxy| {
            proxy
                .receive_signal("PropertiesChanged")
                .map_err(|e| e.to_string())
        });
        let changes = match changes {
            Ok(changes) => changes,
            Err(why) => {
                if !warned {
                    shared.log(&format!(
                        "accessibility: cannot follow the session's accessibility status: {why}"
                    ));
                    warned = true;
                }
                thread::sleep(RETRY * 6);
                continue;
            }
        };
        let mut apply = || {
            let on = read_flag(&session, "IsEnabled");
            let screen_reader = read_flag(&session, "ScreenReaderEnabled");
            shared.set_screen_reader(screen_reader);
            let now = on || screen_reader;
            if now != enabled.get() {
                shared.log(if now {
                    "accessibility is on: the windows join the accessibility bus"
                } else {
                    "accessibility is off: no window's tree is read from Android"
                });
            }
            if !now {
                shared.end_watches();
            }
            enabled.set(now);
        };
        apply();
        for _ in changes {
            apply();
        }
        thread::sleep(RETRY);
    }
}

fn serve_while_enabled(shared: &Arc<Shared>, enabled: &Enabled) {
    let mut warned = false;
    loop {
        enabled.wait();
        match connect_accessibility_bus() {
            Ok(bus) => {
                warned = false;
                if let Err(why) = serve_on(bus, Arc::clone(shared)) {
                    shared.log(&format!("accessibility: {why}"));
                }
                shared.log("accessibility: the accessibility bus went away");
            }
            Err(why) => {
                if !warned {
                    shared.log(&format!("accessibility: {why}"));
                    warned = true;
                }
            }
        }
        shared.set_sink(None);
        shared.set_listeners(false);
        shared.end_watches();
        thread::sleep(RETRY);
    }
}

/// The accessibility bus the session's `org.a11y.Bus` names.
fn connect_accessibility_bus() -> Result<Connection, String> {
    let session =
        Connection::session().map_err(|e| format!("cannot reach the session bus: {e}"))?;
    let address: String = session
        .call_method(
            Some(A11Y_BUS),
            A11Y_BUS_PATH,
            Some(A11Y_BUS),
            "GetAddress",
            &(),
        )
        .map_err(|e| format!("the session has no accessibility bus: {e}"))?
        .body()
        .deserialize()
        .map_err(|e| format!("the accessibility bus's address could not be read: {e}"))?;
    zbus::blocking::connection::Builder::address(address.as_str())
        .and_then(|builder| builder.build())
        .map_err(|e| format!("cannot reach the accessibility bus at {address}: {e}"))
}

/// Serves the provider's objects on `connection` until it ends: embeds the application in
/// the registry's desktop, then answers every call. The registry may call back before it
/// answers the embedding, so the calls are answered from the start.
pub fn serve_on(connection: Connection, shared: Arc<Shared>) -> Result<(), String> {
    let name = connection
        .unique_name()
        .map(|name| name.to_string())
        .ok_or("the accessibility bus gave no name")?;
    let messages = MessageIterator::from(&connection);
    let bus = Arc::new(Bus {
        connection,
        name,
        desktop: Mutex::new(None),
        application_id: Mutex::new(0),
        listeners: Mutex::new(HashMap::new()),
    });
    {
        let bus = Arc::clone(&bus);
        let shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("dsa-compositor-a11y-embed".into())
            .spawn(move || match bus.register() {
                Ok(listening) => {
                    shared.set_listeners(listening);
                    shared.set_sink(Some(Arc::new(BusSink(Arc::clone(&bus)))));
                    shared.log("accessibility: the windows are on the accessibility bus");
                }
                Err(why) => shared.log(&format!("accessibility: {why}")),
            })
            .map_err(|e| format!("cannot join the accessibility bus: {e}"))?;
    }
    for message in messages {
        let Ok(message) = message else {
            continue;
        };
        bus.handle(&shared, &message);
    }
    Ok(())
}

/// Why a call was not answered with its result.
enum Fault {
    UnknownObject,
    UnknownMethod,
    InvalidArgs(String),
    Failed(String),
}

type Answer = Result<(), Fault>;

/// The provider on the accessibility bus.
struct Bus {
    connection: Connection,
    /// The provider's unique name, which every object reference carries.
    name: String,
    /// The registry's desktop, the application's parent.
    desktop: Mutex<Option<Reference>>,
    application_id: Mutex<i32>,
    /// The events each client registered for, by its bus name.
    listeners: Mutex<HashMap<String, Vec<String>>>,
}

fn owned_path(path: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(path.to_string())
        .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked(NULL_PATH).into())
}

/// One argument of a call.
fn args<T>(message: &Message) -> Result<T, Fault>
where
    T: serde::de::DeserializeOwned + zbus::zvariant::Type,
{
    message
        .body()
        .deserialize::<T>()
        .map_err(|e| Fault::InvalidArgs(e.to_string()))
}

impl Bus {
    fn reference(&self, object: Object) -> Reference {
        (self.name.clone(), owned_path(&object.path()))
    }

    fn null(&self) -> Reference {
        (self.name.clone(), owned_path(NULL_PATH))
    }

    fn maybe(&self, object: Option<Object>) -> Reference {
        object.map_or_else(|| self.null(), |object| self.reference(object))
    }

    /// Embeds the application in the registry's desktop. Whether a client already listens
    /// for events.
    fn register(&self) -> Result<bool, String> {
        let reply = self
            .connection
            .call_method(
                Some(REGISTRY),
                APPLICATION_PATH,
                Some(SOCKET),
                "Embed",
                &((
                    self.name.as_str(),
                    ObjectPath::from_static_str_unchecked(APPLICATION_PATH),
                ),),
            )
            .map_err(|e| format!("the registry did not take the windows: {e}"))?;
        let desktop: Reference = reply
            .body()
            .deserialize()
            .map_err(|e| format!("the registry answered oddly: {e}"))?;
        *self.desktop.lock().unwrap_or_else(|e| e.into_inner()) = Some(desktop);
        // Which clients listen, and from now on when that changes.
        let _ = self.connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "AddMatch",
            &(format!("type='signal',interface='{REGISTRY}'"),),
        );
        let registered: Vec<(String, String)> = self
            .connection
            .call_method(
                Some(REGISTRY),
                REGISTRY_PATH,
                Some(REGISTRY),
                "GetRegisteredEvents",
                &(),
            )
            .ok()
            .and_then(|reply| reply.body().deserialize().ok())
            .unwrap_or_default();
        let mut listeners = self.listeners.lock().unwrap_or_else(|e| e.into_inner());
        for (bus, event) in registered {
            if bus != self.name {
                listeners.entry(bus).or_default().push(event);
            }
        }
        Ok(!listeners.is_empty())
    }

    fn handle(&self, shared: &Arc<Shared>, message: &Message) {
        let header = message.header();
        match header.message_type() {
            MessageType::MethodCall => {}
            MessageType::Signal => {
                self.registry_signal(shared, &header, message);
                return;
            }
            _ => return,
        }
        let path = header.path().map(|path| path.as_str().to_string());
        let member = header
            .member()
            .map(|member| member.as_str().to_string())
            .unwrap_or_default();
        let iface = header
            .interface()
            .map(|iface| iface.as_str().to_string())
            .unwrap_or_default();
        let answer = match path.as_deref().and_then(Object::from_path) {
            Some(object) => self.call(shared, &header, message, object, &iface, &member),
            None => Err(Fault::UnknownObject),
        };
        if let Err(fault) = answer {
            let (name, why) = match fault {
                Fault::UnknownObject => (
                    "org.freedesktop.DBus.Error.UnknownObject",
                    "no such object; it may have gone".to_string(),
                ),
                Fault::UnknownMethod => (
                    "org.freedesktop.DBus.Error.UnknownMethod",
                    format!("{iface}.{member} is not offered here"),
                ),
                Fault::InvalidArgs(why) => ("org.freedesktop.DBus.Error.InvalidArgs", why),
                Fault::Failed(why) => ("org.freedesktop.DBus.Error.Failed", why),
            };
            let _ = self.connection.reply_error(&header, name, &(why,));
        }
    }

    fn registry_signal(&self, shared: &Shared, header: &Header<'_>, message: &Message) {
        if header.interface().map(|i| i.as_str()) != Some(REGISTRY) {
            return;
        }
        let member = header.member().map(|m| m.as_str().to_string());
        let Ok((bus, event)) = message.body().deserialize::<(String, String)>() else {
            return;
        };
        if bus == self.name {
            return;
        }
        let mut listeners = self.listeners.lock().unwrap_or_else(|e| e.into_inner());
        match member.as_deref() {
            Some("EventListenerRegistered") => listeners.entry(bus).or_default().push(event),
            Some("EventListenerDeregistered") => {
                if let Some(events) = listeners.get_mut(&bus) {
                    if let Some(at) = events.iter().position(|known| *known == event) {
                        events.remove(at);
                    }
                    if events.is_empty() {
                        listeners.remove(&bus);
                    }
                }
            }
            _ => return,
        }
        shared.set_listeners(!listeners.is_empty());
    }

    fn ok<B>(&self, header: &Header<'_>, body: &B) -> Answer
    where
        B: Serialize + DynamicType,
    {
        let _ = self.connection.reply(header, body);
        Ok(())
    }

    fn call(
        &self,
        shared: &Arc<Shared>,
        header: &Header<'_>,
        message: &Message,
        object: Object,
        iface: &str,
        member: &str,
    ) -> Answer {
        // A read into a window asks the guest for its tree.
        let property = if iface == PROPERTIES && member == "Get" {
            args::<(String, String)>(message).ok().map(|(_, name)| name)
        } else {
            None
        };
        if let Some(task_id) = Access::wants_tree(object, member, property.as_deref()) {
            shared.asked(task_id);
        }
        match iface {
            PROPERTIES => return self.properties(shared, header, message, object, member),
            INTROSPECTABLE if member == "Introspect" => {
                let interfaces = shared.lock().interfaces(object);
                return self.ok(header, &(introspection(&interfaces),));
            }
            PEER if member == "Ping" => return self.ok(header, &()),
            _ => {}
        }
        if !shared.lock().exists(object) {
            return Err(Fault::UnknownObject);
        }
        let offered = shared.lock().interfaces(object);
        // A call that names no interface goes to the first that has the method.
        let order: Vec<&str> = if iface.is_empty() {
            offered.clone()
        } else if offered.contains(&iface) {
            vec![iface]
        } else {
            return Err(Fault::UnknownMethod);
        };
        for iface in order {
            let answer = match iface {
                interface::ACCESSIBLE => self.accessible(shared, header, message, object, member),
                interface::APPLICATION => self.application(shared, header, member),
                interface::COMPONENT => self.component(shared, header, message, object, member),
                interface::ACTION => self.action(shared, header, message, object, member),
                interface::TEXT => self.text(shared, header, message, object, member),
                interface::EDITABLE_TEXT => {
                    self.editable_text(shared, header, message, object, member)
                }
                _ => Err(Fault::UnknownMethod),
            };
            match answer {
                Err(Fault::UnknownMethod) => continue,
                answer => return answer,
            }
        }
        Err(Fault::UnknownMethod)
    }

    fn accessible(
        &self,
        shared: &Shared,
        header: &Header<'_>,
        message: &Message,
        object: Object,
        member: &str,
    ) -> Answer {
        match member {
            "GetChildAtIndex" => {
                let index: i32 = args(message)?;
                let child = usize::try_from(index)
                    .ok()
                    .and_then(|at| shared.lock().children(object).get(at).copied());
                self.ok(header, &(self.maybe(child),))
            }
            "GetChildren" => {
                let children: Vec<Reference> = shared
                    .lock()
                    .children(object)
                    .into_iter()
                    .map(|child| self.reference(child))
                    .collect();
                self.ok(header, &(children,))
            }
            "GetIndexInParent" => {
                let index = shared.lock().index_in_parent(object);
                self.ok(header, &(index,))
            }
            "GetRelationSet" => {
                let relations: Vec<(u32, Vec<Reference>)> = Vec::new();
                self.ok(header, &(relations,))
            }
            "GetRole" => {
                let role = shared.lock().role(object);
                self.ok(header, &(role,))
            }
            "GetRoleName" | "GetLocalizedRoleName" => {
                let role = shared.lock().role(object);
                self.ok(header, &(role_name(role),))
            }
            "GetState" => {
                let states = shared.lock().states(object).words().to_vec();
                self.ok(header, &(states,))
            }
            "GetAttributes" => {
                let attributes: HashMap<String, String> =
                    shared.lock().attributes(object).into_iter().collect();
                self.ok(header, &(attributes,))
            }
            "GetApplication" => self.ok(header, &(self.reference(Object::Application),)),
            "GetInterfaces" => {
                let interfaces = shared.lock().interfaces(object);
                self.ok(header, &(interfaces,))
            }
            _ => Err(Fault::UnknownMethod),
        }
    }

    fn application(&self, shared: &Shared, header: &Header<'_>, member: &str) -> Answer {
        match member {
            "GetLocale" => self.ok(header, &(locale(shared),)),
            _ => Err(Fault::UnknownMethod),
        }
    }

    fn component(
        &self,
        shared: &Shared,
        header: &Header<'_>,
        message: &Message,
        object: Object,
        member: &str,
    ) -> Answer {
        match member {
            "Contains" => {
                let (x, y, coord_type): (i32, i32, u32) = args(message)?;
                let inside = shared.lock().contains(object, x, y, coord_type);
                self.ok(header, &(inside,))
            }
            "GetAccessibleAtPoint" => {
                let (x, y, coord_type): (i32, i32, u32) = args(message)?;
                let found = shared.lock().at_point(object, x, y, coord_type);
                self.ok(header, &(self.maybe(found),))
            }
            "GetExtents" => {
                let coord_type: u32 = args(message)?;
                let rect = shared
                    .lock()
                    .extents(object, coord_type)
                    .unwrap_or((0, 0, 0, 0));
                self.ok(header, &(rect,))
            }
            "GetPosition" => {
                let coord_type: u32 = args(message)?;
                let (x, y, _, _) = shared
                    .lock()
                    .extents(object, coord_type)
                    .unwrap_or((0, 0, 0, 0));
                self.ok(header, &(x, y))
            }
            "GetSize" => {
                let (_, _, width, height) = shared
                    .lock()
                    .extents(object, coord::WINDOW)
                    .unwrap_or((0, 0, 0, 0));
                self.ok(header, &(width, height))
            }
            "GetLayer" => {
                let layer = match object {
                    Object::Window(_) => layer::WINDOW,
                    _ => layer::WIDGET,
                };
                self.ok(header, &(layer,))
            }
            "GetMDIZOrder" => self.ok(header, &(0i16,)),
            "GetAlpha" => self.ok(header, &(1.0f64,)),
            "GrabFocus" => {
                let action = shared.lock().focus_action(object);
                let done = action
                    .is_some_and(|action| shared.request(object, action, String::new()).is_ok());
                self.ok(header, &(done,))
            }
            "SetExtents" | "SetPosition" | "SetSize" | "ScrollTo" | "ScrollToPoint" => {
                self.ok(header, &(false,))
            }
            _ => Err(Fault::UnknownMethod),
        }
    }

    fn action(
        &self,
        shared: &Shared,
        header: &Header<'_>,
        message: &Message,
        object: Object,
        member: &str,
    ) -> Answer {
        let actions = shared.lock().actions(object);
        let nth = |message: &Message| -> Result<Option<super::objects::Action>, Fault> {
            let index: i32 = args(message)?;
            Ok(usize::try_from(index)
                .ok()
                .and_then(|at| actions.get(at).copied()))
        };
        match member {
            "GetName" => {
                let name = nth(message)?.map_or("", |action| action.name);
                self.ok(header, &(name,))
            }
            "GetLocalizedName" | "GetDescription" => {
                let name = nth(message)?.map_or("", |action| action.localized);
                self.ok(header, &(name,))
            }
            "GetKeyBinding" => self.ok(header, &("",)),
            "GetActions" => {
                let all: Vec<(&str, &str, &str)> = actions
                    .iter()
                    .map(|action| (action.localized, action.localized, ""))
                    .collect();
                self.ok(header, &(all,))
            }
            "DoAction" => {
                let index: i32 = args(message)?;
                match shared.perform(object, index) {
                    Ok(()) => self.ok(header, &(true,)),
                    Err(why) => {
                        shared.log(&format!("accessibility: an action was not done: {why}"));
                        self.ok(header, &(false,))
                    }
                }
            }
            _ => Err(Fault::UnknownMethod),
        }
    }

    fn text(
        &self,
        shared: &Shared,
        header: &Header<'_>,
        message: &Message,
        object: Object,
        member: &str,
    ) -> Answer {
        let content = shared.lock().text(object).unwrap_or_default();
        let rect = shared
            .lock()
            .extents(object, coord::WINDOW)
            .unwrap_or((0, 0, 0, 0));
        match member {
            "GetText" => {
                let (start, end): (i32, i32) = args(message)?;
                self.ok(header, &(text::slice(&content, start, end),))
            }
            "GetCharacterAtOffset" => {
                let offset: i32 = args(message)?;
                self.ok(header, &(text::character_at(&content, offset),))
            }
            "GetStringAtOffset" => {
                let (offset, granularity): (i32, u32) = args(message)?;
                let unit = Unit::from_granularity(granularity);
                self.ok(header, &text::unit_at(&content, offset, unit))
            }
            "GetTextAtOffset" | "GetTextBeforeOffset" | "GetTextAfterOffset" => {
                let (offset, boundary): (i32, u32) = args(message)?;
                let unit = Unit::from_boundary(boundary);
                let piece = match member {
                    "GetTextBeforeOffset" => text::unit_before(&content, offset, unit),
                    "GetTextAfterOffset" => text::unit_after(&content, offset, unit),
                    _ => text::unit_at(&content, offset, unit),
                };
                self.ok(header, &piece)
            }
            "GetNSelections" => self.ok(header, &(0i32,)),
            "GetSelection" => self.ok(header, &(0i32, 0i32)),
            "GetCharacterExtents" | "GetRangeExtents" => self.ok(header, &rect),
            "GetOffsetAtPoint" => self.ok(header, &(-1i32,)),
            "GetAttributeRun" | "GetAttributes" => {
                let attributes: HashMap<String, String> = HashMap::new();
                self.ok(header, &(attributes, 0i32, text::length(&content)))
            }
            "GetDefaultAttributes" | "GetDefaultAttributeSet" => {
                let attributes: HashMap<String, String> = HashMap::new();
                self.ok(header, &(attributes,))
            }
            "SetCaretOffset" | "AddSelection" | "RemoveSelection" | "SetSelection" => {
                self.ok(header, &(false,))
            }
            _ => Err(Fault::UnknownMethod),
        }
    }

    fn editable_text(
        &self,
        shared: &Shared,
        header: &Header<'_>,
        message: &Message,
        object: Object,
        member: &str,
    ) -> Answer {
        let set = |new: String| shared.request(object, NodeAction::SetText, new).is_ok();
        // A password's text is not known, only its length, so it can only be replaced whole.
        let current = || {
            let access = shared.lock();
            let password = access.role(object) == role::PASSWORD_TEXT;
            (!password).then(|| access.text(object).unwrap_or_default())
        };
        match member {
            "SetTextContents" => {
                let new: String = args(message)?;
                self.ok(header, &(set(new),))
            }
            "InsertText" => {
                let (position, inserted, length): (i32, String, i32) = args(message)?;
                let inserted = text::slice(&inserted, 0, length);
                let done =
                    current().is_some_and(|now| set(text::insert(&now, position, &inserted)));
                self.ok(header, &(done,))
            }
            "DeleteText" => {
                let (start, end): (i32, i32) = args(message)?;
                let done = current().is_some_and(|now| set(text::delete(&now, start, end)));
                self.ok(header, &(done,))
            }
            "CopyText" => self.ok(header, &()),
            "CutText" | "PasteText" => self.ok(header, &(false,)),
            _ => Err(Fault::UnknownMethod),
        }
    }

    fn properties(
        &self,
        shared: &Shared,
        header: &Header<'_>,
        message: &Message,
        object: Object,
        member: &str,
    ) -> Answer {
        match member {
            "Get" => {
                let (iface, name): (String, String) = args(message)?;
                match self.property(shared, object, &iface, &name) {
                    Some(value) => self.ok(header, &(value,)),
                    None => Err(Fault::InvalidArgs(format!("no property {iface}.{name}"))),
                }
            }
            "GetAll" => {
                let iface: String = args(message)?;
                let names: &[&str] = match iface.as_str() {
                    interface::ACCESSIBLE => &[
                        "Name",
                        "Description",
                        "Parent",
                        "ChildCount",
                        "Locale",
                        "AccessibleId",
                        "HelpText",
                    ],
                    interface::APPLICATION => &["ToolkitName", "Version", "AtspiVersion", "Id"],
                    interface::ACTION => &["NActions"],
                    interface::VALUE => &[
                        "MinimumValue",
                        "MaximumValue",
                        "MinimumIncrement",
                        "CurrentValue",
                        "Text",
                    ],
                    interface::TEXT => &["CharacterCount", "CaretOffset"],
                    _ => &[],
                };
                let all: HashMap<&str, Value<'static>> = names
                    .iter()
                    .filter_map(|name| Some((*name, self.property(shared, object, &iface, name)?)))
                    .collect();
                self.ok(header, &(all,))
            }
            "Set" => {
                let (iface, name, value): (String, String, OwnedValue) = args(message)?;
                match (iface.as_str(), name.as_str()) {
                    (interface::APPLICATION, "Id") => {
                        if let Ok(id) = i32::try_from(value) {
                            *self
                                .application_id
                                .lock()
                                .unwrap_or_else(|e| e.into_inner()) = id;
                        }
                        self.ok(header, &())
                    }
                    (interface::VALUE, "CurrentValue") => {
                        let value =
                            f64::try_from(value).map_err(|e| Fault::InvalidArgs(e.to_string()))?;
                        let action = shared.lock().value_action(object, value);
                        if let Some(action) = action {
                            shared
                                .request(object, action, String::new())
                                .map_err(Fault::Failed)?;
                        }
                        self.ok(header, &())
                    }
                    _ => Err(Fault::InvalidArgs(format!("{iface}.{name} cannot be set"))),
                }
            }
            _ => Err(Fault::UnknownMethod),
        }
    }

    /// The value of one property of the object.
    fn property(
        &self,
        shared: &Shared,
        object: Object,
        iface: &str,
        name: &str,
    ) -> Option<Value<'static>> {
        let access = shared.lock();
        let value = match (iface, name) {
            (interface::ACCESSIBLE, "Name") => Value::from(access.name(object)),
            (interface::ACCESSIBLE, "Description") => Value::from(access.description(object)),
            (interface::ACCESSIBLE, "HelpText") => Value::from(access.help_text(object)),
            (interface::ACCESSIBLE, "AccessibleId") => Value::from(access.accessible_id(object)),
            (interface::ACCESSIBLE, "Locale") => Value::from(locale_of(&access)),
            (interface::ACCESSIBLE, "ChildCount") => {
                Value::from(access.children(object).len() as i32)
            }
            (interface::ACCESSIBLE, "Parent") => {
                let parent = match access.parent(object) {
                    Some(parent) => self.reference(parent),
                    None if object == Object::Application => self
                        .desktop
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone()
                        .unwrap_or_else(|| self.null()),
                    None => self.null(),
                };
                Value::from(parent)
            }
            (interface::APPLICATION, "ToolkitName") => Value::from("DSA".to_string()),
            (interface::APPLICATION, "Version") => Value::from(crate::VERSION.to_string()),
            (interface::APPLICATION, "AtspiVersion") => Value::from("2.1".to_string()),
            (interface::APPLICATION, "Id") => Value::from(
                *self
                    .application_id
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()),
            ),
            (interface::ACTION, "NActions") => Value::from(access.actions(object).len() as i32),
            (interface::VALUE, "MinimumValue") => Value::from(f64::from(access.range(object)?.min)),
            (interface::VALUE, "MaximumValue") => Value::from(f64::from(access.range(object)?.max)),
            (interface::VALUE, "CurrentValue") => {
                Value::from(f64::from(access.range(object)?.current))
            }
            (interface::VALUE, "MinimumIncrement") => Value::from(0.0f64),
            (interface::VALUE, "Text") => Value::from(access.description(object)),
            (interface::TEXT, "CharacterCount") => {
                Value::from(text::length(&access.text(object).unwrap_or_default()))
            }
            (interface::TEXT, "CaretOffset") => {
                // Android does not say where the caret is; a focused field has it at its end.
                let focused = access.states(object).contains(state::FOCUSED);
                let length = text::length(&access.text(object).unwrap_or_default());
                Value::from(if focused { length } else { 0 })
            }
            _ => return None,
        };
        Some(value)
    }

    /// Sends one event as AT-SPI's signal.
    fn signal(&self, event: &Event) {
        let empty = || -> HashMap<&str, Value<'static>> { HashMap::new() };
        let (object, iface, member, minor, one, two, data): (
            Object,
            &str,
            &str,
            String,
            i32,
            i32,
            Value<'static>,
        ) = match event {
            Event::ChildAdded {
                parent,
                index,
                child,
            } => (
                *parent,
                EVENT_OBJECT,
                "ChildrenChanged",
                "add".into(),
                *index,
                0,
                Value::from(self.reference(*child)),
            ),
            Event::ChildRemoved {
                parent,
                index,
                child,
            } => (
                *parent,
                EVENT_OBJECT,
                "ChildrenChanged",
                "remove".into(),
                *index,
                0,
                Value::from(self.reference(*child)),
            ),
            Event::State { object, state, on } => (
                *object,
                EVENT_OBJECT,
                "StateChanged",
                (*state).into(),
                i32::from(*on),
                0,
                Value::from(0i32),
            ),
            Event::Name { object, name } => (
                *object,
                EVENT_OBJECT,
                "PropertyChange",
                "accessible-name".into(),
                0,
                0,
                Value::from(name.clone()),
            ),
            Event::Description {
                object,
                description,
            } => (
                *object,
                EVENT_OBJECT,
                "PropertyChange",
                "accessible-description".into(),
                0,
                0,
                Value::from(description.clone()),
            ),
            Event::Value { object, value } => (
                *object,
                EVENT_OBJECT,
                "PropertyChange",
                "accessible-value".into(),
                0,
                0,
                Value::from(*value),
            ),
            Event::Bounds { object, rect } => (
                *object,
                EVENT_OBJECT,
                "BoundsChanged",
                String::new(),
                0,
                0,
                Value::from(*rect),
            ),
            Event::TextRemoved {
                object,
                start,
                text,
            } => (
                *object,
                EVENT_OBJECT,
                "TextChanged",
                "delete".into(),
                *start,
                text::length(text),
                Value::from(text.clone()),
            ),
            Event::TextInserted {
                object,
                start,
                text,
            } => (
                *object,
                EVENT_OBJECT,
                "TextChanged",
                "insert".into(),
                *start,
                text::length(text),
                Value::from(text.clone()),
            ),
            Event::Window {
                task_id,
                kind,
                title,
            } => (
                Object::Window(*task_id),
                EVENT_WINDOW,
                match kind {
                    WindowEvent::Create => "Create",
                    WindowEvent::Destroy => "Destroy",
                    WindowEvent::Activate => "Activate",
                    WindowEvent::Deactivate => "Deactivate",
                },
                String::new(),
                0,
                0,
                Value::from(title.clone()),
            ),
        };
        let path = object.path();
        let _ = self.connection.emit_signal(
            None::<&str>,
            path.as_str(),
            iface,
            member,
            &(minor, one, two, data, empty()),
        );
    }
}

/// Sends the provider's events on the accessibility bus.
struct BusSink(Arc<Bus>);

impl EventSink for BusSink {
    fn emit(&self, events: &[Event]) {
        for event in events {
            self.0.signal(event);
        }
    }
}

fn locale_of(access: &Access) -> String {
    if access.korean() { "ko_KR" } else { "en_US" }.to_string()
}

fn locale(shared: &Shared) -> String {
    locale_of(&shared.lock())
}

/// What `Introspect` says of an object: the interfaces it implements.
fn introspection(interfaces: &[&str]) -> String {
    let mut xml = String::from(
        "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \
         \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">\n<node>\n",
    );
    for iface in interfaces
        .iter()
        .chain([PROPERTIES, INTROSPECTABLE, PEER].iter())
    {
        xml.push_str(&format!("  <interface name=\"{iface}\"/>\n"));
    }
    xml.push_str("</node>\n");
    xml
}
