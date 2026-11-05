//! The kernel's key codes (`linux/input-event-codes.h`) as the guest's key codes.
//!
//! `wl_keyboard` reports a key by its evdev code, the physical key whatever the layout. The
//! guest is told the physical key too, and a key that types a character goes as text from
//! the layout instead (see [`super::xkb`]), so every layout types what its keys show.

use dsa_host_core::keys::{keycode, meta};

/// The evdev codes the mapping names, from `linux/input-event-codes.h`.
pub mod evdev_key {
    pub const ESC: u32 = 1;
    pub const KEY_1: u32 = 2;
    pub const KEY_0: u32 = 11;
    pub const MINUS: u32 = 12;
    pub const EQUAL: u32 = 13;
    pub const BACKSPACE: u32 = 14;
    pub const TAB: u32 = 15;
    pub const Q: u32 = 16;
    pub const P: u32 = 25;
    pub const LEFTBRACE: u32 = 26;
    pub const RIGHTBRACE: u32 = 27;
    pub const ENTER: u32 = 28;
    pub const LEFTCTRL: u32 = 29;
    pub const A: u32 = 30;
    pub const L: u32 = 38;
    pub const SEMICOLON: u32 = 39;
    pub const APOSTROPHE: u32 = 40;
    pub const GRAVE: u32 = 41;
    pub const LEFTSHIFT: u32 = 42;
    pub const BACKSLASH: u32 = 43;
    pub const Z: u32 = 44;
    pub const M: u32 = 50;
    pub const COMMA: u32 = 51;
    pub const DOT: u32 = 52;
    pub const SLASH: u32 = 53;
    pub const RIGHTSHIFT: u32 = 54;
    pub const KPASTERISK: u32 = 55;
    pub const LEFTALT: u32 = 56;
    pub const SPACE: u32 = 57;
    pub const CAPSLOCK: u32 = 58;
    pub const F1: u32 = 59;
    pub const F10: u32 = 68;
    pub const NUMLOCK: u32 = 69;
    pub const SCROLLLOCK: u32 = 70;
    pub const KP7: u32 = 71;
    pub const KP8: u32 = 72;
    pub const KP9: u32 = 73;
    pub const KPMINUS: u32 = 74;
    pub const KP4: u32 = 75;
    pub const KP5: u32 = 76;
    pub const KP6: u32 = 77;
    pub const KPPLUS: u32 = 78;
    pub const KP1: u32 = 79;
    pub const KP2: u32 = 80;
    pub const KP3: u32 = 81;
    pub const KP0: u32 = 82;
    pub const KPDOT: u32 = 83;
    pub const F11: u32 = 87;
    pub const F12: u32 = 88;
    pub const KPENTER: u32 = 96;
    pub const RIGHTCTRL: u32 = 97;
    pub const KPSLASH: u32 = 98;
    pub const SYSRQ: u32 = 99;
    pub const RIGHTALT: u32 = 100;
    pub const HOME: u32 = 102;
    pub const UP: u32 = 103;
    pub const PAGEUP: u32 = 104;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const END: u32 = 107;
    pub const DOWN: u32 = 108;
    pub const PAGEDOWN: u32 = 109;
    pub const INSERT: u32 = 110;
    pub const DELETE: u32 = 111;
    pub const MUTE: u32 = 113;
    pub const VOLUMEDOWN: u32 = 114;
    pub const VOLUMEUP: u32 = 115;
    pub const PAUSE: u32 = 119;
    pub const LEFTMETA: u32 = 125;
    pub const RIGHTMETA: u32 = 126;
    pub const COMPOSE: u32 = 127;
    pub const BACK: u32 = 158;
    pub const NEXTSONG: u32 = 163;
    pub const PLAYPAUSE: u32 = 164;
    pub const PREVIOUSSONG: u32 = 165;
    pub const STOPCD: u32 = 166;
}

/// The letters of the three letter rows, in evdev order from Q, A and Z.
const TOP_ROW: &[u8] = b"qwertyuiop";
const HOME_ROW: &[u8] = b"asdfghjkl";
const BOTTOM_ROW: &[u8] = b"zxcvbnm";

