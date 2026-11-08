//! The keyboard layout the compositor hands over (`wl_keyboard.keymap`), read with
//! libxkbcommon, which every Wayland desktop has. It is loaded at run time so building the
//! host needs no development package.
//!
//! The layout says which character a key types, so a key that types one goes to the guest as
//! text and the guest types exactly what the key shows, in any layout; the compose table
//! (from the user's locale) turns dead keys and compose sequences into their characters.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::os::fd::{AsRawFd, OwnedFd};

use libloading::Library;

use super::keymap::{XKB_LOCK_MASK, XKB_MOD2_MASK};

type Ctx = c_void;
type Keymap = c_void;
type State = c_void;
type ComposeTable = c_void;
type ComposeState = c_void;

const XKB_KEYMAP_FORMAT_TEXT_V1: u32 = 1;
const XKB_STATE_MODS_EFFECTIVE: c_int = 1 << 3;
const XKB_COMPOSE_FEED_ACCEPTED: c_int = 1;
const XKB_COMPOSE_COMPOSING: c_int = 1;
const XKB_COMPOSE_COMPOSED: c_int = 2;
const XKB_COMPOSE_CANCELLED: c_int = 3;
const XKB_MOD_INVALID: u32 = 0xffff_ffff;

struct Api {
    _library: Library,
    context_new: unsafe extern "C" fn(c_int) -> *mut Ctx,
    keymap_new_from_string: unsafe extern "C" fn(*mut Ctx, *const c_char, u32, u32) -> *mut Keymap,
    keymap_unref: unsafe extern "C" fn(*mut Keymap),
    keymap_mod_get_index: unsafe extern "C" fn(*mut Keymap, *const c_char) -> u32,
    state_new: unsafe extern "C" fn(*mut Keymap) -> *mut State,
    state_unref: unsafe extern "C" fn(*mut State),
    state_update_mask: unsafe extern "C" fn(*mut State, u32, u32, u32, u32, u32, u32) -> c_int,
    state_key_get_utf32: unsafe extern "C" fn(*mut State, u32) -> u32,
    state_key_get_one_sym: unsafe extern "C" fn(*mut State, u32) -> u32,
    state_mod_name_is_active: unsafe extern "C" fn(*mut State, *const c_char, c_int) -> c_int,
    compose_table_new_from_locale:
        Option<unsafe extern "C" fn(*mut Ctx, *const c_char, c_int) -> *mut ComposeTable>,
    compose_state_new: Option<unsafe extern "C" fn(*mut ComposeTable, c_int) -> *mut ComposeState>,
    compose_state_feed: Option<unsafe extern "C" fn(*mut ComposeState, u32) -> c_int>,
    compose_state_reset: Option<unsafe extern "C" fn(*mut ComposeState)>,
    compose_state_get_status: Option<unsafe extern "C" fn(*mut ComposeState) -> c_int>,
    compose_state_get_utf8:
        Option<unsafe extern "C" fn(*mut ComposeState, *mut c_char, usize) -> c_int>,
}

impl Api {
    fn load() -> Result<Api, String> {
        // SAFETY: libxkbcommon runs no code of its own when loaded.
        let library = unsafe { Library::new("libxkbcommon.so.0") }
            .map_err(|error| format!("cannot load libxkbcommon: {error}"))?;
        // SAFETY: each symbol is looked up with the type libxkbcommon's header gives it.
        unsafe {
            macro_rules! required {
                ($name:literal) => {
                    *library
                        .get(concat!($name, "\0").as_bytes())
                        .map_err(|error| format!("libxkbcommon has no {}: {error}", $name))?
                };
            }
            macro_rules! optional {
                ($name:literal) => {
                    library
                        .get(concat!($name, "\0").as_bytes())
                        .ok()
                        .map(|symbol| *symbol)
                };
            }
            Ok(Api {
                context_new: required!("xkb_context_new"),
                keymap_new_from_string: required!("xkb_keymap_new_from_string"),
                keymap_unref: required!("xkb_keymap_unref"),
                keymap_mod_get_index: required!("xkb_keymap_mod_get_index"),
                state_new: required!("xkb_state_new"),
                state_unref: required!("xkb_state_unref"),
                state_update_mask: required!("xkb_state_update_mask"),
                state_key_get_utf32: required!("xkb_state_key_get_utf32"),
                state_key_get_one_sym: required!("xkb_state_key_get_one_sym"),
                state_mod_name_is_active: required!("xkb_state_mod_name_is_active"),
                compose_table_new_from_locale: optional!("xkb_compose_table_new_from_locale"),
                compose_state_new: optional!("xkb_compose_state_new"),
                compose_state_feed: optional!("xkb_compose_state_feed"),
                compose_state_reset: optional!("xkb_compose_state_reset"),
                compose_state_get_status: optional!("xkb_compose_state_get_status"),
                compose_state_get_utf8: optional!("xkb_compose_state_get_utf8"),
                _library: library,
            })
        }
    }
}

