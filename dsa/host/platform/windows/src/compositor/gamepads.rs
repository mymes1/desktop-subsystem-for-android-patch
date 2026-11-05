//! The user's game controllers for their Android, read through XInput and
//! Windows.Gaming.Input.
//!
//! The service runs in session 0, where no controller is read, so the window manager, which
//! runs in the user's session, reads them and gives them to the service
//! ([`dsa_host_core::gamepads::follow`]). XInput sees every controller Windows treats as an
//! Xbox controller, which covers the Xbox controllers and the many that present themselves as
//! one, wired or wireless. Every other controller Windows sees (a DualShock 4 or DualSense, a
//! Switch Pro controller, any HID gamepad) is read through Windows.Gaming.Input's
//! RawGameController, with its own button layout or a generic one; those have no rumble. Each
//! becomes a gamepad of its own in Android, with the USB IDs of
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
    // Windows.Gaming.Input is WinRT, used from this thread in the multithreaded apartment.
    // SAFETY: initialises COM for this thread, which ends with it.
    let _ = unsafe {
        windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED)
    };
    let mut pads = HostPads::default();
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

/// The first pad ID of the controllers read through Windows.Gaming.Input, past XInput's.
const RAW_FIRST_ID: u32 = 16;

/// How a controller that is not an Xbox controller lays out its buttons and axes, as
/// Windows.Gaming.Input's RawGameController reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawLayout {
    /// Sony's DualShock 4 and DualSense: Square, Cross, Circle, Triangle, L1, R1, L2, R2,
    /// Share/Create, Options, L3, R3, PS; axes left X and Y, right X, L2, R2, right Y.
    PlayStation,
    /// Nintendo's Switch Pro controller: B, A, Y, X, L, R, ZL, ZR, Minus, Plus, left and
    /// right stick, Home; axes left X and Y, right X and Y.
    SwitchPro,
    /// Anything else: the first buttons as A, B, X, Y, the shoulders, Back, Start and the
    /// sticks, and the axes as left X and Y, right X and Y.
    Generic,
}

/// The layout of a controller with these USB IDs.
pub fn raw_layout(vendor: u16, product: u16) -> RawLayout {
    match (vendor, product) {
        // DualShock 4 (both revisions, and the wireless adapter), DualSense, DualSense Edge.
        (0x054c, 0x05c4 | 0x09cc | 0x0ba0 | 0x0ce6 | 0x0df2) => RawLayout::PlayStation,
        // Switch Pro controller.
        (0x057e, 0x2009) => RawLayout::SwitchPro,
        _ => RawLayout::Generic,
    }
}

/// Which button each of a layout's buttons is, in the order the controller reports them.
fn raw_buttons(layout: RawLayout) -> &'static [u32] {
    match layout {
        RawLayout::PlayStation => &[
            buttons::X,
            buttons::A,
            buttons::B,
            buttons::Y,
            buttons::LEFT_SHOULDER,
            buttons::RIGHT_SHOULDER,
            0,
            0,
            buttons::BACK,
            buttons::START,
            buttons::LEFT_THUMB,
            buttons::RIGHT_THUMB,
            buttons::GUIDE,
        ],
        // Nintendo's A is on the right, where Android's B is, so positions are kept.
        RawLayout::SwitchPro => &[
            buttons::A,
            buttons::B,
            buttons::X,
            buttons::Y,
            buttons::LEFT_SHOULDER,
            buttons::RIGHT_SHOULDER,
            0,
            0,
            buttons::BACK,
            buttons::START,
            buttons::LEFT_THUMB,
            buttons::RIGHT_THUMB,
            buttons::GUIDE,
        ],
        RawLayout::Generic => &[
            buttons::A,
            buttons::B,
            buttons::X,
            buttons::Y,
            buttons::LEFT_SHOULDER,
            buttons::RIGHT_SHOULDER,
            buttons::BACK,
            buttons::START,
            buttons::LEFT_THUMB,
            buttons::RIGHT_THUMB,
            buttons::GUIDE,
        ],
    }
}

