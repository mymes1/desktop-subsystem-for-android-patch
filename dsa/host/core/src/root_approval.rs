//! Answering an Android app's su call on the host.
//!
//! The guest's su relay sends a [`control::SuRequest`] for each su call and waits up to
//! [`ANSWER_TIMEOUT`] for the [`control::SuDecision`]. [`RootApproval`] makes that decision for
//! one user's Android: it asks the [`Policy`] first, and only when the policy says the host's
//! administrator has to approve does it show the platform's administrator prompt
//! ([`ApprovalPrompt`]: UAC on Windows, Authorization Services on macOS, polkit on Linux). An
//! approval starts the app's grace, so the app's next su call within it is not asked again.
//!
//! Every request and every decision goes to the host log with the app, the time, the command
//! and the outcome, so the computer's owner can read back what ran as root.
//!
//! The policy is rebuilt from the settings at each start of Android ([`RootApproval::restart`]),
//! which clears every grace. Between starts the per-app levels are read again at each request,
//! so a level changed in the settings applies to the next su call and clears that app's grace;
//! turning the whole feature off stops every approval at once, while turning it on applies only
//! from the next start, as the guest's su is set up at boot.

use std::sync::{Arc, Mutex, TryLockError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use dsa_protocol::control;

use crate::resize_trace::utc_stamp;
use crate::root_elevation::{AppLevel, Decision, DecisionDenied, Level, Policy, ANSWER_TIMEOUT};
use crate::settings::SettingsStore;
use crate::tasks::AppInfo;

/// How long the host's administrator prompt may stay open. Shorter than the guest's
/// [`ANSWER_TIMEOUT`], so the host's "timed out" reaches su (and the log) before su gives up
/// on its own.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(55);

/// How often a request that waits for another app's prompt to close looks again.
const TURN_POLL: Duration = Duration::from_millis(50);

/// One su call the host's administrator is asked to approve, with what the prompt shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRequest {
    pub request_id: u64,
    /// The calling app, with its name and icon as the guest last listed them. An app the guest
    /// has not listed has its package as its name and no icon.
    pub app: AppInfo,
    pub uid: u32,
    /// The command su runs, split as a shell would; empty for an interactive shell.
    pub command: Vec<String>,
    pub interactive: bool,
}

impl ApprovalRequest {
    /// The app's name for the prompt and the log: its label, else its package.
    pub fn app_name(&self) -> &str {
        let label = self.app.label.trim();
        if label.is_empty() {
            &self.app.package_name
        } else {
            label
        }
    }

    /// What su runs, in words for the prompt and the log.
    pub fn command_text(&self) -> String {
        command_text(&self.command, self.interactive)
    }
}

