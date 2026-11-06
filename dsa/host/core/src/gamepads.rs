//! The host's game controllers, which the guest shows its apps as gamepad input devices.
//!
//! The platform reads the controllers the user's session has (on Windows, through XInput) and
//! hands each change to the user's window manager, which passes it over the local API to the
//! service, and the service to the user's Android. The guest's control daemon makes each
//! controller a gamepad device of its own, so a game sees it as it would a USB or Bluetooth
//! gamepad: its buttons, sticks and triggers, its connection and disconnection, and its rumble,
//! which comes back the same way.
//!
//! Nothing here names a host operating system API.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dsa_protocol::control;
use dsa_transport::Connector;

use crate::host_api::HostApiClient;

/// The most controllers the guest is shown at once. XInput has four; this leaves room for
/// another kind of controller beside them.
pub const MAX_PADS: usize = 8;

/// The longest controller name passed on, in bytes.
pub const MAX_NAME: usize = 128;

/// The bits of [`GamepadState::buttons`], the same as the control protocol's `GamepadButton`.
pub mod buttons {
    pub const A: u32 = 1;
    pub const B: u32 = 2;
    pub const X: u32 = 4;
    pub const Y: u32 = 8;
    pub const LEFT_SHOULDER: u32 = 16;
    pub const RIGHT_SHOULDER: u32 = 32;
    pub const BACK: u32 = 64;
    pub const START: u32 = 128;
    pub const GUIDE: u32 = 256;
    pub const LEFT_THUMB: u32 = 512;
    pub const RIGHT_THUMB: u32 = 1024;
    pub const DPAD_UP: u32 = 2048;
    pub const DPAD_DOWN: u32 = 4096;
    pub const DPAD_LEFT: u32 = 8192;
    pub const DPAD_RIGHT: u32 = 16384;
    /// Every bit a controller may set.
    pub const ALL: u32 = 32767;
}

/// A controller that was connected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GamepadInfo {
    pub pad_id: u32,
    pub name: String,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Whether it can rumble.
    pub rumble: bool,
}

/// Everything a controller's buttons, sticks and triggers say at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GamepadState {
    pub pad_id: u32,
    /// The buttons held, one bit each ([`buttons`]).
    pub buttons: u32,
    /// The sticks, from -32768 (left, or up) to 32767 (right, or down).
    pub left_x: i16,
    pub left_y: i16,
    pub right_x: i16,
    pub right_y: i16,
    /// The triggers, from 0 to 255.
    pub left_trigger: u8,
    pub right_trigger: u8,
}

/// A change to the host's controllers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GamepadEvent {
    Connected(GamepadInfo),
    Disconnected { pad_id: u32 },
    State(GamepadState),
}

impl GamepadEvent {
    pub fn pad_id(&self) -> u32 {
        match self {
            GamepadEvent::Connected(info) => info.pad_id,
            GamepadEvent::Disconnected { pad_id } => *pad_id,
            GamepadEvent::State(state) => state.pad_id,
        }
    }
}

/// A rumble effect an app plays on a controller: the strong (low frequency) and weak (high
/// frequency) motors from 0 to 65535, for `duration_ms` (0 until the next one).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GamepadRumble {
    pub pad_id: u32,
    pub strong: u16,
    pub weak: u16,
    pub duration_ms: u32,
}

impl GamepadRumble {
    /// The rumble a guest's message asks for, its strengths kept to what a motor takes.
    pub fn from_control(rumble: &control::GamepadRumble) -> GamepadRumble {
        GamepadRumble {
            pad_id: rumble.pad_id,
            strong: rumble.strong.min(u32::from(u16::MAX)) as u16,
            weak: rumble.weak.min(u32::from(u16::MAX)) as u16,
            duration_ms: rumble.duration_ms,
        }
    }
}

