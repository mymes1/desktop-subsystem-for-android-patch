//! Keys on macOS: an `NSEvent`'s virtual key code (the `kVK_` codes, which name a key by
//! where it sits on an ANSI keyboard, whatever the layout) as the guest's key code, and its
//! modifier flags as the guest's locks.

use dsa_host_core::keys::{keycode, meta};

/// Android's `KEYCODE_CLEAR`, which the keypad's Clear key is.
pub const ANDROID_CLEAR: u32 = 28;
/// Android's `KEYCODE_FUNCTION`, the `fn` key.
pub const ANDROID_FUNCTION: u32 = 119;
/// Android's `KEYCODE_NUMPAD_EQUALS`.
pub const ANDROID_NUMPAD_EQUALS: u32 = 161;

/// `NSEventModifierFlagCapsLock`.
pub const FLAG_CAPS_LOCK: u64 = 1 << 16;

// The device-dependent bits of an NSEvent's modifier flags, which say which of a pair of
// modifier keys is down (IOKit's NX_DEVICE*KEYMASK).
const DEVICE_LEFT_CONTROL: u64 = 0x0000_0001;
const DEVICE_LEFT_SHIFT: u64 = 0x0000_0002;
const DEVICE_RIGHT_SHIFT: u64 = 0x0000_0004;
const DEVICE_LEFT_COMMAND: u64 = 0x0000_0008;
const DEVICE_RIGHT_COMMAND: u64 = 0x0000_0010;
const DEVICE_LEFT_OPTION: u64 = 0x0000_0020;
const DEVICE_RIGHT_OPTION: u64 = 0x0000_0040;
const DEVICE_RIGHT_CONTROL: u64 = 0x0000_2000;
/// `NSEventModifierFlagFunction`.
const FLAG_FUNCTION: u64 = 1 << 23;

