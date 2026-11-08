//! The guest side of root elevation: the su relay.
//!
//! When the host turns root elevation on for this boot, the daemon drives the kernel's su
//! guard (`security/dsa_su`) so the image's su binary exists only for the uids of the apps
//! the user set to ask or remember. su itself does not decide anything: it connects to the
//! relay over a guest-local socket, the relay asks the host ([`control::SuRequest`]), and the
//! host answers ([`control::SuDecision`]). Only on approval does the daemon run the command
//! as uid 0.
//!
//! This module is the relay's rule and bookkeeping, which runs and is tested without a
//! socket, a kernel or a child process:
//! - [`SuRelay::apply_config`] turns the host's [`control::RootElevationConfig`] into the
//!   lines the kernel's control file takes, keeps it as the relay's current config, and
//!   refuses every waiting call whose app may no longer ask.
//! - [`ConfigApplier::apply`] does that for each config the host sends (on connect and on
//!   every change), names each app by its uid from the package list, and writes the lines to
//!   the kernel's control file, so a change applies to the running Android at once.
//! - [`SuRelay::begin`] registers a waiting su call and returns the frame to send the host.
//! - [`SuRelay::decide`] matches a host decision to its call.
//! - [`SuRelay::expire`] fails a call that waited past [`ANSWER_TIMEOUT`].
//!
//! The socket server that su connects to and the uid 0 child that runs the approved command
//! are the only parts that touch the system; this logic drives them. They are wired into the
//! daemon's main loop, which this module does not reach into.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::{Envelope, RootElevationConfig, RootLevel, SuDecision, SuRequest};

/// How long a su call waits for the host before it fails, matching the host's own limit.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);

/// The kernel su guard's control file, one command per line.
pub const CONTROL_FILE: &str = "/sys/kernel/security/dsa_su/control";

/// Where the package manager lists each installed package with its uid.
pub const PACKAGES_LIST: &str = "/data/system/packages.list";

/// The paths the guard watches: where the image's su can be.
pub const SU_PATHS: [&str; 3] = ["/system/bin/su", "/system/xbin/su", "/sbin/su"];

/// A su invocation waiting for the host's answer.
#[derive(Debug)]
pub struct Pending {
    pub request_id: u64,
    pub package: String,
    pub uid: u32,
    started: Instant,
}

/// What the host decided for a su call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Approved,
    Refused(String),
}

/// The lines the kernel's `/sys/kernel/security/dsa_su/control` file takes for one config.
///
/// The guard always watches su's paths. With the feature on it is enabled, and lets the
/// uids of the ask and remember apps see su; with it off it is disabled, which hides su from
/// every uid. `reset` comes first, so a uid left out of a later config no longer sees su. The
/// daemon writes the lines in one write, which the kernel applies as one change.
pub fn control_lines(config: &RootElevationConfig) -> Vec<String> {
    let mut lines = vec!["reset".to_string()];
    lines.extend(SU_PATHS.iter().map(|path| format!("path {path}")));
    for uid in allowed_uids(config) {
        lines.push(format!("allow {uid}"));
    }
    lines.push(if config.enabled { "enable" } else { "disable" }.to_string());
    lines
}

/// The uids that may see su under `config`: the ask and remember apps while the feature is
/// on, none while it is off, and never uid 0 (an app the package list did not name).
fn allowed_uids(config: &RootElevationConfig) -> Vec<u32> {
    if !config.enabled {
        return Vec::new();
    }
    let mut uids: Vec<u32> = config
        .apps
        .iter()
        .filter(|app| app.uid != 0)
        .filter(|app| app.level() == RootLevel::Ask || app.level() == RootLevel::Remember)
        .map(|app| app.uid)
        .collect();
    uids.sort_unstable();
    uids.dedup();
    uids
}

/// Each package's uid from the package manager's list (`/data/system/packages.list`), whose
/// lines start with the package name and its uid.
pub fn uids_from_packages_list(text: &str) -> HashMap<String, u32> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let package = fields.next()?;
            let uid = fields.next()?.parse::<u32>().ok()?;
            Some((package.to_string(), uid))
        })
        .collect()
}

/// `config` with each app's uid filled in from `uids` where the host left it 0: the host
/// knows an app by its package, the kernel by its uid. An app the list does not name keeps
/// uid 0 and so sees no su.
pub fn with_uids(config: &RootElevationConfig, uids: &HashMap<String, u32>) -> RootElevationConfig {
    let mut resolved = config.clone();
    for app in &mut resolved.apps {
        if app.uid == 0 {
            if let Some(uid) = uids.get(&app.package) {
                app.uid = *uid;
            }
        }
    }
    resolved
}