/// `name` cut to [`MAX_NAME`] bytes at a character boundary, without control characters.
fn clean_name(name: &str) -> String {
    let mut clean: String = name.chars().filter(|c| !c.is_control()).collect();
    if clean.len() > MAX_NAME {
        let mut end = MAX_NAME;
        while !clean.is_char_boundary(end) {
            end -= 1;
        }
        clean.truncate(end);
    }
    if clean.trim().is_empty() {
        "Gamepad".into()
    } else {
        clean
    }
}

impl GamepadEvent {
    /// The control message that carries this change to the guest.
    pub fn to_control(&self) -> control::envelope::Body {
        use control::envelope::Body;
        match self {
            GamepadEvent::Connected(info) => Body::GamepadConnected(control::GamepadConnected {
                pad_id: info.pad_id,
                name: clean_name(&info.name),
                vendor_id: u32::from(info.vendor_id),
                product_id: u32::from(info.product_id),
                rumble: info.rumble,
            }),
            GamepadEvent::Disconnected { pad_id } => {
                Body::GamepadDisconnected(control::GamepadDisconnected { pad_id: *pad_id })
            }
            GamepadEvent::State(state) => Body::GamepadState(control::GamepadState {
                pad_id: state.pad_id,
                buttons: state.buttons & buttons::ALL,
                left_x: i32::from(state.left_x),
                left_y: i32::from(state.left_y),
                right_x: i32::from(state.right_x),
                right_y: i32::from(state.right_y),
                left_trigger: u32::from(state.left_trigger),
                right_trigger: u32::from(state.right_trigger),
            }),
        }
    }
}

/// The controllers connected now and the latest state of each, so a guest that comes (a new
/// boot, a control daemon that restarted, a thaw) is given them all.
#[derive(Debug, Clone, Default)]
pub struct GamepadTable {
    pads: Vec<(GamepadInfo, Option<GamepadState>)>,
}

impl GamepadTable {
    pub fn new() -> GamepadTable {
        GamepadTable::default()
    }

    /// Takes in a change, and says whether the guest is to be told of it: a state of a
    /// controller that is not connected, a second connection of one that is, a disconnection
    /// of one that is not, a state that changed nothing, and a controller past [`MAX_PADS`]
    /// are not.
    pub fn apply(&mut self, event: &GamepadEvent) -> bool {
        let index = self
            .pads
            .iter()
            .position(|(info, _)| info.pad_id == event.pad_id());
        match (event, index) {
            (GamepadEvent::Connected(_), Some(_)) => false,
            (GamepadEvent::Connected(info), None) => {
                if self.pads.len() >= MAX_PADS {
                    return false;
                }
                self.pads.push((info.clone(), None));
                true
            }
            (GamepadEvent::Disconnected { .. }, Some(index)) => {
                self.pads.remove(index);
                true
            }
            (GamepadEvent::Disconnected { .. }, None) => false,
            (GamepadEvent::State(state), Some(index)) => {
                let last = &mut self.pads[index].1;
                if *last == Some(*state) {
                    return false;
                }
                *last = Some(*state);
                true
            }
            (GamepadEvent::State(_), None) => false,
        }
    }

    /// What a guest that has just come is told: every controller, then its latest state.
    pub fn replay(&self) -> Vec<GamepadEvent> {
        let mut events = Vec::new();
        for (info, state) in &self.pads {
            events.push(GamepadEvent::Connected(info.clone()));
            if let Some(state) = state {
                events.push(GamepadEvent::State(*state));
            }
        }
        events
    }

    /// The controllers connected now.
    pub fn connected(&self) -> Vec<u32> {
        self.pads.iter().map(|(info, _)| info.pad_id).collect()
    }
}

/// Reads the host's controllers for [`follow`], and runs their motors.
pub trait GamepadSource {
    /// The controllers connected now, each with its state now.
    fn read(&mut self) -> Vec<(GamepadInfo, GamepadState)>;
    /// Runs a controller's motors at these strengths until told otherwise.
    fn rumble(&mut self, pad_id: u32, strong: u16, weak: u16);
}

