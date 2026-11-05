//! A game controller's evdev buttons and axes as the guest's gamepad state.
//!
//! The kernel's gamepad drivers report the standard layout (`BTN_SOUTH` for A, `ABS_X` for
//! the left stick, `ABS_Z` for the left trigger, a hat for the d-pad on most controllers and
//! `BTN_DPAD_*` on a few). Each axis has its own range, which the device states; this scales
//! it to the guest's.

use dsa_host_core::gamepads::{buttons, GamepadState};

/// The evdev codes a gamepad reports.
pub mod codes {
    pub const BTN_SOUTH: u16 = 0x130;
    pub const BTN_EAST: u16 = 0x131;
    pub const BTN_NORTH: u16 = 0x133;
    pub const BTN_WEST: u16 = 0x134;
    pub const BTN_TL: u16 = 0x136;
    pub const BTN_TR: u16 = 0x137;
    pub const BTN_TL2: u16 = 0x138;
    pub const BTN_TR2: u16 = 0x139;
    pub const BTN_SELECT: u16 = 0x13a;
    pub const BTN_START: u16 = 0x13b;
    pub const BTN_MODE: u16 = 0x13c;
    pub const BTN_THUMBL: u16 = 0x13d;
    pub const BTN_THUMBR: u16 = 0x13e;
    pub const BTN_DPAD_UP: u16 = 0x220;
    pub const BTN_DPAD_DOWN: u16 = 0x221;
    pub const BTN_DPAD_LEFT: u16 = 0x222;
    pub const BTN_DPAD_RIGHT: u16 = 0x223;

    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
    pub const ABS_Z: u16 = 0x02;
    pub const ABS_RX: u16 = 0x03;
    pub const ABS_RY: u16 = 0x04;
    pub const ABS_RZ: u16 = 0x05;
    pub const ABS_GAS: u16 = 0x09;
    pub const ABS_BRAKE: u16 = 0x0a;
    pub const ABS_HAT0X: u16 = 0x10;
    pub const ABS_HAT0Y: u16 = 0x11;
}

/// The guest's button for an evdev button code.
pub fn button_bit(code: u16) -> Option<u32> {
    use codes::*;
    Some(match code {
        BTN_SOUTH => buttons::A,
        BTN_EAST => buttons::B,
        BTN_NORTH => buttons::Y,
        BTN_WEST => buttons::X,
        BTN_TL => buttons::LEFT_SHOULDER,
        BTN_TR => buttons::RIGHT_SHOULDER,
        BTN_SELECT => buttons::BACK,
        BTN_START => buttons::START,
        BTN_MODE => buttons::GUIDE,
        BTN_THUMBL => buttons::LEFT_THUMB,
        BTN_THUMBR => buttons::RIGHT_THUMB,
        BTN_DPAD_UP => buttons::DPAD_UP,
        BTN_DPAD_DOWN => buttons::DPAD_DOWN,
        BTN_DPAD_LEFT => buttons::DPAD_LEFT,
        BTN_DPAD_RIGHT => buttons::DPAD_RIGHT,
        _ => return None,
    })
}

/// An axis's range as the device states it (`EVIOCGABS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisRange {
    pub minimum: i32,
    pub maximum: i32,
}

impl AxisRange {
    /// `value` on a stick, from -32768 to 32767.
    pub fn stick(&self, value: i32) -> i16 {
        let span = i64::from(self.maximum) - i64::from(self.minimum);
        if span <= 0 {
            return 0;
        }
        let offset = i64::from(value.clamp(self.minimum, self.maximum)) - i64::from(self.minimum);
        (offset * 65535 / span - 32768) as i16
    }

    /// `value` on a trigger, from 0 to 255.
    pub fn trigger(&self, value: i32) -> u8 {
        let span = i64::from(self.maximum) - i64::from(self.minimum);
        if span <= 0 {
            return 0;
        }
        let offset = i64::from(value.clamp(self.minimum, self.maximum)) - i64::from(self.minimum);
        (offset * 255 / span) as u8
    }
}

/// What a controller reports at one moment: the evdev buttons held, and each axis's value
/// with its range.
#[derive(Debug, Clone, Default)]
pub struct RawPad {
    pub held: Vec<u16>,
    pub axes: Vec<(u16, i32, AxisRange)>,
}

