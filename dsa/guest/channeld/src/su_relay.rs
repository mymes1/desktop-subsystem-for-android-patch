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
//!   lines the kernel's control file takes, and says whether the guard is on.
//! - [`SuRelay::begin`] registers a waiting su call and returns the frame to send the host.
//! - [`SuRelay::decide`] matches a host decision to its call.
//! - [`SuRelay::expire`] fails a call that waited past [`ANSWER_TIMEOUT`].
//!
//! The socket server that su connects to and the uid 0 child that runs the approved command
//! are the only parts that touch the system; this logic drives them. They are wired into the
//! daemon's main loop, which this module does not reach into.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use dsa_protocol::control::envelope::Body;
use dsa_protocol::control::{Envelope, RootElevationConfig, RootLevel, SuDecision, SuRequest};

/// How long a su call waits for the host before it fails, matching the host's own limit.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);

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
/// Empty means the guard is off: the daemon then writes a single `disable`.
pub fn control_lines(config: &RootElevationConfig) -> Vec<String> {
    if !config.enabled {
        return vec!["reset".to_string()];
    }
    let mut lines = vec![
        "reset".to_string(),
        "path /system/bin/su".to_string(),
        "path /system/xbin/su".to_string(),
        "path /sbin/su".to_string(),
    ];
    for app in &config.apps {
        // Only ask and remember apps may see su; off apps are left out.
        if app.level() == RootLevel::Ask || app.level() == RootLevel::Remember {
            lines.push(format!("allow {}", app.uid));
        }
    }
    lines.push("enable".to_string());
    lines
}

/// The relay's bookkeeping: the next request id and the calls waiting for the host.
pub struct SuRelay {
    next_id: AtomicU64,
    waiting: Mutex<HashMap<u64, WaitingCall>>,
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
        }
    }

    /// Writes the kernel's su guard for `config`: which uids may see su, and whether the
    /// guard is on. Returns the lines written, for the log and tests. The real write goes to
    /// `/sys/kernel/security/dsa_su/control`; a daemon that cannot open it (the guard not
    /// built in) logs and carries on with su hidden from all.
    pub fn apply_config(&self, config: &RootElevationConfig) -> Vec<String> {
        control_lines(config)
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
    fn fr35_a_disabled_config_resets_the_kernel_guard() {
        let lines = control_lines(&config(false, vec![("com.x", 10100, RootLevel::Ask)]));
        assert_eq!(lines, vec!["reset".to_string()]);
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
}