/// How often [`follow`] reads the controllers: a little faster than a game's frames.
pub const POLL: Duration = Duration::from_millis(8);

/// How long [`follow`] waits before trying the service again.
const RETRY: Duration = Duration::from_secs(2);

/// How long [`follow`] waits for the service to answer.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// What reading the controllers changed, compared with the controllers `known`: the new
/// ones, those that are gone, and each state. While `active` is false (no window of the
/// user's Android is in front) every state reads as nothing held, so a game does not act on
/// what the user does in another program.
pub fn changes(
    known: &[u32],
    now: &[(GamepadInfo, GamepadState)],
    active: bool,
) -> Vec<GamepadEvent> {
    let mut events = Vec::new();
    for pad_id in known {
        if !now.iter().any(|(info, _)| info.pad_id == *pad_id) {
            events.push(GamepadEvent::Disconnected { pad_id: *pad_id });
        }
    }
    for (info, state) in now {
        if !known.contains(&info.pad_id) {
            events.push(GamepadEvent::Connected(info.clone()));
        }
        let state = if active {
            GamepadState {
                pad_id: info.pad_id,
                ..*state
            }
        } else {
            GamepadState {
                pad_id: info.pad_id,
                ..GamepadState::default()
            }
        };
        events.push(GamepadEvent::State(state));
    }
    events
}

/// Carries the host's controllers to the user's Android until `stop` is set: every
/// connection, disconnection and change of state, and each rumble effect an app plays back to
/// the controller's motors, for as long as it lasts. `active` says whether a window of the
/// user's Android is in front.
pub fn follow(
    connector: &mut dyn Connector,
    build: &str,
    source: &mut dyn GamepadSource,
    active: &dyn Fn() -> bool,
    stop: &AtomicBool,
    log: &dyn Fn(&str),
) {
    let mut warned = false;
    while !stop.load(Ordering::Relaxed) {
        let stream = HostApiClient::connect(connector, build, CONNECT_TIMEOUT)
            .and_then(|client| client.watch_gamepads());
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(why) => {
                if !warned {
                    log(&format!(
                        "gamepads: cannot give Android the controllers yet: {why}"
                    ));
                    warned = true;
                }
                sleep_unless(stop, RETRY);
                continue;
            }
        };
        warned = false;
        // A new connection knows no controller.
        let mut table = GamepadTable::new();
        let mut rumbling: Vec<(u32, Instant)> = Vec::new();
        'connected: while !stop.load(Ordering::Relaxed) {
            let now = source.read();
            for event in changes(&table.connected(), &now, active()) {
                match &event {
                    GamepadEvent::Connected(info) => {
                        log(&format!("gamepads: {} connected", info.name))
                    }
                    GamepadEvent::Disconnected { pad_id } => {
                        log(&format!("gamepads: controller {pad_id} disconnected"))
                    }
                    GamepadEvent::State(_) => {}
                }
                if table.apply(&event) {
                    if let Err(why) = stream.send(&event) {
                        log(&format!("gamepads: the service connection ended: {why}"));
                        break 'connected;
                    }
                }
            }
            let effect = match stream.next(POLL) {
                Ok(effect) => effect,
                Err(why) => {
                    log(&format!("gamepads: the service connection ended: {why}"));
                    break 'connected;
                }
            };
            if let Some(effect) = effect {
                if table.connected().contains(&effect.pad_id) {
                    source.rumble(effect.pad_id, effect.strong, effect.weak);
                    rumbling.retain(|(pad_id, _)| *pad_id != effect.pad_id);
                    let playing = effect.strong > 0 || effect.weak > 0;
                    if playing && effect.duration_ms > 0 {
                        let length = Duration::from_millis(effect.duration_ms.into());
                        rumbling.push((effect.pad_id, Instant::now() + length));
                    }
                }
            }
            let instant = Instant::now();
            rumbling.retain(|(pad_id, until)| {
                if instant >= *until {
                    source.rumble(*pad_id, 0, 0);
                    false
                } else {
                    true
                }
            });
        }
        // Nothing keeps rumbling once nobody can stop it.
        for (pad_id, _) in rumbling {
            source.rumble(pad_id, 0, 0);
        }
        sleep_unless(stop, RETRY);
    }
}

