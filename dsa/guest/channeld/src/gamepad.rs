//! The host's game controllers, as gamepad input devices of the guest.
//!
//! The host says when a controller is connected (`GamepadConnected`), what its buttons, sticks
//! and triggers read (`GamepadState`) and when it is gone (`GamepadDisconnected`). The daemon
//! makes each controller a uinput device that reports what an Xbox controller's driver (xpad)
//! does, so Android finds a gamepad with the key layout it has for one, and every app reads
//! it as it would a USB or Bluetooth gamepad. A rumble effect an app plays on the device goes
//! back to the host (`GamepadRumble`), which runs the controller's motors.
//!
//! [`events_for`] turns a state into the input events that tell the device, and
//! [`GamepadDevices`] keeps a device per controller. Neither knows about uinput: the platform
//! makes the devices ([`PadMaker`]), so the rules run and are tested on any platform.
//! [`uinput`] is the Linux one.

use std::collections::BTreeMap;
use std::sync::mpsc::Sender;
use std::sync::Mutex;

use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::{GamepadConnected, GamepadRumble, GamepadState};

/// Linux input event types.
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;
pub const EV_FF: u16 = 0x15;
pub const SYN_REPORT: u16 = 0;

/// The buttons an Xbox controller's driver reports.
pub const BTN_A: u16 = 0x130;
pub const BTN_B: u16 = 0x131;
pub const BTN_X: u16 = 0x133;
pub const BTN_Y: u16 = 0x134;
pub const BTN_TL: u16 = 0x136;
pub const BTN_TR: u16 = 0x137;
pub const BTN_SELECT: u16 = 0x13a;
pub const BTN_START: u16 = 0x13b;
pub const BTN_MODE: u16 = 0x13c;
pub const BTN_THUMBL: u16 = 0x13d;
pub const BTN_THUMBR: u16 = 0x13e;

/// The axes it reports: the sticks, the triggers and the directional pad.
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_Z: u16 = 0x02;
pub const ABS_RX: u16 = 0x03;
pub const ABS_RY: u16 = 0x04;
pub const ABS_RZ: u16 = 0x05;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

/// The rumble force feedback effect.
pub const FF_RUMBLE: u16 = 0x50;

/// The control protocol's button bits, and the key each becomes.
pub const BUTTONS: [(u32, u16); 11] = [
    (1, BTN_A),
    (2, BTN_B),
    (4, BTN_X),
    (8, BTN_Y),
    (16, BTN_TL),
    (32, BTN_TR),
    (64, BTN_SELECT),
    (128, BTN_START),
    (256, BTN_MODE),
    (512, BTN_THUMBL),
    (1024, BTN_THUMBR),
];

/// The control protocol's directional pad bits.
const DPAD_UP: u32 = 2048;
const DPAD_DOWN: u32 = 4096;
const DPAD_LEFT: u32 = 8192;
const DPAD_RIGHT: u32 = 16384;

/// Each axis, with its range: the sticks from -32768 to 32767, the triggers from 0 to 255,
/// the directional pad from -1 to 1.
pub const AXES: [(u16, i32, i32); 8] = [
    (ABS_X, -32768, 32767),
    (ABS_Y, -32768, 32767),
    (ABS_RX, -32768, 32767),
    (ABS_RY, -32768, 32767),
    (ABS_Z, 0, 255),
    (ABS_RZ, 0, 255),
    (ABS_HAT0X, -1, 1),
    (ABS_HAT0Y, -1, 1),
];

/// One input event for a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

impl InputEvent {
    pub const fn new(kind: u16, code: u16, value: i32) -> InputEvent {
        InputEvent { kind, code, value }
    }
}

/// The value of each axis in `state`.
fn axes_of(state: &GamepadState) -> [(u16, i32); 8] {
    let clamp = |value: i32| value.clamp(-32768, 32767);
    let trigger = |value: u32| value.min(255) as i32;
    let pad = |plus: u32, minus: u32| {
        i32::from(state.buttons & plus != 0) - i32::from(state.buttons & minus != 0)
    };
    [
        (ABS_X, clamp(state.left_x)),
        (ABS_Y, clamp(state.left_y)),
        (ABS_RX, clamp(state.right_x)),
        (ABS_RY, clamp(state.right_y)),
        (ABS_Z, trigger(state.left_trigger)),
        (ABS_RZ, trigger(state.right_trigger)),
        (ABS_HAT0X, pad(DPAD_RIGHT, DPAD_LEFT)),
        (ABS_HAT0Y, pad(DPAD_DOWN, DPAD_UP)),
    ]
}