/// A RawGameController axis (0.0 to 1.0, 0.5 at rest) as a stick value.
pub fn raw_stick(value: f64) -> i16 {
    ((value.clamp(0.0, 1.0) * 65535.0).round() as i32 - 32768) as i16
}

/// A RawGameController axis as a trigger value.
pub fn raw_trigger(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The directional pad bits for a GameControllerSwitchPosition (0 at rest, 1 up, clockwise
/// to 8, up left).
pub fn raw_dpad(position: i32) -> u32 {
    let (up, right, down, left) = (
        buttons::DPAD_UP,
        buttons::DPAD_RIGHT,
        buttons::DPAD_DOWN,
        buttons::DPAD_LEFT,
    );
    match position {
        1 => up,
        2 => up | right,
        3 => right,
        4 => down | right,
        5 => down,
        6 => down | left,
        7 => left,
        8 => up | left,
        _ => 0,
    }
}

/// The state of a raw controller from what it reported: its buttons, its first switch (the
/// directional pad) and its axes.
pub fn raw_state(
    pad_id: u32,
    layout: RawLayout,
    pressed: &[bool],
    dpad: i32,
    axes: &[f64],
) -> GamepadState {
    let mut held = raw_dpad(dpad);
    for (index, bit) in raw_buttons(layout).iter().enumerate() {
        if pressed.get(index).copied().unwrap_or(false) {
            held |= bit;
        }
    }
    let axis = |index: usize| axes.get(index).copied().unwrap_or(0.5);
    let trigger_button = |index: usize| pressed.get(index).copied().unwrap_or(false);
    let (right_x, right_y, left_trigger, right_trigger) = match layout {
        RawLayout::PlayStation => (
            raw_stick(axis(2)),
            raw_stick(axis(5)),
            raw_trigger(axes.get(3).copied().unwrap_or(0.0)),
            raw_trigger(axes.get(4).copied().unwrap_or(0.0)),
        ),
        // Its triggers are buttons.
        RawLayout::SwitchPro => (
            raw_stick(axis(2)),
            raw_stick(axis(3)),
            if trigger_button(6) { 255 } else { 0 },
            if trigger_button(7) { 255 } else { 0 },
        ),
        RawLayout::Generic => (
            raw_stick(axis(2)),
            raw_stick(axis(3)),
            raw_trigger(axes.get(4).copied().unwrap_or(0.0)),
            raw_trigger(axes.get(5).copied().unwrap_or(0.0)),
        ),
    };
    GamepadState {
        pad_id,
        buttons: held,
        left_x: raw_stick(axis(0)),
        left_y: raw_stick(axis(1)),
        right_x,
        right_y,
        left_trigger,
        right_trigger,
    }
}

/// The controllers Windows.Gaming.Input sees that are not Xbox controllers (those come through
/// XInput, with their rumble): each with its layout from its USB IDs.
#[derive(Default)]
pub struct RawPads {
    /// The pad ID given to each controller, by its NonRoamableId.
    ids: std::collections::HashMap<String, u32>,
    warned: bool,
}

impl RawPads {
    fn read(&mut self) -> Vec<(GamepadInfo, GamepadState)> {
        match self.try_read() {
            Ok(pads) => pads,
            Err(why) => {
                if !self.warned {
                    eprintln!("gamepads: Windows.Gaming.Input cannot be read: {why}");
                    self.warned = true;
                }
                Vec::new()
            }
        }
    }

    fn try_read(&mut self) -> windows::core::Result<Vec<(GamepadInfo, GamepadState)>> {
        use windows::core::Interface;
        use windows::Gaming::Input::{
            Gamepad, GameControllerSwitchPosition, IGameController, RawGameController,
        };
        let mut pads = Vec::new();
        let controllers = RawGameController::RawGameControllers()?;
        for raw in controllers {
            // An Xbox controller is XInput's to read.
            if Gamepad::FromGameController(&raw.cast::<IGameController>()?).is_ok() {
                continue;
            }
            let key = raw.NonRoamableId()?.to_string();
            let next = RAW_FIRST_ID + self.ids.len() as u32;
            let pad_id = *self.ids.entry(key).or_insert(next);
            let (vendor, product) = (raw.HardwareVendorId()?, raw.HardwareProductId()?);
            let layout = raw_layout(vendor, product);
            let mut pressed = vec![false; raw.ButtonCount()?.max(0) as usize];
            let mut switches =
                vec![GameControllerSwitchPosition::Center; raw.SwitchCount()?.max(0) as usize];
            let mut axes = vec![0.5f64; raw.AxisCount()?.max(0) as usize];
            raw.GetCurrentReading(&mut pressed, &mut switches, &mut axes)?;
            let dpad = switches.first().map(|position| position.0).unwrap_or(0);
            let name = raw.DisplayName().map(|name| name.to_string()).unwrap_or_default();
            pads.push((
                GamepadInfo {
                    pad_id,
                    name: if name.is_empty() {
                        format!("Gamepad {pad_id}")
                    } else {
                        name
                    },
                    vendor_id: vendor,
                    product_id: product,
                    rumble: false,
                },
                raw_state(pad_id, layout, &pressed, dpad, &axes),
            ));
        }
        Ok(pads)
    }
}

/// Every controller the user has: the Xbox ones through XInput, the rest through
/// Windows.Gaming.Input.
#[derive(Default)]
pub struct HostPads {
    xinput: XInputPads,
    raw: RawPads,
}

impl GamepadSource for HostPads {
    fn read(&mut self) -> Vec<(GamepadInfo, GamepadState)> {
        let mut pads = self.xinput.read();
        pads.extend(self.raw.read());
        pads
    }

    fn rumble(&mut self, pad_id: u32, strong: u16, weak: u16) {
        self.xinput.rumble(pad_id, strong, weak);
    }
}

#[cfg(test)]
mod raw_tests {
    use super::*;

    #[test]
    fn fr30_a_dualsense_and_a_switch_pro_controller_get_their_own_layouts() {
        assert_eq!(raw_layout(0x054c, 0x0ce6), RawLayout::PlayStation);
        assert_eq!(raw_layout(0x054c, 0x05c4), RawLayout::PlayStation);
        assert_eq!(raw_layout(0x057e, 0x2009), RawLayout::SwitchPro);
        assert_eq!(raw_layout(0x1234, 0x5678), RawLayout::Generic);
    }

    #[test]
    fn fr30_a_playstation_controllers_cross_is_android_a_and_its_axes_land_where_they_belong() {
        let mut pressed = vec![false; 14];
        pressed[1] = true; // Cross
        pressed[3] = true; // Triangle
        let axes = [0.0, 1.0, 0.5, 1.0, 0.0, 0.5];
        let state = raw_state(16, RawLayout::PlayStation, &pressed, 3, &axes);
        assert_eq!(state.buttons, buttons::A | buttons::Y | buttons::DPAD_RIGHT);
        assert_eq!(state.left_x, -32768);
        assert_eq!(state.left_y, 32767);
        assert_eq!(state.left_trigger, 255);
        assert_eq!(state.right_trigger, 0);
    }

    #[test]
    fn fr30_a_switch_pro_controllers_triggers_are_buttons_and_an_unknown_pad_reads_generically() {
        let mut pressed = vec![false; 14];
        pressed[7] = true; // ZR
        let state = raw_state(17, RawLayout::SwitchPro, &pressed, 0, &[0.5; 4]);
        assert_eq!((state.left_trigger, state.right_trigger), (0, 255));
        assert_eq!(state.buttons, 0);
        let mut pressed = vec![false; 4];
        pressed[0] = true;
        let state = raw_state(18, RawLayout::Generic, &pressed, 8, &[]);
        assert_eq!(state.buttons, buttons::A | buttons::DPAD_UP | buttons::DPAD_LEFT);
        assert_eq!(state.left_x, 0);
    }
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
