//! An Android app's window host on macOS: the program inside each app's bundle in
//! `~/Applications/DSA Apps`, which owns the app's task windows.
//!
//! `dsa-app-window [--package <name>] [--launch]`. Without `--package` the package is the
//! one the bundle's `Info.plist` names. Started from Launchpad, Spotlight or the Dock it asks
//! for the app to be started; the window manager passes `--shown-by-window-manager` when it
//! starts the program itself.

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("dsa-app-window runs on macOS only");
    std::process::ExitCode::FAILURE
}

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    use dsa_host_macos::compositor::window_host::{
        host_api_socket, package_of, run, support_folder, window_manager_socket, HostSetup,
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(package) = package_of(&args) else {
        eprintln!("dsa-app-window: no app to show; pass --package or run it from an app bundle");
        return std::process::ExitCode::FAILURE;
    };
    let Some(support) = support_folder() else {
        eprintln!("dsa-app-window: HOME is not set, so DSA's folder cannot be found");
        return std::process::ExitCode::FAILURE;
    };
    let setup = HostSetup {
        package,
        window_manager: window_manager_socket(&support),
        host_api: host_api_socket(&support),
        launch: !args.iter().any(|a| a == "--shown-by-window-manager"),
    };
    match run(setup) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("dsa-app-window: {why}");
            std::process::ExitCode::FAILURE
        }
    }
}