fn sleep_unless(stop: &AtomicBool, wait: Duration) {
    let deadline = Instant::now() + wait;
    while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(pad_id: u32) -> GamepadInfo {
        GamepadInfo {
            pad_id,
            name: "Xbox Controller".into(),
            vendor_id: 0x045e,
            product_id: 0x028e,
            rumble: true,
        }
    }

    #[test]
    fn fr30_a_guest_that_comes_is_given_every_controller_and_its_latest_state() {
        let mut table = GamepadTable::new();
        assert!(table.apply(&GamepadEvent::Connected(pad(1))));
        assert!(table.apply(&GamepadEvent::Connected(pad(2))));
        let pressed = GamepadState {
            pad_id: 1,
            buttons: buttons::A,
            left_x: -32768,
            ..GamepadState::default()
        };
        assert!(table.apply(&GamepadEvent::State(pressed)));
        assert!(!table.apply(&GamepadEvent::State(pressed)));
        assert!(table.apply(&GamepadEvent::Disconnected { pad_id: 2 }));
        assert_eq!(
            table.replay(),
            vec![
                GamepadEvent::Connected(pad(1)),
                GamepadEvent::State(pressed)
            ]
        );
    }

    #[test]
    fn fr30_a_state_of_a_controller_that_is_not_connected_is_not_passed_on() {
        let mut table = GamepadTable::new();
        assert!(!table.apply(&GamepadEvent::State(GamepadState {
            pad_id: 3,
            ..GamepadState::default()
        })));
        assert!(!table.apply(&GamepadEvent::Disconnected { pad_id: 3 }));
        for pad_id in 0..(MAX_PADS as u32 + 2) {
            table.apply(&GamepadEvent::Connected(pad(pad_id)));
        }
        assert_eq!(table.connected().len(), MAX_PADS);
    }

    #[test]
    fn fr30_a_controllers_state_reaches_the_guest_with_the_same_values() {
        let state = GamepadState {
            pad_id: 1,
            buttons: buttons::B | buttons::DPAD_LEFT,
            left_x: 32767,
            left_y: -100,
            right_x: -32768,
            right_y: 5,
            left_trigger: 255,
            right_trigger: 3,
        };
        assert_eq!(
            GamepadEvent::State(state).to_control(),
            control::envelope::Body::GamepadState(control::GamepadState {
                pad_id: 1,
                buttons: buttons::B | buttons::DPAD_LEFT,
                left_x: 32767,
                left_y: -100,
                right_x: -32768,
                right_y: 5,
                left_trigger: 255,
                right_trigger: 3,
            })
        );
    }

    #[test]
    fn fr30_a_controllers_name_is_cleaned_before_it_reaches_the_guest() {
        let info = GamepadInfo {
            name: format!("Pad\n{}", "x".repeat(300)),
            ..pad(1)
        };
        let control::envelope::Body::GamepadConnected(connected) =
            GamepadEvent::Connected(info).to_control()
        else {
            panic!("expected a connection");
        };
        assert!(connected.name.starts_with("Padx"));
        assert_eq!(connected.name.len(), MAX_NAME);
        assert_eq!(clean_name("  "), "Gamepad");
    }

    #[test]
    fn fr30_a_rumble_from_the_guest_is_kept_to_what_a_motor_takes() {
        let rumble = GamepadRumble::from_control(&control::GamepadRumble {
            pad_id: 2,
            strong: 70_000,
            weak: 1000,
            duration_ms: 250,
        });
        assert_eq!(
            rumble,
            GamepadRumble {
                pad_id: 2,
                strong: u16::MAX,
                weak: 1000,
                duration_ms: 250
            }
        );
    }
}