/// What a key press types, by the layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    /// Text: the key goes to the guest as this text, not as a key event.
    Text(String),
    /// A dead key or part of a compose sequence: the key types nothing yet, and goes nowhere.
    Composing,
    /// No character: the key goes as a key event.
    Nothing,
}

/// The keyboard's layout and state.
pub struct Keyboard {
    api: Api,
    context: *mut Ctx,
    keymap: *mut Keymap,
    state: *mut State,
    compose: *mut ComposeState,
    caps_mask: u32,
    num_mask: u32,
}

// SAFETY: the pointers are only used through `&mut self` on one thread at a time.
unsafe impl Send for Keyboard {}

impl Keyboard {
    pub fn new() -> Result<Keyboard, String> {
        let api = Api::load()?;
        // SAFETY: no flags.
        let context = unsafe { (api.context_new)(0) };
        if context.is_null() {
            return Err("libxkbcommon could not make a context".into());
        }
        let compose = match (api.compose_table_new_from_locale, api.compose_state_new) {
            (Some(table_new), Some(state_new)) => {
                let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
                    .iter()
                    .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
                    .unwrap_or_else(|| "C".into());
                let locale = CString::new(locale).unwrap_or_default();
                // SAFETY: a valid context and a NUL-terminated locale.
                let table = unsafe { table_new(context, locale.as_ptr(), 0) };
                if table.is_null() {
                    std::ptr::null_mut()
                } else {
                    // SAFETY: a valid table; the state keeps its own reference.
                    unsafe { state_new(table, 0) }
                }
            }
            _ => std::ptr::null_mut(),
        };
        Ok(Keyboard {
            api,
            context,
            keymap: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            compose,
            caps_mask: XKB_LOCK_MASK,
            num_mask: XKB_MOD2_MASK,
        })
    }

