//! Gives each program its name, version and icon, so Task Manager and the taskbar show
//! "Desktop Subsystem for Android Service" and "DSA Window Manager" rather than a file name.

use dsa_exe_resources::{embed, embed_with, names, shell_string_tables};

const ICON: &str = "installer/setup-ui/assets/logo.ico";

fn main() {
    embed("dsa-hostd", names::SERVICE, None, ICON);
    embed(
        "dsa-wm",
        names::WINDOW_MANAGER,
        Some(names::WINDOW_MANAGER_KO),
        ICON,
    );
    // Each app's copy of the window host gets the app's name and icon instead. The installed
    // one also carries the words Explorer and the Start menu show for DSA, in English and
    // Korean, which the installer points at as indirect strings.
    embed_with(
        "dsa-app-window",
        names::APP_WINDOW,
        None,
        ICON,
        shell_string_tables(),
    );
}
