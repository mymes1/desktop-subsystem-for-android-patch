//! The user's game controllers for their Android, read through XInput.
//!
//! The service runs in session 0, where no controller is read, so the window manager, which
//! runs in the user's session, reads them and gives them to the service
//! ([`dsa_host_core::gamepads::follow`]). XInput sees every controller Windows treats as an
//! Xbox controller, which covers the Xbox controllers and the many that present themselves as
//! one, wired or wireless. Each becomes a gamepad of its own in Android, with the USB IDs of
//! an Xbox 360 controller so Android's own key layout for it applies. The buttons, sticks and
//! triggers reach Android only while one of its windows is in front, and a game's rumble runs
//! the controller's motors.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dsa_host_core::gamepads::{self, buttons, GamepadInfo, GamepadSource, GamepadState};
use dsa_transport::Connector;

use super::hosts::any_window_focused;
use super::log::{note, CompositorLogger};

/// The build the window manager names itself by when it carries the controllers.
const BUILD: &str = "dsa-wm gamepads";

/// XInput's controller slots.
const SLOTS: u32 = 4;

/// How often a slot with no controller is looked at again: asking XInput about an empty slot
/// takes long enough that it is not done on every read.
const EMPTY_SLOT_POLL: Duration = Duration::from_secs(1);

/// The USB IDs the guest's gamepad reports: an Xbox 360 controller's, whose key layout every
/// Android has.
const XBOX_VENDOR: u16 = 0x045e;
const XBOX_PRODUCT: u16 = 0x028e;

/// XInput's button bits (`XINPUT_GAMEPAD_*`).
mod xinput {
    pub const DPAD_UP: u16 = 0x0001;
    pub const DPAD_DOWN: u16 = 0x0002;
    pub const DPAD_LEFT: u16 = 0x0004;
    pub const DPAD_RIGHT: u16 = 0x0008;
    pub const START: u16 = 0x0010;
    pub const BACK: u16 = 0x0020;
    pub const LEFT_THUMB: u16 = 0x0040;
    pub const RIGHT_THUMB: u16 = 0x0080;
    pub const LEFT_SHOULDER: u16 = 0x0100;
    pub const RIGHT_SHOULDER: u16 = 0x0200;
    pub const A: u16 = 0x1000;
    pub const B: u16 = 0x2000;
    pub const X: u16 = 0x4000;
    pub const Y: u16 = 0x8000;
}

/// The buttons of an XInput `wButtons` as the control protocol's bits.
pub fn buttons_from_xinput(pressed: u16) -> u32 {
    const MAP: [(u16, u32); 14] = [
        (xinput::DPAD_UP, buttons::DPAD_UP),
        (xinput::DPAD_DOWN, buttons::DPAD_DOWN),
        (xinput::DPAD_LEFT, buttons::DPAD_LEFT),
        (xinput::DPAD_RIGHT, buttons::DPAD_RIGHT),
        (xinput::START, buttons::START),
        (xinput::BACK, buttons::BACK),
        (xinput::LEFT_THUMB, buttons::LEFT_THUMB),
        (xinput::RIGHT_THUMB, buttons::RIGHT_THUMB),
        (xinput::LEFT_SHOULDER, buttons::LEFT_SHOULDER),
        (xinput::RIGHT_SHOULDER, buttons::RIGHT_SHOULDER),
        (xinput::A, buttons::A),
        (xinput::B, buttons::B),
        (xinput::X, buttons::X),
        (xinput::Y, buttons::Y),
    ];
    MAP.iter()
        .filter(|(bit, _)| pressed & bit != 0)
        .fold(0, |all, (_, ours)| all | ours)
}