/// The events that take a device from `previous` (`None` for a device that has reported
/// nothing yet) to `state`: a key event for each button that changed, an axis event for each
/// axis that changed, then the report that ends them. Empty when nothing changed.
pub fn events_for(previous: Option<&GamepadState>, state: &GamepadState) -> Vec<InputEvent> {
    let before = previous.cloned().unwrap_or_default();
    let mut events = Vec::new();
    for (bit, code) in BUTTONS {
        let held = state.buttons & bit != 0;
        if previous.is_none() || held != (before.buttons & bit != 0) {
            events.push(InputEvent::new(EV_KEY, code, i32::from(held)));
        }
    }
    let old = axes_of(&before);
    for (index, (code, value)) in axes_of(state).into_iter().enumerate() {
        if previous.is_none() || value != old[index].1 {
            events.push(InputEvent::new(EV_ABS, code, value));
        }
    }
    if !events.is_empty() {
        events.push(InputEvent::new(EV_SYN, SYN_REPORT, 0));
    }
    events
}

/// One controller's device in the guest.
pub trait VirtualPad: Send {
    /// Reports `events` on the device.
    fn write(&mut self, events: &[InputEvent]) -> Result<(), String>;
}

/// Makes the guest's devices. `rumble` is where a device sends the rumble effects apps play
/// on it, as frames for the host.
pub trait PadMaker: Send + Sync {
    fn make(
        &self,
        pad: &GamepadConnected,
        rumble: Sender<Vec<u8>>,
    ) -> Result<Box<dyn VirtualPad>, String>;
}

/// The frame that tells the host to run a controller's motors.
pub fn rumble_frame(pad_id: u32, strong: u16, weak: u16, duration_ms: u32) -> Vec<u8> {
    dsa_protocol::encode(&dsa_protocol::control::Envelope {
        body: Some(Body::GamepadRumble(GamepadRumble {
            pad_id,
            strong: u32::from(strong),
            weak: u32::from(weak),
            duration_ms,
        })),
    })
}

/// The most controllers the guest has devices for at once.
pub const MAX_PADS: usize = 8;

struct Pad {
    device: Box<dyn VirtualPad>,
    last: Option<GamepadState>,
}

/// A device for each of the host's controllers.
pub struct GamepadDevices {
    maker: Box<dyn PadMaker>,
    to_host: Sender<Vec<u8>>,
    pads: Mutex<BTreeMap<u32, Pad>>,
}

impl GamepadDevices {
    pub fn new(maker: Box<dyn PadMaker>, to_host: Sender<Vec<u8>>) -> GamepadDevices {
        GamepadDevices {
            maker,
            to_host,
            pads: Mutex::new(BTreeMap::new()),
        }
    }

    /// Acts on the host's controller message `body`, and says whether it was one. A device
    /// that cannot be made or written to is logged; the daemon goes on.
    pub fn handle(&self, body: &Body) -> bool {
        let mut pads = self.pads.lock().unwrap_or_else(|e| e.into_inner());
        match body {
            Body::GamepadConnected(pad) => {
                if pads.contains_key(&pad.pad_id) || pads.len() >= MAX_PADS {
                    return true;
                }
                match self.maker.make(pad, self.to_host.clone()) {
                    Ok(device) => {
                        eprintln!("dsa-channeld: gamepad {} ({}) is here", pad.pad_id, pad.name);
                        pads.insert(pad.pad_id, Pad { device, last: None });
                    }
                    Err(why) => eprintln!(
                        "dsa-channeld: could not make a gamepad for {}: {why}",
                        pad.name
                    ),
                }
                true
            }
            Body::GamepadDisconnected(gone) => {
                if pads.remove(&gone.pad_id).is_some() {
                    eprintln!("dsa-channeld: gamepad {} is gone", gone.pad_id);
                }
                true
            }
            Body::GamepadState(state) => {
                let Some(pad) = pads.get_mut(&state.pad_id) else {
                    return true;
                };
                let events = events_for(pad.last.as_ref(), state);
                if events.is_empty() {
                    return true;
                }
                match pad.device.write(&events) {
                    Ok(()) => pad.last = Some(state.clone()),
                    Err(why) => eprintln!(
                        "dsa-channeld: gamepad {} did not take its state: {why}",
                        state.pad_id
                    ),
                }
                true
            }
            _ => false,
        }
    }

