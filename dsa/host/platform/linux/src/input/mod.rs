//! Input from the Linux seat, named and shaped as the guest receives it.
//!
//! - [`keymap`]: the kernel's key codes (what `wl_keyboard` and evdev report) as the guest's.
//! - [`pointer`]: `wl_pointer` buttons and wheel steps, and the window edge a press resizes.
//! - [`text_input`]: an input method's preedit and commits (text-input-v3) as compositions.
//! - [`gamepad`]: a controller's evdev buttons and axes as the guest's gamepad state.
//! - [`xkb`]: the keyboard layout, loaded from libxkbcommon at run time (Linux only).
//! - [`evdev`]: the game controllers under `/dev/input` (Linux only).

pub mod gamepad;
pub mod keymap;
pub mod pointer;
pub mod text_input;

#[cfg(target_os = "linux")]
pub mod evdev;
#[cfg(target_os = "linux")]
pub mod xkb;
