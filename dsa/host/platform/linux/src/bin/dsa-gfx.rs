//! `dsa-gfx`, the graphics renderer of Desktop Subsystem for Android on Linux.
//!
//! The compositor (`dsa-compositor`) starts it with its end of a socket pair
//! (`--window-manager <descriptor>`) and starts it again whenever it ends. It loads
//! `libgfxstream_backend.so` from beside itself, takes the guest's graphics streams, and hands
//! each task's frames to the compositor as dmabufs. It ends with the compositor.

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("this build of dsa-gfx runs on Linux only");
    std::process::ExitCode::FAILURE
}

#[cfg(target_os = "linux")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    dsa_host_linux::renderer::process::run(&args)
}