    /// `wl_keyboard.keymap`: the layout, as XKB text in `fd`, `size` bytes with its NUL.
    pub fn set_keymap(&mut self, fd: OwnedFd, size: u32) {
        let size = size as usize;
        // SAFETY: maps the compositor's file read-only, privately, for as long as it is read.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                fd.as_raw_fd(),
                0,
            )
        };
        if map == libc::MAP_FAILED || size == 0 {
            return;
        }
        // SAFETY: the map holds the keymap text, NUL-terminated within `size` bytes.
        let keymap = unsafe {
            let text =
                CStr::from_bytes_until_nul(std::slice::from_raw_parts(map.cast::<u8>(), size))
                    .map(CStr::as_ptr)
                    .unwrap_or(std::ptr::null());
            let keymap = if text.is_null() {
                std::ptr::null_mut()
            } else {
                (self.api.keymap_new_from_string)(self.context, text, XKB_KEYMAP_FORMAT_TEXT_V1, 0)
            };
            libc::munmap(map, size);
            keymap
        };
        if keymap.is_null() {
            return;
        }
        // SAFETY: a valid keymap; the old state and keymap are released once replaced.
        unsafe {
            let state = (self.api.state_new)(keymap);
            if state.is_null() {
                (self.api.keymap_unref)(keymap);
                return;
            }
            self.release();
            self.keymap = keymap;
            self.state = state;
            let index = |name: &CStr| (self.api.keymap_mod_get_index)(keymap, name.as_ptr());
            let caps = index(c"Lock");
            let num = index(c"Mod2");
            self.caps_mask = if caps == XKB_MOD_INVALID {
                0
            } else {
                1 << caps
            };
            self.num_mask = if num == XKB_MOD_INVALID { 0 } else { 1 << num };
        }
    }

    /// `wl_keyboard.modifiers`.
    pub fn modifiers(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        if self.state.is_null() {
            return;
        }
        // SAFETY: a valid state.
        unsafe {
            (self.api.state_update_mask)(self.state, depressed, latched, locked, 0, 0, group);
        }
    }

    /// Which modifier masks are Caps Lock and Num Lock in this layout.
    pub fn lock_masks(&self) -> (u32, u32) {
        (self.caps_mask, self.num_mask)
    }

    fn active(&self, name: &CStr) -> bool {
        // SAFETY: a valid state and a NUL-terminated name.
        !self.state.is_null()
            && unsafe {
                (self.api.state_mod_name_is_active)(
                    self.state,
                    name.as_ptr(),
                    XKB_STATE_MODS_EFFECTIVE,
                )
            } > 0
    }

    /// Whether Ctrl or Alt is held, which makes a key a shortcut rather than text.
    pub fn ctrl_or_alt(&self) -> bool {
        self.ctrl() || self.alt()
    }

    /// Whether Alt is held.
    pub fn alt(&self) -> bool {
        self.active(c"Mod1")
    }

    /// Whether Ctrl is held.
    pub fn ctrl(&self) -> bool {
        self.active(c"Control")
    }

    /// What pressing the key with evdev code `code` types.
    pub fn press(&mut self, code: u32) -> Typed {
        if self.state.is_null() {
            return Typed::Nothing;
        }
        let key = code + 8;
        // SAFETY: a valid state, and a compose state when it is not null.
        unsafe {
            if !self.compose.is_null() && !self.ctrl_or_alt() {
                if let (Some(feed), Some(status), Some(reset), Some(utf8)) = (
                    self.api.compose_state_feed,
                    self.api.compose_state_get_status,
                    self.api.compose_state_reset,
                    self.api.compose_state_get_utf8,
                ) {
                    let sym = (self.api.state_key_get_one_sym)(self.state, key);
                    if feed(self.compose, sym) == XKB_COMPOSE_FEED_ACCEPTED {
                        match status(self.compose) {
                            XKB_COMPOSE_COMPOSING => return Typed::Composing,
                            XKB_COMPOSE_COMPOSED => {
                                let mut buffer = [0 as c_char; 64];
                                let len = utf8(self.compose, buffer.as_mut_ptr(), buffer.len());
                                reset(self.compose);
                                let text = CStr::from_ptr(buffer.as_ptr()).to_string_lossy();
                                return if len > 0 {
                                    Typed::Text(text.into_owned())
                                } else {
                                    Typed::Composing
                                };
                            }
                            XKB_COMPOSE_CANCELLED => {
                                reset(self.compose);
                                return Typed::Composing;
                            }
                            _ => {}
                        }
                    }
                }
            }
            let unit = (self.api.state_key_get_utf32)(self.state, key);
            match char::from_u32(unit) {
                Some(character) if unit != 0 => {
                    if super::keymap::types_text(character, self.ctrl_or_alt()) {
                        Typed::Text(character.to_string())
                    } else {
                        Typed::Nothing
                    }
                }
                _ => Typed::Nothing,
            }
        }
    }

    fn release(&mut self) {
        // SAFETY: each pointer is released once, then forgotten.
        unsafe {
            if !self.state.is_null() {
                (self.api.state_unref)(self.state);
            }
            if !self.keymap.is_null() {
                (self.api.keymap_unref)(self.keymap);
            }
        }
        self.state = std::ptr::null_mut();
        self.keymap = std::ptr::null_mut();
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        self.release();
        // The context and the compose state live as long as the process; libxkbcommon frees
        // them with it.
    }
}
