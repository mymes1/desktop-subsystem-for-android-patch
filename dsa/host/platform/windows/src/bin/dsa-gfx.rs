//! `dsa-gfx`, the graphics renderer of Desktop Subsystem for Android: Task Manager shows it as
//! "Graphics renderer for Android".
//!
//! The window manager (`dsa-wm.exe`) starts it, with the pipe to dial back on
//! (`--window-manager <pipe>`), and starts it again whenever it ends. It loads
//! `gfxstream_backend.dll` from beside itself, takes the guest's graphics streams on the VM's
//! Hyper-V sockets, and copies each task's frames on the GPU into the textures the window
//! manager hands it. It has no window and no console, and ends with its window manager.
#![windows_subsystem = "windows"]

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("this build of dsa-gfx runs on Windows only");
    std::process::ExitCode::FAILURE
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    dsa_host_windows::renderer::process::run(&args)
}