/// The relay's bookkeeping: the next request id and the calls waiting for the host.
pub struct SuRelay {
    next_id: AtomicU64,
    waiting: Mutex<HashMap<u64, WaitingCall>>,
    /// The host's config as last applied, with uids filled in. Off until the host sends one.
    config: Mutex<RootElevationConfig>,
}

struct WaitingCall {
    pending: Pending,
    answer: Sender<Outcome>,
}

impl Default for SuRelay {
    fn default() -> SuRelay {
        SuRelay::new()
    }
}

impl SuRelay {
    pub fn new() -> SuRelay {
        SuRelay {
            next_id: AtomicU64::new(1),
            waiting: Mutex::new(HashMap::new()),
            config: Mutex::new(RootElevationConfig::default()),
        }
    }

    /// Takes `config` (uids already filled in) as the relay's current config and returns the
    /// lines to write to the kernel's control file for it. Every su call waiting for the host
    /// whose app may no longer ask (the feature turned off, or the app set to off) is refused
    /// at once.
    pub fn apply_config(&self, config: &RootElevationConfig) -> Vec<String> {
        let allowed: HashSet<u32> = allowed_uids(config).into_iter().collect();
        *self.config.lock().unwrap_or_else(|e| e.into_inner()) = config.clone();
        let lines = control_lines(config);
        let refused: Vec<WaitingCall> = {
            let mut waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
            let gone: Vec<u64> = waiting
                .iter()
                .filter(|(_, call)| !allowed.contains(&call.pending.uid))
                .map(|(id, _)| *id)
                .collect();
            gone.into_iter()
                .filter_map(|id| waiting.remove(&id))
                .collect()
        };
        for call in refused {
            let _ = call.answer.send(Outcome::Refused(format!(
                "{} may no longer ask for root: the computer's administrator changed its setting",
                call.pending.package
            )));
        }
        lines
    }

    /// Whether the app with `uid` may ask for root under the config last applied.
    pub fn allows(&self, uid: u32) -> bool {
        let config = self.config.lock().unwrap_or_else(|e| e.into_inner());
        allowed_uids(&config).contains(&uid)
    }

    /// The config last applied, with uids filled in.
    pub fn config(&self) -> RootElevationConfig {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Registers a waiting su call and returns the frame to send the host and the receiver
    /// the caller blocks on. The caller sends the frame, then waits on the receiver up to
    /// [`ANSWER_TIMEOUT`]; on timeout it calls [`SuRelay::expire`].
    pub fn begin(
        &self,
        package: &str,
        uid: u32,
        command: Vec<String>,
        interactive: bool,
    ) -> (u64, Vec<u8>, std::sync::mpsc::Receiver<Outcome>) {
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (answer, wait) = std::sync::mpsc::channel();
        let pending = Pending {
            request_id,
            package: package.to_string(),
            uid,
            started: Instant::now(),
        };
        self.waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(request_id, WaitingCall { pending, answer });
        let frame = encode(Body::SuRequest(SuRequest {
            request_id,
            package: package.to_string(),
            uid,
            command,
            interactive,
        }));
        (request_id, frame, wait)
    }

    /// Delivers a host decision to the waiting call, if any is still waiting. Returns the
    /// request it answered, for the log.
    pub fn decide(&self, decision: &SuDecision) -> Option<Pending> {
        let call = self
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&decision.request_id)?;
        let outcome = if decision.approved {
            Outcome::Approved
        } else {
            Outcome::Refused(if decision.reason.is_empty() {
                "the computer's administrator refused the root request".to_string()
            } else {
                decision.reason.clone()
            })
        };
        let _ = call.answer.send(outcome);
        Some(call.pending)
    }

    /// Fails a call that has waited past [`ANSWER_TIMEOUT`], freeing its slot. Returns true if
    /// the call was still waiting.
    pub fn expire(&self, request_id: u64) -> bool {
        if let Some(call) = self
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&request_id)
        {
            let _ = call.answer.send(Outcome::Refused(
                "the root request timed out waiting for the computer's administrator".to_string(),
            ));
            return true;
        }
        false
    }

    /// How long the call has waited, for the timeout check.
    pub fn waited(&self, request_id: u64) -> Option<Duration> {
        self.waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&request_id)
            .map(|call| call.pending.started.elapsed())
    }
}

/// Where the commands for the kernel's su guard go.
pub trait GuardControl: Send + Sync {
    /// Writes `lines` to the guard in one write, which the kernel applies as one change: all
    /// of it, or (on an error) none of it. A watched path that does not exist is skipped.
    fn write(&self, lines: &[String]) -> std::io::Result<()>;
}

