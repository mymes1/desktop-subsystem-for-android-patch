//! The game controllers under `/dev/input`, read through the kernel's evdev interface.
//!
//! Each read asks every controller for the buttons held and each axis's value with the
//! state ioctls, so nothing queues up while no window of the user's Android is in front.
//! The devices are looked for again every few seconds, so a controller plugged in later is
//! found. A controller with a rumble motor plays the guest's effects through the kernel's
//! force feedback.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dsa_host_core::gamepads::{GamepadInfo, GamepadSource, GamepadState, MAX_NAME, MAX_PADS};

use super::gamepad::{codes, is_gamepad, state, AxisRange, RawPad};

const EV_KEY: u32 = 0x01;
const EV_ABS: u32 = 0x03;
const EV_FF: u32 = 0x15;
const FF_RUMBLE: u16 = 0x50;
const KEY_MAX: usize = 0x2ff;
const ABS_MAX: usize = 0x3f;

/// How often the devices are looked for again.
const RESCAN: Duration = Duration::from_secs(3);

const fn ioc(dir: u64, nr: u64, size: u64) -> u64 {
    (dir << 30) | (size << 16) | ((b'E' as u64) << 8) | nr
}
const READ: u64 = 2;
const WRITE: u64 = 1;

fn eviocgbit(event: u32, len: usize) -> u64 {
    ioc(READ, 0x20 + u64::from(event), len as u64)
}
fn eviocgkey(len: usize) -> u64 {
    ioc(READ, 0x18, len as u64)
}
fn eviocgname(len: usize) -> u64 {
    ioc(READ, 0x06, len as u64)
}
fn eviocgabs(axis: u16) -> u64 {
    ioc(
        READ,
        0x40 + u64::from(axis),
        std::mem::size_of::<AbsInfo>() as u64,
    )
}
const EVIOCGID: u64 = ioc(READ, 0x02, 8);
const EVIOCSFF: u64 = ioc(WRITE, 0x80, std::mem::size_of::<FfEffect>() as u64);

// The kernel fills every field; only some are read.
#[allow(dead_code)]
#[repr(C)]
#[derive(Default)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

/// `struct ff_effect` on a 64-bit kernel: the union starts at 16 and is 32 bytes long.
#[allow(dead_code)]
#[repr(C)]
struct FfEffect {
    kind: u16,
    id: i16,
    direction: u16,
    trigger: [u16; 2],
    replay: [u16; 2],
    padding: u16,
    union: [u64; 4],
}

#[allow(dead_code)]
#[repr(C)]
struct InputEvent {
    time: [i64; 2],
    kind: u16,
    code: u16,
    value: i32,
}

/// # Safety
/// `arg` must point at memory the request may read or write, as large as it says.
unsafe fn ioctl(file: &File, request: u64, arg: *mut libc::c_void) -> bool {
    libc::ioctl(file.as_raw_fd(), request as _, arg) >= 0
}

fn bit(bits: &[u8], index: usize) -> bool {
    bits.get(index / 8)
        .is_some_and(|byte| byte & (1 << (index % 8)) != 0)
}

fn bits(file: &File, request: u64, len: usize) -> Vec<u8> {
    let mut bits = vec![0u8; len];
    // SAFETY: the buffer is `len` bytes, as the request says.
    if unsafe { ioctl(file, request, bits.as_mut_ptr().cast()) } {
        bits
    } else {
        vec![0u8; len]
    }
}

struct Pad {
    path: PathBuf,
    file: File,
    info: GamepadInfo,
    axes: Vec<(u16, AxisRange)>,
    /// The force feedback effect uploaded for rumble, once one has been.
    effect: Option<i16>,
}

fn open(path: &Path) -> Option<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .or_else(|_| File::open(path))
        .ok()
}

fn probe(path: &Path, pad_id: u32) -> Option<Pad> {
    let file = open(path)?;
    let keys = bits(&file, eviocgbit(EV_KEY, KEY_MAX / 8 + 1), KEY_MAX / 8 + 1);
    let abs = bits(&file, eviocgbit(EV_ABS, ABS_MAX / 8 + 1), ABS_MAX / 8 + 1);
    if !is_gamepad(
        bit(&keys, usize::from(codes::BTN_SOUTH)),
        bit(&abs, usize::from(codes::ABS_X)),
    ) {
        return None;
    }
    let ff = bits(&file, eviocgbit(EV_FF, 16), 16);
    let mut name = vec![0u8; 256];
    // SAFETY: the buffer is 256 bytes, as the request says.
    let named = unsafe { ioctl(&file, eviocgname(name.len()), name.as_mut_ptr().cast()) };
    let name = if named {
        let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
        let mut name = String::from_utf8_lossy(&name[..end]).into_owned();
        while name.len() > MAX_NAME {
            name.pop();
        }
        name
    } else {
        String::new()
    };
    let mut id = [0u16; 4];
    // SAFETY: `struct input_id` is four u16, eight bytes.
    unsafe { ioctl(&file, EVIOCGID, id.as_mut_ptr().cast()) };
    let mut axes = Vec::new();
    for axis in 0..=ABS_MAX as u16 {
        if !bit(&abs, usize::from(axis)) {
            continue;
        }
        let mut info = AbsInfo::default();
        // SAFETY: `info` is a `struct input_absinfo`.
        if unsafe { ioctl(&file, eviocgabs(axis), (&mut info as *mut AbsInfo).cast()) } {
            axes.push((
                axis,
                AxisRange {
                    minimum: info.minimum,
                    maximum: info.maximum,
                },
            ));
        }
    }
    Some(Pad {
        path: path.to_path_buf(),
        file,
        info: GamepadInfo {
            pad_id,
            name,
            vendor_id: id[1],
            product_id: id[2],
            rumble: bit(&ff, usize::from(FF_RUMBLE)),
        },
        axes,
        effect: None,
    })
}

