//! Game controllers through the GameController framework: Xbox, PlayStation, Switch and
//! MFi controllers, read as the extended gamepad profile, with their motors run through
//! Core Haptics.

use dsa_host_core::gamepads::{buttons, GamepadState};

/// A stick axis from GameController (-1 to 1, up positive) as the guest's (-32768 to 32767,
/// down positive when `vertical`).
pub fn stick(value: f32, vertical: bool) -> i16 {
    let value = if value.is_finite() { value.clamp(-1.0, 1.0) } else { 0.0 };
    let value = if vertical { -value } else { value };
    if value >= 0.0 {
        (value * 32767.0).round() as i16
    } else {
        (value * 32768.0).round() as i16
    }
}

/// A trigger from GameController (0 to 1) as the guest's (0 to 255).
pub fn trigger(value: f32) -> u8 {
    let value = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
    (value * 255.0).round() as u8
}

/// What an extended gamepad says at one moment, as GameController reads it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ExtendedGamepad {
    pub a: bool,
    pub b: bool,
    pub x: bool,
    pub y: bool,
    pub left_shoulder: bool,
    pub right_shoulder: bool,
    /// The options (view, share, minus) button.
    pub options: bool,
    /// The menu (start, plus) button.
    pub menu: bool,
    pub home: bool,
    pub left_thumb: bool,
    pub right_thumb: bool,
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub left_stick: (f32, f32),
    pub right_stick: (f32, f32),
    pub left_trigger: f32,
    pub right_trigger: f32,
}

/// The guest's state for controller `pad_id` reading `pad`.
pub fn state(pad_id: u32, pad: &ExtendedGamepad) -> GamepadState {
    let mut bits = 0;
    for (held, bit) in [
        (pad.a, buttons::A),
        (pad.b, buttons::B),
        (pad.x, buttons::X),
        (pad.y, buttons::Y),
        (pad.left_shoulder, buttons::LEFT_SHOULDER),
        (pad.right_shoulder, buttons::RIGHT_SHOULDER),
        (pad.options, buttons::BACK),
        (pad.menu, buttons::START),
        (pad.home, buttons::GUIDE),
        (pad.left_thumb, buttons::LEFT_THUMB),
        (pad.right_thumb, buttons::RIGHT_THUMB),
        (pad.up, buttons::DPAD_UP),
        (pad.down, buttons::DPAD_DOWN),
        (pad.left, buttons::DPAD_LEFT),
        (pad.right, buttons::DPAD_RIGHT),
    ] {
        if held {
            bits |= bit;
        }
    }
    GamepadState {
        pad_id,
        buttons: bits,
        left_x: stick(pad.left_stick.0, false),
        left_y: stick(pad.left_stick.1, true),
        right_x: stick(pad.right_stick.0, false),
        right_y: stick(pad.right_stick.1, true),
        left_trigger: trigger(pad.left_trigger),
        right_trigger: trigger(pad.right_trigger),
    }
}

/// A motor strength from the guest (0 to 65535) as Core Haptics' intensity (0 to 1).
pub fn intensity(strong: u16, weak: u16) -> f32 {
    f32::from(strong.max(weak)) / 65535.0
}

#[cfg(target_os = "macos")]
pub use source::GameControllers;

#[cfg(target_os = "macos")]
mod source {
    use std::collections::HashMap;

    use dsa_host_core::gamepads::{GamepadInfo, GamepadSource, GamepadState, MAX_PADS};
    use objc2::msg_send;
    use objc2::rc::{autoreleasepool, Allocated, Retained};
    use objc2::runtime::AnyObject;

    use super::{intensity, state, ExtendedGamepad};
    use crate::objc::{class, ns_string, string_of};

    #[link(name = "GameController", kind = "framework")]
    extern "C" {}
    #[link(name = "CoreHaptics", kind = "framework")]
    extern "C" {}

    /// How long one rumble runs unless the guest changes it first; the guest's own rumbles
    /// are shorter and always end with a zero.
    const RUMBLE_SECONDS: f64 = 30.0;

    unsafe fn pressed(element: *mut AnyObject) -> bool {
        !element.is_null() && msg_send![element, isPressed]
    }

    unsafe fn value(element: *mut AnyObject) -> f32 {
        if element.is_null() {
            0.0
        } else {
            msg_send![element, value]
        }
    }

    unsafe fn axes(stick: *mut AnyObject) -> (f32, f32) {
        if stick.is_null() {
            return (0.0, 0.0);
        }
        (
            value(msg_send![stick, xAxis]),
            value(msg_send![stick, yAxis]),
        )
    }

