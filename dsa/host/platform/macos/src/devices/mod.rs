//! The Mac's devices and settings, as the platform-neutral host's device bridges take them.
//!
//! - [`settings`]: time zone, languages, regional format, dark mode and accent color.
//! - [`gamepads`]: game controllers through the GameController framework.
//! - [`location`]: CoreLocation's fixes and the user's permission.
//! - [`camera`]: the cameras, through AVFoundation, as NV12 pictures.
//! - [`shared_folders`]: the user's folders shared with the guest.
//! - `audio`, `print` (macOS only): sound through Audio Queue Services, and printing through
//!   PDFKit and `NSPrintOperation`.

pub mod camera;
pub mod gamepads;
pub mod location;
pub mod settings;
pub mod shared_folders;

#[cfg(target_os = "macos")]
pub mod audio;
#[cfg(target_os = "macos")]
pub mod print;