/// What su runs, in words: the command with each word that needs it quoted, or "an
/// interactive shell" when there is no command.
pub fn command_text(command: &[String], interactive: bool) -> String {
    if command.is_empty() {
        return if interactive {
            "an interactive shell".to_string()
        } else {
            "a shell".to_string()
        };
    }
    command
        .iter()
        .map(|word| {
            let plain = !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
            if plain {
                word.clone()
            } else {
                format!("'{}'", word.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// How the host's administrator answered a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAnswer {
    /// An administrator approved: the platform confirmed the answer came with administrator
    /// rights (an elevated process, an authorization right, a polkit decision).
    Approved,
    /// The prompt was refused or cancelled.
    Refused,
    /// The person at the computer is not one of its administrators, and no administrator
    /// answered for them.
    NotAdministrator,
    /// Nobody answered within the time the prompt had.
    TimedOut,
    /// The prompt could not be shown, or its answer could not be read. Counts as a refusal.
    Failed(String),
}

/// The platform's administrator prompt for one user's su calls.
pub trait ApprovalPrompt: Send + Sync {
    /// Shows the prompt for `request` in the user's session, naming the app and showing its
    /// icon, and waits at most `timeout` for the answer. It must not approve on anything less
    /// than an answer made with administrator rights.
    fn ask(&self, request: &ApprovalRequest, timeout: Duration) -> PromptAnswer;
}

/// Where the decisions are written: the host log.
pub type LogLine = Arc<dyn Fn(&str) + Send + Sync>;

/// The host log of the service: its standard error.
pub fn service_log() -> LogLine {
    Arc::new(|line: &str| eprintln!("{line}"))
}

/// The policy for the running boot, and whether the boot was started with the feature on.
struct BootPolicy {
    booted_enabled: bool,
    policy: Policy,
}

/// Decides each su call of one user's Android.
pub struct RootApproval {
    user: String,
    store: Arc<dyn SettingsStore>,
    prompt: Arc<dyn ApprovalPrompt>,
    log: LogLine,
    timeout: Duration,
    boot: Mutex<BootPolicy>,
    /// Held while a prompt is open, so the user sees one prompt at a time and a second call
    /// from the same app finds the grace the first one's approval started.
    turn: Mutex<()>,
}

impl RootApproval {
    /// Decides `user`'s su calls by the settings in `store`, asking through `prompt`, and
    /// writes each decision with `log`. The policy is that of a boot started now.
    pub fn new(
        user: &str,
        store: Arc<dyn SettingsStore>,
        prompt: Arc<dyn ApprovalPrompt>,
        log: LogLine,
    ) -> RootApproval {
        let approval = RootApproval {
            user: user.to_string(),
            store,
            prompt,
            log,
            timeout: PROMPT_TIMEOUT,
            boot: Mutex::new(BootPolicy {
                booted_enabled: false,
                policy: Policy::new(),
            }),
            turn: Mutex::new(()),
        };
        approval.restart();
        approval
    }

    /// Gives the prompt `timeout` instead of [`PROMPT_TIMEOUT`].
    pub fn with_timeout(mut self, timeout: Duration) -> RootApproval {
        self.timeout = timeout;
        self
    }

    /// Starts the policy of a new boot of Android: the feature as the settings say now, the
    /// stored levels, and no grace for any app.
    pub fn restart(&self) {
        let enabled = self.store.machine().root_elevation;
        let levels = self.stored_levels();
        *self.boot.lock().unwrap_or_else(|e| e.into_inner()) = BootPolicy {
            booted_enabled: enabled,
            policy: Policy::with_levels(enabled, levels),
        };
    }

    fn stored_levels(&self) -> Vec<AppLevel> {
        match self.store.root_apps(&self.user) {
            Ok(apps) => apps
                .into_iter()
                .filter_map(|app| {
                    Level::from_code(&app.level).map(|level| AppLevel {
                        package: app.package,
                        level,
                    })
                })
                .collect(),
            Err(why) => {
                (self.log)(&format!(
                    "root: {}: the apps' root levels cannot be read, so every app is off: {why}",
                    self.user
                ));
                Vec::new()
            }
        }
    }

    /// Brings the boot's policy up to the settings for `package`: the feature turned off since
    /// the boot stops approvals, and a level changed since the last request applies (and
    /// clears the app's grace). Returns the decision at `now`.
    fn decide_now(&self, package: &str, now: Instant) -> Decision {
        let enabled = self.store.machine().root_elevation;
        let stored = self
            .stored_levels()
            .into_iter()
            .find(|app| app.package == package)
            .map_or(Level::Off, |app| app.level);
        let mut boot = self.boot.lock().unwrap_or_else(|e| e.into_inner());
        let enabled = boot.booted_enabled && enabled;
        if boot.policy.enabled() != enabled {
            boot.policy.set_enabled(enabled);
        }
        if boot.policy.level(package) != stored {
            boot.policy.set_level(package, stored);
        }
        boot.policy.decide(package, now)
    }

    /// Decides one su call. `app` is the calling app as the guest last listed it, for the name
    /// and icon the prompt shows. Blocks while the prompt is open, at most the prompt's time.
    pub fn decide(
        &self,
        request: &control::SuRequest,
        app: Option<&AppInfo>,
    ) -> control::SuDecision {
        let received = Instant::now();
        let deadline = received + self.timeout;
        let asking = ApprovalRequest {
            request_id: request.request_id,
            app: app.cloned().unwrap_or_else(|| AppInfo {
                package_name: request.package.clone(),
                label: request.package.clone(),
                ..Default::default()
            }),
            uid: request.uid,
            command: request.command.clone(),
            interactive: request.interactive,
        };
        self.write(
            &asking,
            &format!(
                "asks to run {} as root (request {})",
                asking.command_text(),
                asking.request_id
            ),
        );
        let package = request.package.as_str();
        let first = self.decide_now(package, received);
        let outcome = match first {
            Decision::Deny => Err(DecisionDenied::NotAllowed.describe().to_string()),
            Decision::ApproveWithoutPrompt => Ok(self.approve_without_prompt(package, received)),
            Decision::PromptHost => self.ask(&asking, deadline),
        };
        match outcome {
            Ok(how) => {
                self.write(
                    &asking,
                    &format!("approved {how} (request {})", asking.request_id),
                );
                control::SuDecision {
                    request_id: request.request_id,
                    approved: true,
                    reason: String::new(),
                }
            }
            Err(reason) => {
                self.write(
                    &asking,
                    &format!("refused (request {}): {reason}", asking.request_id),
                );
                control::SuDecision {
                    request_id: request.request_id,
                    approved: false,
                    reason,
                }
            }
        }
    }

    /// A call the policy approves at once: a remember app, which also renews its standing
    /// state, or an app within its grace. Says how, for the log.
    fn approve_without_prompt(&self, package: &str, now: Instant) -> &'static str {
        let mut boot = self.boot.lock().unwrap_or_else(|e| e.into_inner());
        if boot.policy.level(package) == Level::Remember {
            boot.policy.approved(package, now);
            "without asking (the app may keep root)"
        } else {
            "without asking (within the app's 15-minute grace)"
        }
    }

    /// Waits for the prompt's turn, decides again (an approval in another prompt may have
    /// started the app's grace meanwhile), and asks the administrator. `Ok` says how it was
    /// approved; `Err` says why not, in words su prints.
    fn ask(&self, asking: &ApprovalRequest, deadline: Instant) -> Result<&'static str, String> {
        let package = asking.app.package_name.as_str();
        let _turn = loop {
            match self.turn.try_lock() {
                Ok(turn) => break turn,
                Err(TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(DecisionDenied::TimedOut.describe().to_string());
                    }
                    thread::sleep(TURN_POLL);
                }
            }
        };
        match self.decide_now(package, Instant::now()) {
            Decision::Deny => return Err(DecisionDenied::NotAllowed.describe().to_string()),
            Decision::ApproveWithoutPrompt => {
                return Ok(self.approve_without_prompt(package, Instant::now()))
            }
            Decision::PromptHost => {}
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(DecisionDenied::TimedOut.describe().to_string());
        }
        self.write(
            asking,
            &format!(
                "asking the computer's administrator (request {})",
                asking.request_id
            ),
        );
        let answer = self.prompt.ask(asking, left);
        // An answer that comes after su gave up runs nothing, so it starts no grace either.
        let answer = if answer == PromptAnswer::Approved && Instant::now() > deadline {
            PromptAnswer::TimedOut
        } else {
            answer
        };
        match answer {
            PromptAnswer::Approved => {
                let now = Instant::now();
                // The feature or the level may have changed while the prompt was open.
                match self.decide_now(package, now) {
                    Decision::Deny => Err(DecisionDenied::NotAllowed.describe().to_string()),
                    _ => {
                        self.boot
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .policy
                            .approved(package, now);
                        Ok("by the computer's administrator")
                    }
                }
            }
            PromptAnswer::Refused => Err(DecisionDenied::Refused.describe().to_string()),
            PromptAnswer::NotAdministrator => Err(
                "the root request needs one of the computer's administrators, and you are not \
                 one"
                .to_string(),
            ),
            PromptAnswer::TimedOut => Err(DecisionDenied::TimedOut.describe().to_string()),
            PromptAnswer::Failed(why) => Err(format!(
                "the computer's administrator could not be asked: {why}"
            )),
        }
    }

    /// One line in the host log about `asking`, with the time, the user and the app.
    fn write(&self, asking: &ApprovalRequest, what: &str) {
        let since_epoch = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        (self.log)(&format!(
            "root: {} {}: {} ({}, uid {}) {what}",
            utc_stamp(since_epoch),
            self.user,
            asking.app_name(),
            asking.app.package_name,
            asking.uid
        ));
    }
}
