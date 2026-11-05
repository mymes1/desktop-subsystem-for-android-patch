//! The per-user agent: its configuration and its launchd property list.
#![cfg(unix)]

use std::path::Path;

use dsa_host_macos::host_config::{memory_range_mb, MacHostConfig, AUTOMATIC_GROWTH_CAP_MB};
use dsa_host_macos::launchd::{
    domain, install_commands, plist, plist_path, service_target, start_command, LABEL,
};

#[test]
fn fr2_the_configuration_names_an_image_to_boot() {
    assert!(MacHostConfig::parse("{}").is_err());
    assert!(MacHostConfig::parse(r#"{"images_dir": "/Library/Application Support/DSA"}"#).is_err());
    let config = MacHostConfig::parse(r#"{"image_dir": "/images/16"}"#).unwrap();
    assert_eq!(config.control_port, 5000);
    assert_eq!(config.log_port, 5001);
    assert!(config.network && config.pstore);
    assert!(MacHostConfig::parse(r#"{"image_dir": "/i", "unknown": 1}"#).is_err());
}

#[test]
fn pr5_the_users_files_and_socket_are_in_their_own_folder() {
    let config = MacHostConfig::parse(r#"{"image_dir": "/images/16"}"#).unwrap();
    let user = config.user(Path::new("/Users/ann"));
    let root = Path::new("/Users/ann/Library/Application Support/DSA");
    assert_eq!(user.root, root);
    // Where dsa looks for the local API.
    assert_eq!(config.socket(&user), root.join("host.sock"));
    assert_eq!(user.disk(), root.join("android.img"));
    assert!(user.console_log().starts_with(root.join("logs")));
    assert_eq!(config.settings_dir(&user), root.join("settings"));
}

#[test]
fn nfr2_memory_starts_small_and_grows_to_what_the_mac_can_spare() {
    // No limit: the default start, growing to the Mac's memory less its reserve, capped.
    assert_eq!(memory_range_mb(0, Some(16384)), (3072, AUTOMATIC_GROWTH_CAP_MB));
    assert_eq!(memory_range_mb(0, Some(8192)).0, 3072);
    assert!(memory_range_mb(0, Some(8192)).1 <= 8192);
    // A limit is how far it grows; a low one is also where it starts.
    assert_eq!(memory_range_mb(6144, Some(32768)), (3072, 6144));
    assert_eq!(memory_range_mb(2048, Some(32768)), (2048, 2048));
    // Unknown memory: the start, fixed.
    assert_eq!(memory_range_mb(0, None), (3072, 3072));
    let fixed = MacHostConfig::parse(r#"{"image_dir": "/i", "memory_mb": 4096}"#).unwrap();
    assert_eq!(fixed.memory_mb(0, Some(65536)), (4096, 4096));
}

#[test]
fn fr2_launchd_starts_the_agent_at_login_and_again_after_a_failure() {
    let text = plist(
        Path::new("/Applications/DSA.app/Contents/MacOS/dsa-hostd"),
        Some(Path::new("/Library/Application Support/DSA/dsa-hostd.json")),
        Path::new("/Users/ann/Library/Application Support/DSA/logs/dsa-hostd.log"),
    );
    assert!(text.contains(&format!("<string>{LABEL}</string>")));
    assert!(text.contains("<string>/Applications/DSA.app/Contents/MacOS/dsa-hostd</string>"));
    assert!(text.contains("<string>serve</string>"));
    assert!(text.contains("<key>RunAtLoad</key>\n\t<true/>"));
    assert!(text.contains("<key>SuccessfulExit</key>\n\t\t<false/>"));
    assert!(text.contains("<string>Aqua</string>"));
    let escaped = plist(Path::new("/a&b/dsa-hostd"), None, Path::new("/l"));
    assert!(escaped.contains("/a&amp;b/dsa-hostd"));
    assert_eq!(
        plist_path(Path::new("/Users/ann")),
        Path::new("/Users/ann/Library/LaunchAgents").join(format!("{LABEL}.plist"))
    );
}

#[test]
fn fr2_the_agent_is_loaded_into_the_users_login_session() {
    assert_eq!(domain(501), "gui/501");
    assert_eq!(service_target(501), format!("gui/501/{LABEL}"));
    let commands = install_commands(501, Path::new("/p.plist"));
    assert_eq!(commands[0], vec!["bootout".to_string(), service_target(501)]);
    assert_eq!(
        commands[1],
        vec!["bootstrap".to_string(), "gui/501".into(), "/p.plist".into()]
    );
    assert_eq!(start_command(501)[0], "kickstart");
}