/// The guest's state for what a controller reports. A trigger that only has a button
/// (`BTN_TL2`) reads as fully pressed while it is held.
pub fn state(pad_id: u32, raw: &RawPad) -> GamepadState {
    use codes::*;
    let mut state = GamepadState {
        pad_id,
        ..GamepadState::default()
    };
    for code in &raw.held {
        if let Some(bit) = button_bit(*code) {
            state.buttons |= bit;
        }
        match *code {
            BTN_TL2 => state.left_trigger = 255,
            BTN_TR2 => state.right_trigger = 255,
            _ => {}
        }
    }
    for (code, value, range) in &raw.axes {
        match *code {
            ABS_X => state.left_x = range.stick(*value),
            ABS_Y => state.left_y = range.stick(*value),
            ABS_RX => state.right_x = range.stick(*value),
            ABS_RY => state.right_y = range.stick(*value),
            ABS_Z | ABS_BRAKE => state.left_trigger = range.trigger(*value),
            ABS_RZ | ABS_GAS => state.right_trigger = range.trigger(*value),
            ABS_HAT0X if *value < 0 => state.buttons |= buttons::DPAD_LEFT,
            ABS_HAT0X if *value > 0 => state.buttons |= buttons::DPAD_RIGHT,
            ABS_HAT0Y if *value < 0 => state.buttons |= buttons::DPAD_UP,
            ABS_HAT0Y if *value > 0 => state.buttons |= buttons::DPAD_DOWN,
            _ => {}
        }
    }
    state
}

/// Whether a device with these capabilities is a game controller: it has the south face
/// button and a stick. A keyboard or a mouse has neither.
pub fn is_gamepad(has_south_button: bool, has_left_stick: bool) -> bool {
    has_south_button && has_left_stick
}

#[cfg(test)]
mod tests {
    use super::*;

    const STICK: AxisRange = AxisRange {
        minimum: -32768,
        maximum: 32767,
    };
    const BYTE_STICK: AxisRange = AxisRange {
        minimum: 0,
        maximum: 255,
    };
    const TRIGGER: AxisRange = AxisRange {
        minimum: 0,
        maximum: 1023,
    };

    #[test]
    fn fr30_face_buttons_and_shoulders_are_named_as_the_guest_names_them() {
        let raw = RawPad {
            held: vec![
                codes::BTN_SOUTH,
                codes::BTN_WEST,
                codes::BTN_TR,
                codes::BTN_START,
            ],
            axes: vec![],
        };
        let state = state(2, &raw);
        assert_eq!(state.pad_id, 2);
        assert_eq!(
            state.buttons,
            buttons::A | buttons::X | buttons::RIGHT_SHOULDER | buttons::START
        );
    }

    #[test]
    fn fr30_sticks_and_triggers_scale_to_the_guests_ranges() {
        let raw = RawPad {
            held: vec![],
            axes: vec![
                (codes::ABS_X, 32767, STICK),
                (codes::ABS_Y, -32768, STICK),
                (codes::ABS_RX, 0, BYTE_STICK),
                (codes::ABS_RY, 255, BYTE_STICK),
                (codes::ABS_Z, 1023, TRIGGER),
                (codes::ABS_RZ, 0, TRIGGER),
            ],
        };
        let state = state(0, &raw);
        assert_eq!(state.left_x, 32767);
        assert_eq!(state.left_y, -32768);
        assert_eq!(state.right_x, -32768);
        assert_eq!(state.right_y, 32767);
        assert_eq!(state.left_trigger, 255);
        assert_eq!(state.right_trigger, 0);
    }

    #[test]
    fn fr30_the_hat_is_the_dpad() {
        let hat = AxisRange {
            minimum: -1,
            maximum: 1,
        };
        let raw = RawPad {
            held: vec![],
            axes: vec![(codes::ABS_HAT0X, -1, hat), (codes::ABS_HAT0Y, 1, hat)],
        };
        assert_eq!(
            state(0, &raw).buttons,
            buttons::DPAD_LEFT | buttons::DPAD_DOWN
        );
    }

    #[test]
    fn fr30_a_digital_trigger_reads_as_fully_pressed() {
        let raw = RawPad {
            held: vec![codes::BTN_TL2],
            axes: vec![],
        };
        assert_eq!(state(0, &raw).left_trigger, 255);
    }

    #[test]
    fn fr30_a_device_without_a_face_button_or_a_stick_is_not_a_controller() {
        assert!(is_gamepad(true, true));
        assert!(!is_gamepad(false, true));
        assert!(!is_gamepad(true, false));
    }
}