/// A stick's vertical axis: XInput's grows upward, Android's downward.
pub fn flip_axis(value: i16) -> i16 {
    (-i32::from(value)).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// The state of a controller in `slot`, from the fields of its `XINPUT_GAMEPAD`.
#[allow(clippy::too_many_arguments)]
pub fn state_from_xinput(
    slot: u32,
    pressed: u16,
    left_trigger: u8,
    right_trigger: u8,
    thumb_lx: i16,
    thumb_ly: i16,
    thumb_rx: i16,
    thumb_ry: i16,
) -> GamepadState {
    GamepadState {
        pad_id: slot,
        buttons: buttons_from_xinput(pressed),
        left_x: thumb_lx,
        left_y: flip_axis(thumb_ly),
        right_x: thumb_rx,
        right_y: flip_axis(thumb_ry),
        left_trigger,
        right_trigger,
    }
}

/// The controllers XInput reports.
pub struct XInputPads {
    /// Each slot's controller while one is there.
    present: [bool; SLOTS as usize],
    /// When each empty slot was last looked at.
    looked: [Option<Instant>; SLOTS as usize],
}

impl Default for XInputPads {
    fn default() -> XInputPads {
        XInputPads {
            present: [false; SLOTS as usize],
            looked: [None; SLOTS as usize],
        }
    }
}

impl GamepadSource for XInputPads {
    fn read(&mut self) -> Vec<(GamepadInfo, GamepadState)> {
        use windows_sys::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};
        let mut pads = Vec::new();
        for slot in 0..SLOTS {
            let index = slot as usize;
            if !self.present[index] {
                if let Some(looked) = self.looked[index] {
                    if looked.elapsed() < EMPTY_SLOT_POLL {
                        continue;
                    }
                }
                self.looked[index] = Some(Instant::now());
            }
            // SAFETY: a plain structure XInput fills in.
            let mut state: XINPUT_STATE = unsafe { std::mem::zeroed() };
            // SAFETY: a slot index XInput takes and a valid pointer to that structure.
            let result = unsafe { XInputGetState(slot, &mut state) };
            self.present[index] = result == 0;
            if result != 0 {
                continue;
            }
            let pad = state.Gamepad;
            pads.push((
                GamepadInfo {
                    pad_id: slot,
                    name: format!("Xbox Controller {}", slot + 1),
                    vendor_id: XBOX_VENDOR,
                    product_id: XBOX_PRODUCT,
                    rumble: true,
                },
                state_from_xinput(
                    slot,
                    pad.wButtons as u16,
                    pad.bLeftTrigger,
                    pad.bRightTrigger,
                    pad.sThumbLX,
                    pad.sThumbLY,
                    pad.sThumbRX,
                    pad.sThumbRY,
                ),
            ));
        }
        pads
    }

    fn rumble(&mut self, pad_id: u32, strong: u16, weak: u16) {
        use windows_sys::Win32::UI::Input::XboxController::{XInputSetState, XINPUT_VIBRATION};
        if pad_id >= SLOTS {
            return;
        }
        // The left motor is the strong, low-frequency one.
        let mut vibration = XINPUT_VIBRATION {
            wLeftMotorSpeed: strong,
            wRightMotorSpeed: weak,
        };
        // SAFETY: a slot index XInput takes and a valid pointer to the structure.
        unsafe { XInputSetState(pad_id, &mut vibration) };
    }
}

/// Carries the user's controllers to their Android until `stop` is set.
pub fn follow(
    mut connector: Box<dyn Connector>,
    stop: Arc<AtomicBool>,
    logger: Option<Arc<CompositorLogger>>,
) {
    let mut pads = XInputPads::default();
    let log = |line: &str| note(logger.as_deref(), line);
    gamepads::follow(
        &mut *connector,
        BUILD,
        &mut pads,
        &any_window_focused,
        &stop,
        &log,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr30_xinputs_buttons_become_androids_gamepad_buttons() {
        assert_eq!(
            buttons_from_xinput(xinput::A | xinput::DPAD_LEFT | xinput::RIGHT_SHOULDER),
            buttons::A | buttons::DPAD_LEFT | buttons::RIGHT_SHOULDER
        );
        assert_eq!(buttons_from_xinput(0), 0);
        assert_eq!(buttons_from_xinput(0xffff) & buttons::GUIDE, 0);
    }

    #[test]
    fn fr30_a_stick_pushed_up_reads_as_up_in_android() {
        let state = state_from_xinput(1, 0, 10, 255, 100, 32767, -32768, -32768);
        assert_eq!(state.pad_id, 1);
        assert_eq!(state.left_y, -32767);
        assert_eq!(state.right_x, -32768);
        assert_eq!(state.right_y, 32767);
        assert_eq!((state.left_trigger, state.right_trigger), (10, 255));
    }
}
