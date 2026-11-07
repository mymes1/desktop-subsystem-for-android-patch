//! `dsa-gfx`, the graphics renderer of Desktop Subsystem for Android on macOS.
//!
//! The window manager (`dsa-wm`) starts it from DSA.app with its end of a socket pair
//! (`--window-manager <descriptor>`) and starts it again whenever it ends. It loads
//! `libgfxstream_backend.dylib` from the bundle's Frameworks, serves the guest's graphics
//! streams, and copies each task's frames into the window manager's surfaces. It ends with the
//! window manager.

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("this build of dsa-gfx runs on macOS only");
    std::process::ExitCode::FAILURE
}

#[cfg(target_os = "macos")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    dsa_host_macos::gfx::process::run(&args)
}