    unsafe fn read_pad(pad: *mut AnyObject) -> ExtendedGamepad {
        let dpad: *mut AnyObject = msg_send![pad, dpad];
        let button = |name: objc2::runtime::Sel| -> bool {
            let responds: bool = msg_send![pad, respondsToSelector: name];
            responds && pressed(msg_send![pad, performSelector: name])
        };
        ExtendedGamepad {
            a: pressed(msg_send![pad, buttonA]),
            b: pressed(msg_send![pad, buttonB]),
            x: pressed(msg_send![pad, buttonX]),
            y: pressed(msg_send![pad, buttonY]),
            left_shoulder: pressed(msg_send![pad, leftShoulder]),
            right_shoulder: pressed(msg_send![pad, rightShoulder]),
            options: button(objc2::sel!(buttonOptions)),
            menu: pressed(msg_send![pad, buttonMenu]),
            home: button(objc2::sel!(buttonHome)),
            left_thumb: button(objc2::sel!(leftThumbstickButton)),
            right_thumb: button(objc2::sel!(rightThumbstickButton)),
            up: !dpad.is_null() && pressed(msg_send![dpad, up]),
            down: !dpad.is_null() && pressed(msg_send![dpad, down]),
            left: !dpad.is_null() && pressed(msg_send![dpad, left]),
            right: !dpad.is_null() && pressed(msg_send![dpad, right]),
            left_stick: axes(msg_send![pad, leftThumbstick]),
            right_stick: axes(msg_send![pad, rightThumbstick]),
            left_trigger: value(msg_send![pad, leftTrigger]),
            right_trigger: value(msg_send![pad, rightTrigger]),
        }
    }

    struct Motor {
        engine: Retained<AnyObject>,
        player: Option<Retained<AnyObject>>,
    }

    /// The controllers connected to the Mac.
    #[derive(Default)]
    pub struct GameControllers {
        /// The guest's number for each controller, by the controller object's address, kept
        /// while it stays connected.
        ids: HashMap<usize, u32>,
        next_id: u32,
        motors: HashMap<u32, Motor>,
        controllers: HashMap<u32, Retained<AnyObject>>,
    }

    // Controllers are read on the gamepad thread only.
    unsafe impl Send for GameControllers {}

    impl GameControllers {
        pub fn new() -> GameControllers {
            unsafe {
                // Controllers are reported even while no DSA window is in front, as a game
                // in a window behind another still reads its controller.
                if let Ok(class) = class(c"GCController") {
                    let responds: bool = msg_send![
                        class,
                        respondsToSelector: objc2::sel!(setShouldMonitorBackgroundEvents:)
                    ];
                    if responds {
                        let _: () = msg_send![class, setShouldMonitorBackgroundEvents: true];
                    }
                }
            }
            GameControllers::default()
        }

        unsafe fn motor(&mut self, pad_id: u32) -> Option<&mut Motor> {
            if !self.motors.contains_key(&pad_id) {
                let controller = self.controllers.get(&pad_id)?;
                let haptics: *mut AnyObject = msg_send![&**controller, haptics];
                if haptics.is_null() {
                    return None;
                }
                let locality = ns_string("Default");
                let engine: Option<Retained<AnyObject>> =
                    msg_send![haptics, createEngineWithLocality: &*locality];
                let engine = engine?;
                let mut error: *mut AnyObject = std::ptr::null_mut();
                let started: bool = msg_send![&*engine, startAndReturnError: &mut error];
                if !started {
                    return None;
                }
                self.motors.insert(
                    pad_id,
                    Motor {
                        engine,
                        player: None,
                    },
                );
            }
            self.motors.get_mut(&pad_id)
        }
    }

    impl GamepadSource for GameControllers {
        fn read(&mut self) -> Vec<(GamepadInfo, GamepadState)> {
            autoreleasepool(|_| unsafe {
                let Ok(class) = class(c"GCController") else {
                    return Vec::new();
                };
                let list: *mut AnyObject = msg_send![class, controllers];
                let count: usize = if list.is_null() {
                    0
                } else {
                    msg_send![list, count]
                };
                let mut seen = HashMap::new();
                let mut out = Vec::new();
                for i in 0..count.min(MAX_PADS) {
                    let controller: *mut AnyObject = msg_send![list, objectAtIndex: i];
                    let pad: *mut AnyObject = msg_send![controller, extendedGamepad];
                    if pad.is_null() {
                        continue;
                    }
                    let key = controller as usize;
                    let pad_id = match self.ids.get(&key) {
                        Some(id) => *id,
                        None => {
                            self.next_id = self.next_id.wrapping_add(1);
                            self.next_id
                        }
                    };
                    seen.insert(key, pad_id);
                    if let Some(retained) = Retained::retain(controller) {
                        self.controllers.insert(pad_id, retained);
                    }
                    let haptics: *mut AnyObject = msg_send![controller, haptics];
                    let info = GamepadInfo {
                        pad_id,
                        name: string_of(msg_send![controller, vendorName]),
                        // GameController does not name the USB vendor and product.
                        vendor_id: 0,
                        product_id: 0,
                        rumble: !haptics.is_null(),
                    };
                    out.push((info, state(pad_id, &read_pad(pad))));
                }
                let gone: Vec<u32> = self
                    .ids
                    .values()
                    .filter(|id| !seen.values().any(|s| s == *id))
                    .copied()
                    .collect();
                for id in gone {
                    self.motors.remove(&id);
                    self.controllers.remove(&id);
                }
                self.ids = seen;
                out
            })
        }