/// The kernel's own control file.
pub struct KernelGuard {
    pub path: PathBuf,
}

impl Default for KernelGuard {
    fn default() -> KernelGuard {
        KernelGuard {
            path: PathBuf::from(CONTROL_FILE),
        }
    }
}

impl GuardControl for KernelGuard {
    fn write(&self, lines: &[String]) -> std::io::Result<()> {
        // One write, so su is shown to nobody between the reset and the new config's uids.
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::OpenOptions::new()
            .write(true)
            .open(&self.path)?
            .write_all(text.as_bytes())
    }
}

/// Reads the package manager's list of packages and their uids; `None` when it cannot.
pub type PackagesList = Box<dyn Fn() -> Option<String> + Send + Sync>;

/// Applies each root elevation config the host sends to the running Android: fills in the
/// apps' uids, updates the relay, and writes the kernel's su guard.
pub struct ConfigApplier {
    relay: Arc<SuRelay>,
    guard: Box<dyn GuardControl>,
    packages: PackagesList,
    say: Box<dyn Fn(&str) + Send + Sync>,
}

impl ConfigApplier {
    pub fn new(
        relay: Arc<SuRelay>,
        guard: Box<dyn GuardControl>,
        packages: PackagesList,
        say: impl Fn(&str) + Send + Sync + 'static,
    ) -> ConfigApplier {
        ConfigApplier {
            relay,
            guard,
            packages,
            say: Box::new(say),
        }
    }

    /// The applier for this guest: the kernel's control file and the package manager's list.
    pub fn of_system(
        relay: Arc<SuRelay>,
        say: impl Fn(&str) + Send + Sync + 'static,
    ) -> ConfigApplier {
        ConfigApplier::new(
            relay,
            Box::new(KernelGuard::default()),
            Box::new(|| std::fs::read_to_string(PACKAGES_LIST).ok()),
            say,
        )
    }