/// The guest's key code for the key with evdev code `code`; `None` for a key the guest has
/// no code for, which stays with the desktop.
pub fn guest_key(code: u32) -> Option<u32> {
    use evdev_key as k;
    let row = |letters: &[u8], first: u32| -> Option<u32> {
        letters
            .get((code - first) as usize)
            .and_then(|letter| keycode::letter(char::from(*letter)))
    };
    let key = match code {
        k::Q..=k::P => return row(TOP_ROW, k::Q),
        k::A..=k::L => return row(HOME_ROW, k::A),
        k::Z..=k::M => return row(BOTTOM_ROW, k::Z),
        k::KEY_1..=k::KEY_0 => {
            // 1 to 9, then 0.
            let digit = (code - k::KEY_1 + 1) % 10;
            return keycode::digit(digit);
        }
        k::F1..=k::F10 => return keycode::function(code - k::F1 + 1),
        k::F11 => keycode::F1 + 10,
        k::F12 => keycode::F12,
        k::ESC => keycode::ESCAPE,
        k::MINUS => keycode::MINUS,
        k::EQUAL => keycode::EQUALS,
        k::BACKSPACE => keycode::DEL,
        k::TAB => keycode::TAB,
        k::LEFTBRACE => keycode::LEFT_BRACKET,
        k::RIGHTBRACE => keycode::RIGHT_BRACKET,
        k::ENTER => keycode::ENTER,
        k::LEFTCTRL => keycode::CTRL_LEFT,
        k::RIGHTCTRL => keycode::CTRL_RIGHT,
        k::SEMICOLON => keycode::SEMICOLON,
        k::APOSTROPHE => keycode::APOSTROPHE,
        k::GRAVE => keycode::GRAVE,
        k::LEFTSHIFT => keycode::SHIFT_LEFT,
        k::RIGHTSHIFT => keycode::SHIFT_RIGHT,
        k::BACKSLASH => keycode::BACKSLASH,
        k::COMMA => keycode::COMMA,
        k::DOT => keycode::PERIOD,
        k::SLASH => keycode::SLASH,
        k::LEFTALT => keycode::ALT_LEFT,
        k::RIGHTALT => keycode::ALT_RIGHT,
        k::SPACE => keycode::SPACE,
        k::CAPSLOCK => keycode::CAPS_LOCK,
        k::NUMLOCK => keycode::NUM_LOCK,
        k::SCROLLLOCK => keycode::SCROLL_LOCK,
        k::KP0 => keycode::NUMPAD_0,
        k::KP1 => keycode::NUMPAD_0 + 1,
        k::KP2 => keycode::NUMPAD_0 + 2,
        k::KP3 => keycode::NUMPAD_0 + 3,
        k::KP4 => keycode::NUMPAD_0 + 4,
        k::KP5 => keycode::NUMPAD_0 + 5,
        k::KP6 => keycode::NUMPAD_0 + 6,
        k::KP7 => keycode::NUMPAD_0 + 7,
        k::KP8 => keycode::NUMPAD_0 + 8,
        k::KP9 => keycode::NUMPAD_9,
        k::KPASTERISK => keycode::NUMPAD_MULTIPLY,
        k::KPMINUS => keycode::NUMPAD_SUBTRACT,
        k::KPPLUS => keycode::NUMPAD_ADD,
        k::KPDOT => keycode::NUMPAD_DOT,
        k::KPSLASH => keycode::NUMPAD_DIVIDE,
        k::KPENTER => keycode::NUMPAD_ENTER,
        k::SYSRQ => keycode::SYSRQ,
        k::PAUSE => keycode::BREAK,
        k::HOME => keycode::MOVE_HOME,
        k::END => keycode::MOVE_END,
        k::UP => keycode::DPAD_UP,
        k::DOWN => keycode::DPAD_DOWN,
        k::LEFT => keycode::DPAD_LEFT,
        k::RIGHT => keycode::DPAD_RIGHT,
        k::PAGEUP => keycode::PAGE_UP,
        k::PAGEDOWN => keycode::PAGE_DOWN,
        k::INSERT => keycode::INSERT,
        k::DELETE => keycode::FORWARD_DEL,
        k::MUTE => keycode::VOLUME_MUTE,
        k::VOLUMEDOWN => keycode::VOLUME_DOWN,
        k::VOLUMEUP => keycode::VOLUME_UP,
        k::LEFTMETA => keycode::META_LEFT,
        k::RIGHTMETA => keycode::META_RIGHT,
        k::COMPOSE => keycode::MENU,
        k::BACK => keycode::BACK,
        k::NEXTSONG => keycode::MEDIA_NEXT,
        k::PLAYPAUSE => keycode::MEDIA_PLAY_PAUSE,
        k::PREVIOUSSONG => keycode::MEDIA_PREVIOUS,
        k::STOPCD => keycode::MEDIA_STOP,
        _ => return None,
    };
    Some(key)
}