    /// Forgets every device, as when the host connects again and gives its controllers anew.
    pub fn reset(&self) {
        self.pads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// The controllers that have a device.
    pub fn pads(&self) -> Vec<u32> {
        self.pads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .copied()
            .collect()
    }
}

/// Gamepad devices through Linux's uinput.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod uinput {
    use std::collections::HashMap;
    use std::ffi::c_ulong;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::mpsc::Sender;
    use std::sync::Arc;

    use dsa_protocol::control::GamepadConnected;

    use super::{
        rumble_frame, InputEvent, PadMaker, VirtualPad, AXES, BUTTONS, EV_ABS, EV_FF, EV_KEY,
        EV_SYN, FF_RUMBLE,
    };

    /// Where uinput is.
    pub const DEVICE: &str = "/dev/uinput";

    const UI_DEV_CREATE: c_ulong = 0x5501;
    const UI_DEV_DESTROY: c_ulong = 0x5502;
    const UI_DEV_SETUP: c_ulong = 0x405c_5503;
    const UI_ABS_SETUP: c_ulong = 0x401c_5504;
    const UI_SET_EVBIT: c_ulong = 0x4004_5564;
    const UI_SET_KEYBIT: c_ulong = 0x4004_5565;
    const UI_SET_ABSBIT: c_ulong = 0x4004_5567;
    const UI_SET_FFBIT: c_ulong = 0x4004_556b;
    const UI_BEGIN_FF_UPLOAD: c_ulong = 0xc068_55c8;
    const UI_END_FF_UPLOAD: c_ulong = 0x4068_55c9;
    const UI_BEGIN_FF_ERASE: c_ulong = 0xc00c_55ca;
    const UI_END_FF_ERASE: c_ulong = 0x400c_55cb;

    /// uinput's own events, which ask the daemon to take or drop an effect.
    const EV_UINPUT: u16 = 0x0101;
    const UI_FF_UPLOAD: u16 = 1;
    const UI_FF_ERASE: u16 = 2;

    /// The USB bus, which the device says it is on.
    const BUS_USB: u16 = 0x03;

    /// The most rumble effects a device holds at once.
    const FF_EFFECTS_MAX: u32 = 16;

    /// `struct input_event` on a 64-bit kernel.
    const EVENT_SIZE: usize = 24;

    #[repr(C)]
    struct InputId {
        bustype: u16,
        vendor: u16,
        product: u16,
        version: u16,
    }

    #[repr(C)]
    struct UinputSetup {
        id: InputId,
        name: [u8; 80],
        ff_effects_max: u32,
    }

    #[repr(C)]
    struct InputAbsinfo {
        value: i32,
        minimum: i32,
        maximum: i32,
        fuzz: i32,
        flat: i32,
        resolution: i32,
    }

    #[repr(C)]
    struct UinputAbsSetup {
        code: u16,
        absinfo: InputAbsinfo,
    }