    /// Applies `config` and returns the commands written to the guard.
    pub fn apply(&self, config: &RootElevationConfig) -> Vec<String> {
        let uids = if config.apps.iter().any(|app| app.uid == 0) {
            match (self.packages)() {
                Some(text) => uids_from_packages_list(&text),
                None => {
                    (self.say)(&format!(
                        "root elevation: cannot read {PACKAGES_LIST}, so the apps the host \
                         named see no su"
                    ));
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };
        let resolved = with_uids(config, &uids);
        for app in resolved.apps.iter().filter(|app| app.uid == 0) {
            (self.say)(&format!(
                "root elevation: {} is not installed, so it sees no su",
                app.package
            ));
        }
        let lines = self.relay.apply_config(&resolved);
        match self.guard.write(&lines) {
            Ok(()) => (self.say)(&format!(
                "root elevation: {}, {} app(s) may ask for root",
                if resolved.enabled { "on" } else { "off" },
                allowed_uids(&resolved).len()
            )),
            Err(error) => (self.say)(&format!(
                "root elevation: the kernel's su guard did not take the config ({error}), so \
                 it stays as it was"
            )),
        }
        lines
    }

    /// Hides su from every app, as the daemon does when it starts: until the host says
    /// otherwise, root elevation is off, whatever the image holds.
    pub fn hide_all(&self) -> Vec<String> {
        self.apply(&RootElevationConfig::default())
    }
}

fn encode(body: Body) -> Vec<u8> {
    dsa_protocol::encode(&Envelope { body: Some(body) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsa_protocol::control::RootApp;

    fn config(enabled: bool, apps: Vec<(&str, u32, RootLevel)>) -> RootElevationConfig {
        RootElevationConfig {
            enabled,
            apps: apps
                .into_iter()
                .map(|(package, uid, level)| RootApp {
                    package: package.to_string(),
                    uid,
                    level: level.into(),
                })
                .collect(),
        }
    }

    #[test]
    fn fr35_a_disabled_config_hides_su_from_every_uid() {
        let lines = control_lines(&config(false, vec![("com.x", 10100, RootLevel::Ask)]));
        assert_eq!(lines.first().unwrap(), "reset");
        assert_eq!(lines.last().unwrap(), "disable");
        assert!(lines.contains(&"path /system/bin/su".to_string()));
        assert!(!lines.iter().any(|line| line.starts_with("allow ")));
    }

    #[test]
    fn fr35_an_enabled_config_allows_only_ask_and_remember_uids() {
        let lines = control_lines(&config(
            true,
            vec![
                ("com.ask", 10100, RootLevel::Ask),
                ("com.off", 10101, RootLevel::Off),
                ("com.remember", 10102, RootLevel::Remember),
            ],
        ));
        assert!(lines.contains(&"enable".to_string()));
        assert!(lines.contains(&"path /system/bin/su".to_string()));
        assert!(lines.contains(&"allow 10100".to_string()));
        assert!(lines.contains(&"allow 10102".to_string()));
        assert!(!lines.contains(&"allow 10101".to_string()));
        // reset comes first, enable last.
        assert_eq!(lines.first().unwrap(), "reset");
        assert_eq!(lines.last().unwrap(), "enable");
    }

    #[test]
    fn fr35_a_decision_answers_the_waiting_call() {
        let relay = SuRelay::new();
        let (id, _frame, wait) = relay.begin("com.ask", 10100, vec!["id".into()], false);
        assert!(relay
            .decide(&SuDecision {
                request_id: id,
                approved: true,
                reason: String::new(),
            })
            .is_some());
        assert_eq!(wait.recv().unwrap(), Outcome::Approved);
        // The call is no longer waiting.
        assert!(relay.waited(id).is_none());
    }

    #[test]
    fn fr35_a_refusal_carries_a_reason() {
        let relay = SuRelay::new();
        let (id, _frame, wait) = relay.begin("com.ask", 10100, vec![], true);
        relay.decide(&SuDecision {
            request_id: id,
            approved: false,
            reason: "refused".into(),
        });
        assert_eq!(wait.recv().unwrap(), Outcome::Refused("refused".into()));
    }

    #[test]
    fn fr35_a_timed_out_call_fails_and_frees_its_slot() {
        let relay = SuRelay::new();
        let (id, _frame, wait) = relay.begin("com.ask", 10100, vec![], false);
        assert!(relay.expire(id));
        match wait.recv().unwrap() {
            Outcome::Refused(why) => assert!(why.contains("timed out")),
            other => panic!("expected a timeout refusal, got {other:?}"),
        }
        // A later decision for the same id finds nothing.
        assert!(relay
            .decide(&SuDecision {
                request_id: id,
                approved: true,
                reason: String::new(),
            })
            .is_none());
    }

    #[test]
    fn fr35_request_ids_do_not_repeat() {
        let relay = SuRelay::new();
        let (a, _, _wa) = relay.begin("com.a", 10100, vec![], false);
        let (b, _, _wb) = relay.begin("com.b", 10101, vec![], false);
        assert_ne!(a, b);
    }

    #[test]
    fn fr35_uids_come_from_the_package_list() {
        let uids = uids_from_packages_list(
            "com.ask 10100 0 /data/user/0/com.ask platform:privapp 3003\n\
             com.other 10101 1 /data/user/0/com.other default none\n\
             broken\n",
        );
        assert_eq!(uids.get("com.ask"), Some(&10100));
        assert_eq!(uids.get("com.other"), Some(&10101));
        assert_eq!(uids.len(), 2);
        let resolved = with_uids(
            &config(
                true,
                vec![
                    ("com.ask", 0, RootLevel::Ask),
                    ("com.gone", 0, RootLevel::Ask),
                ],
            ),
            &uids,
        );
        assert_eq!(resolved.apps[0].uid, 10100);
        // An app that is not installed keeps uid 0, which never sees su.
        assert_eq!(resolved.apps[1].uid, 0);
        assert!(!control_lines(&resolved).contains(&"allow 0".to_string()));
    }

    #[test]
    fn fr35_a_new_config_replaces_the_ledger() {
        let relay = SuRelay::new();
        relay.apply_config(&config(
            true,
            vec![
                ("com.a", 10100, RootLevel::Ask),
                ("com.b", 10101, RootLevel::Remember),
            ],
        ));
        assert!(relay.allows(10100));
        assert!(relay.allows(10101));
        let (_, _, wait_a) = relay.begin("com.a", 10100, vec![], false);
        let (id_b, _, _wait_b) = relay.begin("com.b", 10101, vec![], false);

        // com.a is set to off while its call waits: the call is refused at once, and com.a no
        // longer may ask. com.b's call keeps waiting.
        relay.apply_config(&config(
            true,
            vec![
                ("com.a", 10100, RootLevel::Off),
                ("com.b", 10101, RootLevel::Remember),
            ],
        ));
        assert!(!relay.allows(10100));
        assert!(relay.allows(10101));
        match wait_a.recv().unwrap() {
            Outcome::Refused(why) => assert!(why.contains("com.a")),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(relay.waited(id_b).is_some());

        // The feature turned off refuses every waiting call.
        relay.apply_config(&config(false, vec![("com.b", 10101, RootLevel::Remember)]));
        assert!(!relay.allows(10101));
        assert!(relay.waited(id_b).is_none());
    }
}