/// The guest's key code for the key with macOS virtual key code `code`. `None` for a key the
/// guest has no code for (F13 and up).
pub fn guest_key(code: u16) -> Option<u32> {
    let letter = keycode::letter;
    let digit = keycode::digit;
    let numpad = keycode::numpad_digit;
    let function = keycode::function;
    Some(match code {
        0x00 => letter('a')?,
        0x01 => letter('s')?,
        0x02 => letter('d')?,
        0x03 => letter('f')?,
        0x04 => letter('h')?,
        0x05 => letter('g')?,
        0x06 => letter('z')?,
        0x07 => letter('x')?,
        0x08 => letter('c')?,
        0x09 => letter('v')?,
        0x0B => letter('b')?,
        0x0C => letter('q')?,
        0x0D => letter('w')?,
        0x0E => letter('e')?,
        0x0F => letter('r')?,
        0x10 => letter('y')?,
        0x11 => letter('t')?,
        0x12 => digit(1)?,
        0x13 => digit(2)?,
        0x14 => digit(3)?,
        0x15 => digit(4)?,
        0x16 => digit(6)?,
        0x17 => digit(5)?,
        0x18 => keycode::EQUALS,
        0x19 => digit(9)?,
        0x1A => digit(7)?,
        0x1B => keycode::MINUS,
        0x1C => digit(8)?,
        0x1D => digit(0)?,
        0x1E => keycode::RIGHT_BRACKET,
        0x1F => letter('o')?,
        0x20 => letter('u')?,
        0x21 => keycode::LEFT_BRACKET,
        0x22 => letter('i')?,
        0x23 => letter('p')?,
        0x24 => keycode::ENTER,
        0x25 => letter('l')?,
        0x26 => letter('j')?,
        0x27 => keycode::APOSTROPHE,
        0x28 => letter('k')?,
        0x29 => keycode::SEMICOLON,
        0x2A => keycode::BACKSLASH,
        0x2B => keycode::COMMA,
        0x2C => keycode::SLASH,
        0x2D => letter('n')?,
        0x2E => letter('m')?,
        0x2F => keycode::PERIOD,
        0x30 => keycode::TAB,
        0x31 => keycode::SPACE,
        0x32 => keycode::GRAVE,
        // The key labelled delete erases backward, as Android's DEL.
        0x33 => keycode::DEL,
        0x35 => keycode::ESCAPE,
        0x36 => keycode::META_RIGHT,
        0x37 => keycode::META_LEFT,
        0x38 => keycode::SHIFT_LEFT,
        0x39 => keycode::CAPS_LOCK,
        0x3A => keycode::ALT_LEFT,
        0x3B => keycode::CTRL_LEFT,
        0x3C => keycode::SHIFT_RIGHT,
        0x3D => keycode::ALT_RIGHT,
        0x3E => keycode::CTRL_RIGHT,
        0x3F => ANDROID_FUNCTION,
        0x41 => keycode::NUMPAD_DOT,
        0x43 => keycode::NUMPAD_MULTIPLY,
        0x45 => keycode::NUMPAD_ADD,
        0x47 => ANDROID_CLEAR,
        0x48 => keycode::VOLUME_UP,
        0x49 => keycode::VOLUME_DOWN,
        0x4A => keycode::VOLUME_MUTE,
        0x4B => keycode::NUMPAD_DIVIDE,
        0x4C => keycode::NUMPAD_ENTER,
        0x4E => keycode::NUMPAD_SUBTRACT,
        0x51 => ANDROID_NUMPAD_EQUALS,
        0x52 => numpad(0)?,
        0x53 => numpad(1)?,
        0x54 => numpad(2)?,
        0x55 => numpad(3)?,
        0x56 => numpad(4)?,
        0x57 => numpad(5)?,
        0x58 => numpad(6)?,
        0x59 => numpad(7)?,
        0x5B => numpad(8)?,
        0x5C => numpad(9)?,
        0x60 => function(5)?,
        0x61 => function(6)?,
        0x62 => function(7)?,
        0x63 => function(3)?,
        0x64 => function(8)?,
        0x65 => function(9)?,
        0x67 => function(11)?,
        0x6D => function(10)?,
        0x6F => function(12)?,
        // Help sits where Insert is on a PC keyboard, and is what a PC keyboard's Insert sends.
        0x72 => keycode::INSERT,
        0x73 => keycode::MOVE_HOME,
        0x74 => keycode::PAGE_UP,
        0x75 => keycode::FORWARD_DEL,
        0x76 => function(4)?,
        0x77 => keycode::MOVE_END,
        0x78 => function(2)?,
        0x79 => keycode::PAGE_DOWN,
        0x7A => function(1)?,
        0x7B => keycode::DPAD_LEFT,
        0x7C => keycode::DPAD_RIGHT,
        0x7D => keycode::DPAD_DOWN,
        0x7E => keycode::DPAD_UP,
        _ => return None,
    })
}

/// The locks on, as the guest's meta state bits, for an event's modifier flags. A Mac has no
/// Num Lock and its keypad always types digits, so Num Lock is always on.
pub fn locks(flags: u64) -> u32 {
    let mut bits = meta::NUM_LOCK_ON;
    if flags & FLAG_CAPS_LOCK != 0 {
        bits |= meta::CAPS_LOCK_ON;
    }
    bits
}

/// What a `flagsChanged:` event did to the modifier key with virtual key code `code`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierChange {
    /// The key went down or up.
    Key { key_code: u32, down: bool },
    /// Caps Lock was pressed: macOS reports the lock, not the key, so the guest gets a press
    /// and a release.
    CapsLockPressed,
}