/// The modifier masks of the standard XKB keymap (`wl_keyboard.modifiers`): Lock is Caps
/// Lock and Mod2 is Num Lock. A keymap that moves them is read through [`super::xkb`].
pub const XKB_LOCK_MASK: u32 = 1 << 1;
pub const XKB_MOD2_MASK: u32 = 1 << 4;

/// The guest's lock bits for the locked modifiers `wl_keyboard.modifiers` reports, given
/// which masks are Caps Lock and Num Lock in the keymap. Scroll Lock has no modifier in the
/// standard keymaps, so the key's own presses keep it.
pub fn locks(mods_locked: u32, caps_mask: u32, num_mask: u32) -> u32 {
    let mut bits = 0;
    if mods_locked & caps_mask != 0 {
        bits |= meta::CAPS_LOCK_ON;
    }
    if mods_locked & num_mask != 0 {
        bits |= meta::NUM_LOCK_ON;
    }
    bits
}

/// Whether a character a key typed goes to the guest as text: a printable character, typed
/// with neither Ctrl nor Alt held (those make shortcuts, which go as key events).
pub fn types_text(character: char, ctrl_or_alt: bool) -> bool {
    !ctrl_or_alt && !character.is_control()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr16_letters_digits_and_function_keys_are_named_by_the_physical_key() {
        assert_eq!(guest_key(evdev_key::Q), keycode::letter('q'));
        assert_eq!(guest_key(evdev_key::P), keycode::letter('p'));
        assert_eq!(guest_key(evdev_key::A), keycode::letter('a'));
        assert_eq!(guest_key(evdev_key::L), keycode::letter('l'));
        assert_eq!(guest_key(evdev_key::Z), keycode::letter('z'));
        assert_eq!(guest_key(evdev_key::M), keycode::letter('m'));
        assert_eq!(guest_key(evdev_key::KEY_1), keycode::digit(1));
        assert_eq!(guest_key(evdev_key::KEY_0), keycode::digit(0));
        assert_eq!(guest_key(evdev_key::F1), Some(keycode::F1));
        assert_eq!(guest_key(evdev_key::F10), keycode::function(10));
        assert_eq!(guest_key(evdev_key::F11), keycode::function(11));
        assert_eq!(guest_key(evdev_key::F12), Some(keycode::F12));
    }

    #[test]
    fn fr16_editing_and_navigation_keys_reach_the_guest() {
        assert_eq!(guest_key(evdev_key::BACKSPACE), Some(keycode::DEL));
        assert_eq!(guest_key(evdev_key::DELETE), Some(keycode::FORWARD_DEL));
        assert_eq!(guest_key(evdev_key::ENTER), Some(keycode::ENTER));
        assert_eq!(guest_key(evdev_key::KPENTER), Some(keycode::NUMPAD_ENTER));
        assert_eq!(guest_key(evdev_key::LEFT), Some(keycode::DPAD_LEFT));
        assert_eq!(guest_key(evdev_key::HOME), Some(keycode::MOVE_HOME));
        assert_eq!(guest_key(evdev_key::ESC), Some(keycode::ESCAPE));
        assert_eq!(guest_key(evdev_key::RIGHTCTRL), Some(keycode::CTRL_RIGHT));
        assert_eq!(guest_key(evdev_key::RIGHTSHIFT), Some(keycode::SHIFT_RIGHT));
        assert_eq!(guest_key(evdev_key::KP7), Some(keycode::NUMPAD_0 + 7));
        assert_eq!(guest_key(0x2ff), None, "an unknown key stays with the desktop");
    }

    #[test]
    fn fr16_caps_lock_and_num_lock_come_from_the_locked_modifiers() {
        let both = XKB_LOCK_MASK | XKB_MOD2_MASK;
        assert_eq!(
            locks(both, XKB_LOCK_MASK, XKB_MOD2_MASK),
            meta::CAPS_LOCK_ON | meta::NUM_LOCK_ON
        );
        assert_eq!(locks(0, XKB_LOCK_MASK, XKB_MOD2_MASK), 0);
    }

    #[test]
    fn fr6_a_printable_character_goes_as_text_and_a_shortcut_as_a_key() {
        assert!(types_text('a', false));
        assert!(types_text('\u{e9}', false));
        assert!(!types_text('a', true), "Ctrl+A is a shortcut");
        assert!(!types_text('\r', false), "Enter goes as a key");
        assert!(!types_text('\u{8}', false), "Backspace goes as a key");
    }
}