    /// `struct ff_effect` (48 bytes on a 64-bit kernel). Only the parts a rumble uses are
    /// named; the rest is kept as it came.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FfEffect {
        kind: u16,
        id: i16,
        direction: u16,
        trigger: [u16; 2],
        /// `replay.length` and `replay.delay`, in milliseconds.
        replay: [u16; 2],
        _pad: u16,
        /// The union; for a rumble, its strong and weak magnitudes first.
        union: [u64; 4],
    }

    #[repr(C)]
    struct UinputFfUpload {
        request_id: u32,
        retval: i32,
        effect: FfEffect,
        old: FfEffect,
    }

    #[repr(C)]
    struct UinputFfErase {
        request_id: u32,
        retval: i32,
        effect_id: u32,
    }

    const _: () = assert!(std::mem::size_of::<UinputSetup>() == 92);
    const _: () = assert!(std::mem::size_of::<UinputAbsSetup>() == 28);
    const _: () = assert!(std::mem::size_of::<FfEffect>() == 48);
    const _: () = assert!(std::mem::size_of::<UinputFfUpload>() == 104);
    const _: () = assert!(std::mem::size_of::<UinputFfErase>() == 12);

    fn ioctl<T>(file: &File, request: c_ulong, argument: *mut T) -> Result<(), String> {
        // SAFETY: a uinput request with the structure its number names, which outlives the
        // call.
        let result = unsafe { libc::ioctl(file.as_raw_fd(), request as _, argument) };
        if result < 0 {
            return Err(format!(
                "uinput refused request {request:#x}: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    fn set_bit(file: &File, request: c_ulong, bit: u16) -> Result<(), String> {
        // SAFETY: these requests take the bit as the argument itself.
        let result = unsafe { libc::ioctl(file.as_raw_fd(), request as _, libc::c_int::from(bit)) };
        if result < 0 {
            return Err(format!(
                "uinput refused bit {bit:#x}: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    /// The bytes of one `struct input_event`, its time left to the kernel.
    fn event_bytes(event: &InputEvent) -> [u8; EVENT_SIZE] {
        let mut bytes = [0u8; EVENT_SIZE];
        bytes[16..18].copy_from_slice(&event.kind.to_ne_bytes());
        bytes[18..20].copy_from_slice(&event.code.to_ne_bytes());
        bytes[20..24].copy_from_slice(&event.value.to_ne_bytes());
        bytes
    }

    /// Makes devices on `/dev/uinput`.
    pub struct Uinput;

    impl PadMaker for Uinput {
        fn make(
            &self,
            pad: &GamepadConnected,
            rumble: Sender<Vec<u8>>,
        ) -> Result<Box<dyn VirtualPad>, String> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_CLOEXEC)
                .open(DEVICE)
                .map_err(|e| format!("opening {DEVICE}: {e}"))?;
            set_bit(&file, UI_SET_EVBIT, EV_KEY)?;
            set_bit(&file, UI_SET_EVBIT, EV_ABS)?;
            set_bit(&file, UI_SET_EVBIT, EV_SYN)?;
            for (_, code) in BUTTONS {
                set_bit(&file, UI_SET_KEYBIT, code)?;
            }
            for (code, _, _) in AXES {
                set_bit(&file, UI_SET_ABSBIT, code)?;
            }
            if pad.rumble {
                set_bit(&file, UI_SET_EVBIT, EV_FF)?;
                set_bit(&file, UI_SET_FFBIT, FF_RUMBLE)?;
            }
            let mut name = [0u8; 80];
            let bytes = pad.name.as_bytes();
            let length = bytes.len().min(name.len() - 1);
            name[..length].copy_from_slice(&bytes[..length]);
            let mut setup = UinputSetup {
                id: InputId {
                    bustype: BUS_USB,
                    vendor: pad.vendor_id.min(0xffff) as u16,
                    product: pad.product_id.min(0xffff) as u16,
                    version: 1,
                },
                name,
                ff_effects_max: if pad.rumble { FF_EFFECTS_MAX } else { 0 },
            };
            ioctl(&file, UI_DEV_SETUP, &mut setup)?;
            for (code, minimum, maximum) in AXES {
                // The sticks rest near the middle without being exactly there.
                let stick = maximum > 255;
                let mut axis = UinputAbsSetup {
                    code,
                    absinfo: InputAbsinfo {
                        value: 0,
                        minimum,
                        maximum,
                        fuzz: if stick { 16 } else { 0 },
                        flat: if stick { 128 } else { 0 },
                        resolution: 0,
                    },
                };
                ioctl(&file, UI_ABS_SETUP, &mut axis)?;
            }
            ioctl(&file, UI_DEV_CREATE, std::ptr::null_mut::<u8>())?;
            let file = Arc::new(file);
            if pad.rumble {
                let (reader, pad_id) = (Arc::clone(&file), pad.pad_id);
                std::thread::Builder::new()
                    .name(format!("gamepad-{pad_id}"))
                    .spawn(move || serve_effects(&reader, pad_id, &rumble))
                    .map_err(|e| format!("starting the rumble thread: {e}"))?;
            }
            Ok(Box::new(UinputPad { file }))
        }
    }

    /// Takes the effects apps upload to the device, and passes each one they play to the host,
    /// until the device is destroyed.
    fn serve_effects(file: &Arc<File>, pad_id: u32, rumble: &Sender<Vec<u8>>) {
        let mut effects: HashMap<i16, (u16, u16, u16)> = HashMap::new();
        let mut buffer = [0u8; EVENT_SIZE];
        let mut reader: &File = file;
        loop {
            match reader.read(&mut buffer) {
                Ok(EVENT_SIZE) => {}
                // The device is gone.
                Ok(_) | Err(_) => return,
            }
            let kind = u16::from_ne_bytes([buffer[16], buffer[17]]);
            let code = u16::from_ne_bytes([buffer[18], buffer[19]]);
            let value = i32::from_ne_bytes([buffer[20], buffer[21], buffer[22], buffer[23]]);
            match (kind, code) {
                (EV_UINPUT, UI_FF_UPLOAD) => {
                    // SAFETY: plain integers, filled in by the kernel.
                    let mut upload: UinputFfUpload = unsafe { std::mem::zeroed() };
                    upload.request_id = value as u32;
                    if ioctl(file, UI_BEGIN_FF_UPLOAD, &mut upload).is_err() {
                        continue;
                    }
                    let effect = upload.effect;
                    let magnitudes = effect.union[0];
                    let strong = (magnitudes & 0xffff) as u16;
                    let weak = ((magnitudes >> 16) & 0xffff) as u16;
                    effects.insert(effect.id, (strong, weak, effect.replay[0]));
                    upload.retval = 0;
                    let _ = ioctl(file, UI_END_FF_UPLOAD, &mut upload);
                }
                (EV_UINPUT, UI_FF_ERASE) => {
                    let mut erase = UinputFfErase {
                        request_id: value as u32,
                        retval: 0,
                        effect_id: 0,
                    };
                    if ioctl(file, UI_BEGIN_FF_ERASE, &mut erase).is_err() {
                        continue;
                    }
                    effects.remove(&(erase.effect_id as i16));
                    erase.retval = 0;
                    let _ = ioctl(file, UI_END_FF_ERASE, &mut erase);
                }
                (EV_FF, id) => {
                    let (strong, weak, length) = match effects.get(&(id as i16)) {
                        Some(effect) if value > 0 => *effect,
                        _ => (0, 0, 0),
                    };
                    let _ = rumble.send(rumble_frame(pad_id, strong, weak, u32::from(length)));
                }
                _ => {}
            }
        }
    }

    /// A device on uinput, destroyed when dropped.
    struct UinputPad {
        file: Arc<File>,
    }

    impl VirtualPad for UinputPad {
        fn write(&mut self, events: &[InputEvent]) -> Result<(), String> {
            let mut bytes = Vec::with_capacity(events.len() * EVENT_SIZE);
            for event in events {
                bytes.extend_from_slice(&event_bytes(event));
            }
            let mut writer: &File = &self.file;
            writer
                .write_all(&bytes)
                .map_err(|e| format!("writing to {DEVICE}: {e}"))
        }
    }

    impl Drop for UinputPad {
        fn drop(&mut self) {
            // The rumble thread's read ends with the device.
            let _ = ioctl(&self.file, UI_DEV_DESTROY, std::ptr::null_mut::<u8>());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex as StdMutex};

    #[derive(Default, Clone)]
    struct Written(Arc<StdMutex<Vec<(u32, Vec<InputEvent>)>>>);

    struct FakePad {
        pad_id: u32,
        written: Written,
    }

    impl VirtualPad for FakePad {
        fn write(&mut self, events: &[InputEvent]) -> Result<(), String> {
            self.written
                .0
                .lock()
                .unwrap()
                .push((self.pad_id, events.to_vec()));
            Ok(())
        }
    }

    struct FakeMaker(Written);

    impl PadMaker for FakeMaker {
        fn make(
            &self,
            pad: &GamepadConnected,
            _: Sender<Vec<u8>>,
        ) -> Result<Box<dyn VirtualPad>, String> {
            Ok(Box::new(FakePad {
                pad_id: pad.pad_id,
                written: self.0.clone(),
            }))
        }
    }

    fn connected(pad_id: u32) -> Body {
        Body::GamepadConnected(GamepadConnected {
            pad_id,
            name: "Xbox Controller".into(),
            vendor_id: 0x045e,
            product_id: 0x028e,
            rumble: true,
        })
    }

    #[test]
    fn fr30_a_fake_controllers_state_comes_out_as_gamepad_events_with_the_same_values() {
        let state = GamepadState {
            pad_id: 1,
            buttons: 1 | 32 | DPAD_LEFT | DPAD_DOWN,
            left_x: -32768,
            left_y: 32767,
            right_x: 1200,
            right_y: -5,
            left_trigger: 255,
            right_trigger: 7,
        };
        let events = events_for(None, &state);
        let value = |kind: u16, code: u16| {
            events
                .iter()
                .find(|event| event.kind == kind && event.code == code)
                .map(|event| event.value)
        };
        assert_eq!(value(EV_KEY, BTN_A), Some(1));
        assert_eq!(value(EV_KEY, BTN_B), Some(0));
        assert_eq!(value(EV_KEY, BTN_TR), Some(1));
        assert_eq!(value(EV_ABS, ABS_X), Some(-32768));
        assert_eq!(value(EV_ABS, ABS_Y), Some(32767));
        assert_eq!(value(EV_ABS, ABS_RX), Some(1200));
        assert_eq!(value(EV_ABS, ABS_RY), Some(-5));
        assert_eq!(value(EV_ABS, ABS_Z), Some(255));
        assert_eq!(value(EV_ABS, ABS_RZ), Some(7));
        assert_eq!(value(EV_ABS, ABS_HAT0X), Some(-1));
        assert_eq!(value(EV_ABS, ABS_HAT0Y), Some(1));
        assert_eq!(
            events.last(),
            Some(&InputEvent::new(EV_SYN, SYN_REPORT, 0))
        );
    }

    #[test]
    fn fr30_only_what_changed_is_reported_and_nothing_when_nothing_did() {
        let before = GamepadState {
            pad_id: 1,
            buttons: 1,
            ..GamepadState::default()
        };
        let after = GamepadState {
            buttons: 2,
            right_trigger: 100,
            ..before.clone()
        };
        assert_eq!(
            events_for(Some(&before), &after),
            vec![
                InputEvent::new(EV_KEY, BTN_A, 0),
                InputEvent::new(EV_KEY, BTN_B, 1),
                InputEvent::new(EV_ABS, ABS_RZ, 100),
                InputEvent::new(EV_SYN, SYN_REPORT, 0),
            ]
        );
        assert!(events_for(Some(&after), &after).is_empty());
    }

    #[test]
    fn fr30_each_controller_gets_a_device_until_it_is_disconnected() {
        let written = Written::default();
        let (to_host, _) = mpsc::channel();
        let devices = GamepadDevices::new(Box::new(FakeMaker(written.clone())), to_host);
        assert!(devices.handle(&connected(1)));
        assert!(devices.handle(&connected(1)));
        assert!(devices.handle(&connected(2)));
        assert_eq!(devices.pads(), vec![1, 2]);
        let press = Body::GamepadState(GamepadState {
            pad_id: 2,
            buttons: 8,
            ..GamepadState::default()
        });
        assert!(devices.handle(&press));
        assert!(devices.handle(&press));
        let writes = written.0.lock().unwrap().clone();
        assert_eq!(writes.len(), 1, "the same state again writes nothing");
        assert_eq!(writes[0].0, 2);
        assert!(writes[0].1.contains(&InputEvent::new(EV_KEY, BTN_Y, 1)));
        assert!(devices.handle(&Body::GamepadDisconnected(
            dsa_protocol::control::GamepadDisconnected { pad_id: 1 }
        )));
        assert_eq!(devices.pads(), vec![2]);
        assert!(!devices.handle(&Body::ScreenState(
            dsa_protocol::control::ScreenState { on: true }
        )));
        devices.reset();
        assert!(devices.pads().is_empty());
    }

    #[test]
    fn fr30_a_rumble_goes_to_the_host_as_its_strengths() {
        let frame = rumble_frame(3, 40000, 2000, 120);
        let body = dsa_protocol::decode(&frame).unwrap().body;
        assert_eq!(
            body,
            Some(Body::GamepadRumble(GamepadRumble {
                pad_id: 3,
                strong: 40000,
                weak: 2000,
                duration_ms: 120,
            }))
        );
    }
}