/// The change a `flagsChanged:` event with virtual key code `code` and modifier flags `flags`
/// made. `None` for a key that is not a modifier.
pub fn modifier_change(code: u16, flags: u64) -> Option<ModifierChange> {
    let bit = match code {
        0x37 => DEVICE_LEFT_COMMAND,
        0x36 => DEVICE_RIGHT_COMMAND,
        0x38 => DEVICE_LEFT_SHIFT,
        0x3C => DEVICE_RIGHT_SHIFT,
        0x3A => DEVICE_LEFT_OPTION,
        0x3D => DEVICE_RIGHT_OPTION,
        0x3B => DEVICE_LEFT_CONTROL,
        0x3E => DEVICE_RIGHT_CONTROL,
        0x3F => FLAG_FUNCTION,
        0x39 => return Some(ModifierChange::CapsLockPressed),
        _ => return None,
    };
    Some(ModifierChange::Key {
        key_code: guest_key(code)?,
        down: flags & bit != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr16_letters_digits_and_punctuation_keep_their_place_on_the_keyboard() {
        assert_eq!(guest_key(0x00), keycode::letter('a'));
        assert_eq!(guest_key(0x06), keycode::letter('z'));
        assert_eq!(guest_key(0x1D), keycode::digit(0));
        assert_eq!(guest_key(0x17), keycode::digit(5));
        assert_eq!(guest_key(0x2C), Some(keycode::SLASH));
        assert_eq!(guest_key(0x32), Some(keycode::GRAVE));
    }

    #[test]
    fn fr16_every_key_on_a_mac_keyboard_the_guest_knows_reaches_it() {
        let mapped = (0u16..0x80).filter(|code| guest_key(*code).is_some()).count();
        // 26 letters, 10 digits, 11 punctuation, 12 function keys, 17 keypad keys, editing
        // and navigation keys, modifiers and the volume keys.
        assert!(mapped >= 100, "only {mapped} keys are mapped");
        for code in 0u16..0x80 {
            if let Some(key) = guest_key(code) {
                assert!(key > 0 && key <= keycode::MAX, "{code:#x} maps to {key}");
            }
        }
    }

    #[test]
    fn fr16_delete_erases_backward_and_forward_delete_forward() {
        assert_eq!(guest_key(0x33), Some(keycode::DEL));
        assert_eq!(guest_key(0x75), Some(keycode::FORWARD_DEL));
        assert_eq!(guest_key(0x24), Some(keycode::ENTER));
        assert_eq!(guest_key(0x4C), Some(keycode::NUMPAD_ENTER));
    }

    #[test]
    fn fr16_command_is_the_meta_key_and_option_is_alt() {
        assert_eq!(guest_key(0x37), Some(keycode::META_LEFT));
        assert_eq!(guest_key(0x36), Some(keycode::META_RIGHT));
        assert_eq!(guest_key(0x3A), Some(keycode::ALT_LEFT));
        assert_eq!(guest_key(0x3D), Some(keycode::ALT_RIGHT));
    }

    #[test]
    fn fr16_a_modifier_is_down_while_its_own_side_is_held() {
        assert_eq!(
            modifier_change(0x38, DEVICE_LEFT_SHIFT | (1 << 17)),
            Some(ModifierChange::Key {
                key_code: keycode::SHIFT_LEFT,
                down: true
            })
        );
        // The right shift is still held; the left one came up.
        assert_eq!(
            modifier_change(0x38, DEVICE_RIGHT_SHIFT | (1 << 17)),
            Some(ModifierChange::Key {
                key_code: keycode::SHIFT_LEFT,
                down: false
            })
        );
        assert_eq!(
            modifier_change(0x39, FLAG_CAPS_LOCK),
            Some(ModifierChange::CapsLockPressed)
        );
        assert_eq!(modifier_change(0x00, 0), None);
    }

    #[test]
    fn fr16_caps_lock_is_reported_and_num_lock_is_always_on() {
        assert_eq!(locks(0), meta::NUM_LOCK_ON);
        assert_eq!(locks(FLAG_CAPS_LOCK), meta::NUM_LOCK_ON | meta::CAPS_LOCK_ON);
    }
}
