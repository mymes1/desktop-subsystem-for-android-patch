//! Root elevation through the daemon's router: the host's config, as it arrives on the
//! control channel, drives the kernel's su guard (a recorder here) and the su relay, and the
//! host's decisions answer the calls the relay holds.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use dsa_guest_channeld::router::{Peer, Router};
use dsa_guest_channeld::su_relay::{ConfigApplier, GuardControl, Outcome, SuRelay};
use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::{Envelope, RootApp, RootElevationConfig, RootLevel, SuDecision};

/// The kernel's control file, as the writes made to it, each a list of commands.
#[derive(Default, Clone)]
struct Guard {
    writes: Arc<Mutex<Vec<Vec<String>>>>,
}

impl Guard {
    /// The commands of the one write made since the last look.
    fn take(&self) -> Vec<String> {
        let writes = std::mem::take(&mut *self.writes.lock().unwrap());
        assert!(writes.len() <= 1, "one config is one write: {writes:?}");
        writes.into_iter().next().unwrap_or_default()
    }
}

impl GuardControl for Guard {
    fn write(&self, lines: &[String]) -> std::io::Result<()> {
        self.writes.lock().unwrap().push(lines.to_vec());
        Ok(())
    }
}

const PACKAGES: &str = "com.ask 10100 0 /data/user/0/com.ask default 3003\n\
                        com.remember 10102 0 /data/user/0/com.remember default 3003\n";

fn daemon() -> (Router, Arc<SuRelay>, Guard, Arc<Mutex<Vec<String>>>) {
    let (to_host, _host) = mpsc::channel();
    let relay = Arc::new(SuRelay::new());
    let guard = Guard::default();
    let said = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&said);
    let applier = ConfigApplier::new(
        Arc::clone(&relay),
        Box::new(guard.clone()),
        Box::new(|| Some(PACKAGES.to_string())),
        move |line| log.lock().unwrap().push(line.to_string()),
    );
    let router = Router::new(to_host, |_peer: Peer| Ok(())).on_root_elevation(
        Arc::clone(&relay),
        move |config| {
            applier.apply(config);
        },
    );
    (router, relay, guard, said)
}

/// The config as the host sends it: by package, with no uid.
fn config(enabled: bool, apps: &[(&str, RootLevel)]) -> Vec<u8> {
    dsa_protocol::encode(&Envelope {
        body: Some(Body::RootElevationConfig(RootElevationConfig {
            enabled,
            apps: apps
                .iter()
                .map(|(package, level)| RootApp {
                    package: package.to_string(),
                    uid: 0,
                    level: (*level).into(),
                })
                .collect(),
        })),
    })
}

#[test]
fn fr35_the_hosts_config_writes_the_kernel_guard() {
    let (router, relay, guard, _said) = daemon();
    router.from_host(&config(
        true,
        &[
            ("com.ask", RootLevel::Ask),
            ("com.off", RootLevel::Off),
            ("com.remember", RootLevel::Remember),
        ],
    ));
    let lines = guard.take();
    assert_eq!(lines.first().map(String::as_str), Some("reset"));
    assert_eq!(lines.last().map(String::as_str), Some("enable"));
    // The uids come from the package list.
    assert!(lines.contains(&"allow 10100".to_string()));
    assert!(lines.contains(&"allow 10102".to_string()));
    assert_eq!(
        lines.iter().filter(|l| l.starts_with("allow ")).count(),
        2,
        "only the ask and remember apps may see su: {lines:?}"
    );
    assert!(relay.allows(10100));
    assert!(relay.allows(10102));
}

#[test]
fn fr35_a_changed_config_applies_at_once() {
    let (router, relay, guard, _said) = daemon();
    router.from_host(&config(true, &[("com.ask", RootLevel::Ask)]));
    guard.take();
    let (_, _, waiting) = relay.begin("com.ask", 10100, vec!["id".into()], false);

    // The user sets com.ask to off and com.remember to remember, with Android running.
    router.from_host(&config(true, &[("com.remember", RootLevel::Remember)]));
    let lines = guard.take();
    assert!(!lines.contains(&"allow 10100".to_string()));
    assert!(lines.contains(&"allow 10102".to_string()));
    assert_eq!(lines.first().map(String::as_str), Some("reset"));
    assert!(!relay.allows(10100));
    assert!(matches!(waiting.recv().unwrap(), Outcome::Refused(_)));

    // The administrator turns the feature off: su is hidden from every app at once.
    router.from_host(&config(false, &[("com.remember", RootLevel::Remember)]));
    let lines = guard.take();
    assert_eq!(lines.last().map(String::as_str), Some("disable"));
    assert!(lines.contains(&"path /system/bin/su".to_string()));
    assert!(!lines.iter().any(|l| l.starts_with("allow ")));
    assert!(!relay.allows(10102));
}

#[test]
fn fr35_the_daemon_hides_su_from_every_app_when_it_starts() {
    let (_router, relay, guard, _said) = daemon();
    let applier = ConfigApplier::new(
        Arc::clone(&relay),
        Box::new(guard.clone()),
        Box::new(|| None),
        |_: &str| {},
    );
    applier.hide_all();
    let lines = guard.take();
    assert_eq!(lines.first().map(String::as_str), Some("reset"));
    assert_eq!(lines.last().map(String::as_str), Some("disable"));
    for path in ["/system/bin/su", "/system/xbin/su", "/sbin/su"] {
        assert!(lines.contains(&format!("path {path}")), "{lines:?}");
    }
    assert!(!lines.iter().any(|l| l.starts_with("allow ")));
}

#[test]
fn fr35_the_kernel_guard_takes_a_config_in_one_write() {
    use dsa_guest_channeld::su_relay::KernelGuard;
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("fr35-dsa-su-control");
    std::fs::write(&path, b"").unwrap();
    let guard = KernelGuard { path: path.clone() };
    let lines: Vec<String> = ["reset", "path /sbin/su", "allow 10100", "enable"]
        .iter()
        .map(|line| line.to_string())
        .collect();
    guard.write(&lines).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "reset\npath /sbin/su\nallow 10100\nenable\n"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn fr35_an_app_not_installed_sees_no_su() {
    let (router, relay, guard, said) = daemon();
    router.from_host(&config(true, &[("com.missing", RootLevel::Ask)]));
    let lines = guard.take();
    assert!(!lines.iter().any(|l| l.starts_with("allow ")));
    assert!(!relay.allows(0));
    assert!(said
        .lock()
        .unwrap()
        .iter()
        .any(|line| line.contains("com.missing")));
}

#[test]
fn fr35_the_hosts_decision_reaches_the_waiting_su_call() {
    let (router, relay, _guard, _said) = daemon();
    router.from_host(&config(true, &[("com.ask", RootLevel::Ask)]));
    let (request_id, _frame, waiting) = relay.begin("com.ask", 10100, vec!["id".into()], false);
    router.from_host(&dsa_protocol::encode(&Envelope {
        body: Some(Body::SuDecision(SuDecision {
            request_id,
            approved: true,
            reason: String::new(),
        })),
    }));
    assert_eq!(waiting.recv().unwrap(), Outcome::Approved);
}