        fn rumble(&mut self, pad_id: u32, strong: u16, weak: u16) {
            autoreleasepool(|_| unsafe {
                let level = intensity(strong, weak);
                let Some(motor) = self.motor(pad_id) else {
                    return;
                };
                if let Some(player) = motor.player.take() {
                    let mut error: *mut AnyObject = std::ptr::null_mut();
                    let _: bool = msg_send![&*player, stopAtTime: 0.0f64, error: &mut error];
                }
                if level <= 0.0 {
                    return;
                }
                let (Ok(param_class), Ok(event_class), Ok(pattern_class), Ok(array_class)) = (
                    class(c"CHHapticEventParameter"),
                    class(c"CHHapticEvent"),
                    class(c"CHHapticPattern"),
                    class(c"NSArray"),
                ) else {
                    return;
                };
                let param: Option<Retained<AnyObject>> = msg_send![
                    { let a: Allocated<AnyObject> = msg_send![param_class, alloc]; a },
                    initWithParameterID: &*ns_string("HapticIntensity"),
                    value: level
                ];
                let Some(param) = param else { return };
                let params: *mut AnyObject = msg_send![array_class, arrayWithObject: &*param];
                let event: Option<Retained<AnyObject>> = msg_send![
                    { let a: Allocated<AnyObject> = msg_send![event_class, alloc]; a },
                    initWithEventType: &*ns_string("HapticContinuous"),
                    parameters: params,
                    relativeTime: 0.0f64,
                    duration: RUMBLE_SECONDS
                ];
                let Some(event) = event else { return };
                let events: *mut AnyObject = msg_send![array_class, arrayWithObject: &*event];
                let empty: *mut AnyObject = msg_send![array_class, array];
                let mut error: *mut AnyObject = std::ptr::null_mut();
                let pattern: Option<Retained<AnyObject>> = msg_send![
                    { let a: Allocated<AnyObject> = msg_send![pattern_class, alloc]; a },
                    initWithEvents: events,
                    parameters: empty,
                    error: &mut error
                ];
                let Some(pattern) = pattern else { return };
                let player: Option<Retained<AnyObject>> = msg_send![
                    &*motor.engine,
                    createPlayerWithPattern: &*pattern,
                    error: &mut error
                ];
                let Some(player) = player else { return };
                let started: bool = msg_send![&*player, startAtTime: 0.0f64, error: &mut error];
                if started {
                    motor.player = Some(player);
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr30_sticks_reach_the_guest_with_down_positive() {
        assert_eq!(stick(1.0, false), 32767);
        assert_eq!(stick(-1.0, false), -32768);
        assert_eq!(stick(1.0, true), -32768);
        assert_eq!(stick(-1.0, true), 32767);
        assert_eq!(stick(0.0, true), 0);
        assert_eq!(stick(f32::NAN, false), 0);
        assert_eq!(stick(3.0, false), 32767);
    }

    #[test]
    fn fr30_triggers_reach_the_guest_from_0_to_255() {
        assert_eq!(trigger(0.0), 0);
        assert_eq!(trigger(1.0), 255);
        assert_eq!(trigger(0.5), 128);
        assert_eq!(trigger(-1.0), 0);
    }

    #[test]
    fn fr30_every_button_of_an_extended_gamepad_has_its_bit() {
        let all = ExtendedGamepad {
            a: true,
            b: true,
            x: true,
            y: true,
            left_shoulder: true,
            right_shoulder: true,
            options: true,
            menu: true,
            home: true,
            left_thumb: true,
            right_thumb: true,
            up: true,
            down: true,
            left: true,
            right: true,
            ..ExtendedGamepad::default()
        };
        assert_eq!(state(3, &all).buttons, buttons::ALL);
        let menu = ExtendedGamepad {
            menu: true,
            ..ExtendedGamepad::default()
        };
        assert_eq!(state(3, &menu).buttons, buttons::START);
        assert_eq!(state(3, &menu).pad_id, 3);
    }

    #[test]
    fn fr30_the_stronger_motor_sets_the_rumble() {
        assert_eq!(intensity(0, 0), 0.0);
        assert_eq!(intensity(65535, 0), 1.0);
        assert!((intensity(0, 32768) - 0.5).abs() < 0.001);
    }
}
