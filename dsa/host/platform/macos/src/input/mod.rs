//! Input in a task window on macOS.
//!
//! - [`keys`]: virtual key codes and modifier flags as the guest's keys and locks.
//! - [`pointer`]: the mouse, the trackpad and the pen as window input.
//! - [`text_input`]: the input method's `NSTextInputClient` calls as compositions and commits.
//! - `view` (macOS only): the content view that receives all of it.

pub mod keys;
pub mod pointer;
pub mod text_input;

#[cfg(target_os = "macos")]
pub mod view;
