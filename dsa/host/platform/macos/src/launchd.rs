//! The per-user launchd agent that runs `dsa-hostd`.
//!
//! On macOS each user's Android runs in a process of that user: the Virtualization framework
//! needs no privilege, and a VM belongs to the process that made it. launchd starts the
//! agent when the user logs in and again if it ends with an error, as the Windows service
//! manager does for the Windows host. Android itself starts only when something asks for it
//! through the local API.
//!
//! The agent is a property list in `~/Library/LaunchAgents`, loaded into the user's GUI
//! domain (`gui/<uid>`), so it runs in their login session with access to their windows.

use std::path::{Path, PathBuf};

/// The agent's launchd label.
pub const LABEL: &str = "io.github.thisisthepy.dsa.hostd";

/// Where the agent's property list goes, below the user's home.
pub fn plist_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// The launchd domain of the user `uid`'s login session.
pub fn domain(uid: u32) -> String {
    format!("gui/{uid}")
}

/// The agent's service target, for `launchctl kickstart` and `bootout`.
pub fn service_target(uid: u32) -> String {
    format!("{}/{LABEL}", domain(uid))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The agent's property list: `program serve [config]`, started at login, restarted when it
/// ends with an error, its output appended to `log`.
pub fn plist(program: &Path, config: Option<&Path>, log: &Path) -> String {
    let mut arguments = vec![program.to_string_lossy().into_owned(), "serve".into()];
    if let Some(config) = config {
        arguments.push(config.to_string_lossy().into_owned());
    }
    let arguments: String = arguments
        .iter()
        .map(|a| format!("\t\t<string>{}</string>\n", xml_escape(a)))
        .collect();
    let log = xml_escape(&log.to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{LABEL}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         {arguments}\
         \t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<dict>\n\
         \t\t<key>SuccessfulExit</key>\n\
         \t\t<false/>\n\
         \t</dict>\n\
         \t<key>LimitLoadToSessionType</key>\n\
         \t<string>Aqua</string>\n\
         \t<key>ProcessType</key>\n\
         \t<string>Interactive</string>\n\
         \t<key>StandardOutPath</key>\n\
         \t<string>{log}</string>\n\
         \t<key>StandardErrorPath</key>\n\
         \t<string>{log}</string>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// The `launchctl` invocations that install the agent for user `uid`: unload any earlier
/// copy (which may fail, as there may be none), then load the new one, which starts it.
pub fn install_commands(uid: u32, plist: &Path) -> Vec<Vec<String>> {
    let plist = plist.to_string_lossy().into_owned();
    vec![
        vec!["bootout".into(), service_target(uid)],
        vec!["bootstrap".into(), domain(uid), plist],
    ]
}

/// The `launchctl` invocation that stops and unloads the agent.
pub fn uninstall_commands(uid: u32) -> Vec<Vec<String>> {
    vec![vec!["bootout".into(), service_target(uid)]]
}

/// The `launchctl` invocation that starts the agent if it is not running.
pub fn start_command(uid: u32) -> Vec<String> {
    vec!["kickstart".into(), service_target(uid)]
}

/// Runs `launchctl` with `arguments`. An error says what launchctl said.
#[cfg(target_os = "macos")]
pub fn launchctl(arguments: &[String]) -> Result<(), String> {
    let output = std::process::Command::new("/bin/launchctl")
        .args(arguments)
        .output()
        .map_err(|e| format!("running launchctl: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "launchctl {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Writes the agent's property list for the user whose home is `home` and loads it.
#[cfg(target_os = "macos")]
pub fn install(
    home: &Path,
    uid: u32,
    program: &Path,
    config: Option<&Path>,
) -> Result<PathBuf, String> {
    let path = plist_path(home);
    let log = crate::host_config::user_root_in(home)
        .join("logs")
        .join("dsa-hostd.log");
    for dir in [path.parent(), log.parent()].into_iter().flatten() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, plist(program, config, &log))
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
    let commands = install_commands(uid, &path);
    // There may be no earlier copy to unload.
    let _ = launchctl(&commands[0]);
    launchctl(&commands[1])?;
    Ok(path)
}

/// Unloads the agent and removes its property list.
#[cfg(target_os = "macos")]
pub fn uninstall(home: &Path, uid: u32) -> Result<(), String> {
    for command in uninstall_commands(uid) {
        // Not loaded is as good as unloaded.
        let _ = launchctl(&command);
    }
    let path = plist_path(home);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("removing {}: {e}", path.display())),
    }
}