impl Pad {
    fn read(&self) -> Option<GamepadState> {
        let keys = {
            let mut bits = vec![0u8; KEY_MAX / 8 + 1];
            // SAFETY: the buffer is as long as the request says.
            if !unsafe { ioctl(&self.file, eviocgkey(bits.len()), bits.as_mut_ptr().cast()) } {
                return None;
            }
            bits
        };
        let mut raw = RawPad::default();
        for code in 0x100..=KEY_MAX as u16 {
            if bit(&keys, usize::from(code)) {
                raw.held.push(code);
            }
        }
        for (axis, range) in &self.axes {
            let mut info = AbsInfo::default();
            // SAFETY: `info` is a `struct input_absinfo`.
            if unsafe {
                ioctl(
                    &self.file,
                    eviocgabs(*axis),
                    (&mut info as *mut AbsInfo).cast(),
                )
            } {
                raw.axes.push((*axis, info.value, *range));
            }
        }
        Some(state(self.info.pad_id, &raw))
    }

    fn rumble(&mut self, strong: u16, weak: u16) {
        if !self.info.rumble {
            return;
        }
        let mut effect = FfEffect {
            kind: FF_RUMBLE,
            id: self.effect.unwrap_or(-1),
            direction: 0,
            trigger: [0; 2],
            replay: [0, 0],
            padding: 0,
            union: [u64::from(strong) | (u64::from(weak) << 16), 0, 0, 0],
        };
        // SAFETY: `effect` is a `struct ff_effect`; the kernel writes the effect's id back.
        if !unsafe { ioctl(&self.file, EVIOCSFF, (&mut effect as *mut FfEffect).cast()) } {
            return;
        }
        self.effect = Some(effect.id);
        let play = InputEvent {
            time: [0; 2],
            kind: EV_FF as u16,
            code: effect.id as u16,
            value: i32::from(strong != 0 || weak != 0),
        };
        // SAFETY: writes one whole `struct input_event`.
        unsafe {
            libc::write(
                self.file.as_raw_fd(),
                (&play as *const InputEvent).cast(),
                std::mem::size_of::<InputEvent>(),
            );
        }
    }
}

/// The controllers under `/dev/input`, for `dsa_host_core::gamepads::follow`.
pub struct EvdevGamepads {
    pads: BTreeMap<u32, Pad>,
    scanned: Option<Instant>,
}

impl EvdevGamepads {
    pub fn new() -> EvdevGamepads {
        EvdevGamepads {
            pads: BTreeMap::new(),
            scanned: None,
        }
    }

    fn rescan(&mut self) {
        let now = Instant::now();
        if self
            .scanned
            .is_some_and(|at| now.duration_since(at) < RESCAN)
        {
            return;
        }
        self.scanned = Some(now);
        let Ok(entries) = std::fs::read_dir("/dev/input") else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("event"))
            })
            .collect();
        paths.sort();
        self.pads.retain(|_, pad| pad.path.exists());
        for path in paths {
            if self.pads.len() >= MAX_PADS {
                break;
            }
            if self.pads.values().any(|pad| pad.path == path) {
                continue;
            }
            let pad_id = (0..MAX_PADS as u32)
                .find(|id| !self.pads.contains_key(id))
                .unwrap_or(0);
            if let Some(pad) = probe(&path, pad_id) {
                self.pads.insert(pad_id, pad);
            }
        }
    }
}

impl Default for EvdevGamepads {
    fn default() -> Self {
        EvdevGamepads::new()
    }
}

impl GamepadSource for EvdevGamepads {
    fn read(&mut self) -> Vec<(GamepadInfo, GamepadState)> {
        self.rescan();
        let mut gone = Vec::new();
        let mut read = Vec::new();
        for (pad_id, pad) in &self.pads {
            match pad.read() {
                Some(state) => read.push((pad.info.clone(), state)),
                None => gone.push(*pad_id),
            }
        }
        for pad_id in gone {
            self.pads.remove(&pad_id);
        }
        read
    }

    fn rumble(&mut self, pad_id: u32, strong: u16, weak: u16) {
        if let Some(pad) = self.pads.get_mut(&pad_id) {
            pad.rumble(strong, weak);
        }
    }
}
