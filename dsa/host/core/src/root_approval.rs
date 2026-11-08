//! The host's answer to a su request: the policy's decision, and the administrator prompt
//! when the policy asks for one.
//!
//! [`crate::root_elevation`] says what a request decides to. This module carries it out: a
//! request the policy approves or denies is answered at once, and one that needs the host's
//! administrator goes to the platform's prompt ([`AdminPrompt`]: UAC on Windows, Authorization
//! Services on macOS, polkit on Linux). The prompt has [`PROMPT_TIMEOUT`] to answer; past it
//! the request has timed out, and the prompt is withdrawn where the platform can withdraw it.
//! Prompts go one at a time, so a second request from an app waits for the first to be
//! answered, and is then approved within the grace that answer started.
//!
//! Every decision is logged with the app, the time, the result and the command, whatever
//! decided it.
//!
//! The prompt is the only part that names a host OS API, so everything here runs and is
//! tested on every platform with a fake prompt.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dsa_protocol::control;

use crate::root_elevation::{AppLevel, Decision, DecisionDenied, Level, Policy};
use crate::settings::SettingsStore;

/// How long the administrator prompt may take before the request has timed out. Shorter than
/// [`ANSWER_TIMEOUT`](crate::root_elevation::ANSWER_TIMEOUT), which the guest waits, so the host's own answer (timed out, and why)
/// reaches su before su gives up on its own.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(55);

/// What the administrator prompt is asked about: one su request, with the requesting app's
/// name and icon as the guest last listed them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApprovalRequest {
    /// The guest's ID of the request, unique while its daemon runs.
    pub request_id: u64,
    /// The requesting app's package.
    pub package: String,
    /// The app's name, as a launcher shows it. Empty when the guest has not named it.
    pub app_label: String,
    /// The app's launcher icon as a PNG image. Empty when the guest has not drawn one.
    pub icon_png: Vec<u8>,
    /// The app's Android uid.
    pub uid: u32,
    /// The command to run as root, split as a shell would. Empty for an interactive shell.
    pub command: Vec<String>,
    /// Whether su has a terminal (an interactive shell).
    pub interactive: bool,
}

impl ApprovalRequest {
    /// The request a su call makes, with the app's name and icon.
    pub fn from_su(request: &control::SuRequest, app_label: &str, icon_png: &[u8]) -> Self {
        ApprovalRequest {
            request_id: request.request_id,
            package: request.package.clone(),
            app_label: app_label.to_string(),
            icon_png: icon_png.to_vec(),
            uid: request.uid,
            command: request.command.clone(),
            interactive: request.interactive,
        }
    }

    /// The app's name for the prompt: its label, else its package.
    pub fn app_name(&self) -> &str {
        if self.app_label.trim().is_empty() {
            &self.package
        } else {
            self.app_label.trim()
        }
    }

    /// What the app wants to run as root, in words: the command line, or a root shell.
    pub fn what(&self) -> String {
        if self.command.is_empty() {
            "a root shell".to_string()
        } else {
            format!("`{}`", self.command_line())
        }
    }

    /// The command as one line, each word quoted when it needs to be, as a shell would read
    /// it back. Empty for an interactive shell.
    pub fn command_line(&self) -> String {
        self.command
            .iter()
            .map(|word| quote_word(word))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The sentence the administrator prompt shows: the app by name and package, and what it
    /// wants to run.
    pub fn message(&self) -> String {
        let app = if self.app_name() == self.package {
            format!("The Android app {}", self.package)
        } else {
            format!("The Android app {} ({})", self.app_name(), self.package)
        };
        if self.command.is_empty() {
            format!("{app} wants to open a root shell.")
        } else {
            format!("{app} wants to run {} as root.", self.what())
        }
    }
}

/// A word quoted for a shell when it holds anything but plain characters.
fn quote_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// What the platform's administrator prompt said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAnswer {
    /// An administrator approved.
    Approved,
    /// The prompt refused: wrong credentials, or the account may not approve.
    Denied,
    /// The person dismissed the prompt without answering.
    Cancelled,
    /// The prompt could not be shown; the detail says why (no prompt service, no session to
    /// show it in, an error from the system).
    Unavailable(String),
}

/// The host OS's administrator prompt. [`AdminPrompt::ask`] shows it and blocks until it is
/// answered. It runs on a thread of its own, so a prompt that never returns only costs that
/// thread.
pub trait AdminPrompt: Send + Sync {
    /// Asks the host's administrator about `request`, naming the app and showing its icon
    /// where the platform can, and returns the answer.
    fn ask(&self, request: &ApprovalRequest) -> PromptAnswer;

    /// Takes down the prompt for `request` after it timed out, where the platform can. Its
    /// later answer, if any, is ignored either way.
    fn withdraw(&self, request: &ApprovalRequest) {
        let _ = request;
    }
}

/// The end of one request to the administrator prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Approved,
    /// Refused by the prompt.
    Denied,
    /// Dismissed by the person: denied too.
    Cancelled,
    /// No answer within the prompt's time.
    TimedOut,
    /// The prompt could not be shown: denied, with the reason.
    Unavailable(String),
}

impl Outcome {
    /// The result the policy's caller gets: approved, or denied with the reason.
    pub fn result(&self) -> Result<(), DecisionDenied> {
        match self {
            Outcome::Approved => Ok(()),
            Outcome::Denied | Outcome::Cancelled | Outcome::Unavailable(_) => {
                Err(DecisionDenied::Refused)
            }
            Outcome::TimedOut => Err(DecisionDenied::TimedOut),
        }
    }

    /// The sentence su prints and the log keeps, empty when approved.
    pub fn reason(&self) -> String {
        match self {
            Outcome::Approved => String::new(),
            Outcome::Denied => DecisionDenied::Refused.describe().to_string(),
            Outcome::Cancelled => {
                "the computer's administrator dismissed the root request".to_string()
            }
            Outcome::TimedOut => DecisionDenied::TimedOut.describe().to_string(),
            Outcome::Unavailable(why) => {
                format!("the computer could not ask its administrator: {why}")
            }
        }
    }
}

/// Asks `prompt` about `request` and waits at most `timeout` for its answer. A prompt that
/// does not answer in time is withdrawn and the request has timed out; a prompt that fails
/// (its thread panics) leaves the request denied, and the host keeps running.
pub fn ask_admin(
    prompt: &Arc<dyn AdminPrompt>,
    request: &ApprovalRequest,
    timeout: Duration,
) -> Outcome {
    let (tx, rx) = mpsc::channel();
    let (asked, question) = (Arc::clone(prompt), request.clone());
    let spawned = thread::Builder::new()
        .name("dsa-admin-prompt".into())
        .spawn(move || {
            let _ = tx.send(asked.ask(&question));
        });
    if let Err(why) = spawned {
        return Outcome::Unavailable(format!("no thread for the prompt: {why}"));
    }
    match rx.recv_timeout(timeout) {
        Ok(PromptAnswer::Approved) => Outcome::Approved,
        Ok(PromptAnswer::Denied) => Outcome::Denied,
        Ok(PromptAnswer::Cancelled) => Outcome::Cancelled,
        Ok(PromptAnswer::Unavailable(why)) => Outcome::Unavailable(why),
        Err(RecvTimeoutError::Timeout) => {
            prompt.withdraw(request);
            Outcome::TimedOut
        }
        Err(RecvTimeoutError::Disconnected) => {
            Outcome::Unavailable("the prompt ended without an answer".to_string())
        }
    }
}

/// Says whether the feature is on and each app's level, as the settings have them now.
pub type LevelSource = Arc<dyn Fn() -> (bool, Vec<AppLevel>) + Send + Sync>;

/// The levels `user`'s settings in `store` hold now: whether the computer has the feature on,
/// and each of the user's apps with a level. A level the store cannot read is off.
pub fn settings_levels(store: &dyn SettingsStore, user: &str) -> (bool, Vec<AppLevel>) {
    let enabled = store.machine().root_elevation;
    let apps = store
        .root_apps(user)
        .unwrap_or_else(|why| {
            eprintln!("root: the root levels of {user} cannot be read, so every app is off: {why}");
            Vec::new()
        })
        .into_iter()
        .filter_map(|app| {
            Level::from_code(&app.level).map(|level| AppLevel {
                package: app.package,
                level,
            })
        })
        .collect();
    (enabled, apps)
}

/// A [`LevelSource`] that reads `user`'s levels from `store` at each request.
pub fn levels_from_settings(store: Arc<dyn SettingsStore>, user: &str) -> LevelSource {
    let user = user.to_string();
    Arc::new(move || settings_levels(&*store, &user))
}

/// Where the decisions are logged. The service's own log, by default.
pub type DecisionLog = Arc<dyn Fn(&str) + Send + Sync>;

/// The host's root elevation for one user's Android: the policy of the running boot, the
/// settings it follows, and the administrator prompt.
pub struct RootElevation {
    policy: Mutex<Policy>,
    levels: LevelSource,
    prompt: Arc<dyn AdminPrompt>,
    timeout: Duration,
    log: DecisionLog,
    /// Held while a prompt is up, so prompts go one at a time.
    turn: Mutex<()>,
}

impl RootElevation {
    /// Root elevation that follows `levels` and asks `prompt`, logging to the service's log.
    pub fn new(prompt: Arc<dyn AdminPrompt>, levels: LevelSource) -> RootElevation {
        let elevation = RootElevation {
            policy: Mutex::new(Policy::new()),
            levels,
            prompt,
            timeout: PROMPT_TIMEOUT,
            log: Arc::new(|line: &str| eprintln!("{line}")),
            turn: Mutex::new(()),
        };
        elevation.restart();
        elevation
    }

    /// The same, with the prompt given `timeout` to answer.
    pub fn with_timeout(mut self, timeout: Duration) -> RootElevation {
        self.timeout = timeout;
        self
    }

    /// The same, logging to `log`.
    pub fn with_log(mut self, log: DecisionLog) -> RootElevation {
        self.log = log;
        self
    }

    /// Starts afresh for a new boot of Android: every grace is gone.
    pub fn restart(&self) {
        let (enabled, levels) = (self.levels)();
        *self.policy.lock().unwrap_or_else(|e| e.into_inner()) =
            Policy::with_levels(enabled, levels);
    }

    /// Brings the policy in line with the settings now, keeping each grace whose app's level
    /// has not changed.
    fn follow_settings(&self, policy: &mut Policy) {
        let (enabled, levels) = (self.levels)();
        if policy.enabled() != enabled {
            policy.set_enabled(enabled);
        }
        for app in policy.app_levels() {
            if !levels.iter().any(|a| a.package == app.package) {
                policy.set_level(&app.package, Level::Off);
            }
        }
        for app in levels {
            if policy.level(&app.package) != app.level {
                policy.set_level(&app.package, app.level);
            }
        }
    }

    /// What the policy decides for `package` now, after following the settings.
    fn decide(&self, package: &str) -> (Decision, Level) {
        let mut policy = self.policy.lock().unwrap_or_else(|e| e.into_inner());
        self.follow_settings(&mut policy);
        (
            policy.decide(package, Instant::now()),
            policy.level(package),
        )
    }

    fn record_approval(&self, package: &str) {
        let mut policy = self.policy.lock().unwrap_or_else(|e| e.into_inner());
        policy.approved(package, Instant::now());
    }

    /// Answers one su request: by the policy, and by the administrator prompt when the policy
    /// asks for one. Logs the decision.
    pub fn answer(&self, request: &ApprovalRequest) -> control::SuDecision {
        let asked_at = Instant::now();
        let (decision, level) = self.decide(&request.package);
        let (approved, reason, how) = match decision {
            Decision::Deny => (
                false,
                DecisionDenied::NotAllowed.describe().to_string(),
                "denied: the app may not use root".to_string(),
            ),
            Decision::ApproveWithoutPrompt => {
                if level == Level::Remember {
                    self.record_approval(&request.package);
                    (
                        true,
                        String::new(),
                        "approved without asking (remember)".into(),
                    )
                } else {
                    (
                        true,
                        String::new(),
                        "approved within the grace of an earlier approval".into(),
                    )
                }
            }
            Decision::PromptHost => self.prompt(request, asked_at),
        };
        (self.log)(&format!(
            "root: {} {} ({}, uid {}) asked to run {}: {how}",
            now_utc(),
            request.app_name(),
            request.package,
            request.uid,
            request.what(),
        ));
        control::SuDecision {
            request_id: request.request_id,
            approved,
            reason,
        }
    }

    /// Asks the administrator, one prompt at a time. A request that waited for another
    /// prompt is decided again first, as that prompt's approval may now cover it.
    fn prompt(&self, request: &ApprovalRequest, asked_at: Instant) -> (bool, String, String) {
        let _turn = self.turn.lock().unwrap_or_else(|e| e.into_inner());
        let (decision, _) = self.decide(&request.package);
        match decision {
            Decision::Deny => {
                return (
                    false,
                    DecisionDenied::NotAllowed.describe().to_string(),
                    "denied: the app may not use root".to_string(),
                )
            }
            Decision::ApproveWithoutPrompt => {
                return (
                    true,
                    String::new(),
                    "approved within the grace of an earlier approval".to_string(),
                )
            }
            Decision::PromptHost => {}
        }
        let left = self.timeout.saturating_sub(asked_at.elapsed());
        let outcome = if left.is_zero() {
            Outcome::TimedOut
        } else {
            ask_admin(&self.prompt, request, left)
        };
        if outcome == Outcome::Approved {
            self.record_approval(&request.package);
        }
        let how = match &outcome {
            Outcome::Approved => "approved by the computer's administrator".to_string(),
            Outcome::Denied => "refused by the computer's administrator".to_string(),
            Outcome::Cancelled => "refused: the administrator prompt was dismissed".to_string(),
            Outcome::TimedOut => format!(
                "refused: no answer within {} seconds",
                self.timeout.as_secs()
            ),
            Outcome::Unavailable(why) => {
                format!("refused: the administrator prompt could not be shown ({why})")
            }
        };
        (outcome == Outcome::Approved, outcome.reason(), how)
    }
}

/// The time now in UTC, for the log.
fn now_utc() -> String {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{} UTC",
        crate::guest_log::utc_time(since.as_nanos() as u64)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A prompt that gives the same answer each time, after `delay`, and counts its prompts.
    struct FakePrompt {
        answer: PromptAnswer,
        delay: Duration,
        asked: AtomicUsize,
        withdrawn: AtomicUsize,
        seen: Mutex<Vec<ApprovalRequest>>,
    }

    impl FakePrompt {
        fn new(answer: PromptAnswer) -> Arc<FakePrompt> {
            FakePrompt::slow(answer, Duration::ZERO)
        }
        fn slow(answer: PromptAnswer, delay: Duration) -> Arc<FakePrompt> {
            Arc::new(FakePrompt {
                answer,
                delay,
                asked: AtomicUsize::new(0),
                withdrawn: AtomicUsize::new(0),
                seen: Mutex::new(Vec::new()),
            })
        }
    }

    impl AdminPrompt for FakePrompt {
        fn ask(&self, request: &ApprovalRequest) -> PromptAnswer {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push(request.clone());
            thread::sleep(self.delay);
            self.answer.clone()
        }
        fn withdraw(&self, _request: &ApprovalRequest) {
            self.withdrawn.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct PanickingPrompt;

    impl AdminPrompt for PanickingPrompt {
        fn ask(&self, _request: &ApprovalRequest) -> PromptAnswer {
            panic!("the prompt broke");
        }
    }

    fn levels(enabled: bool, apps: &[(&str, Level)]) -> LevelSource {
        let apps: Vec<AppLevel> = apps
            .iter()
            .map(|(package, level)| AppLevel {
                package: package.to_string(),
                level: *level,
            })
            .collect();
        Arc::new(move || (enabled, apps.clone()))
    }

    fn request(package: &str) -> ApprovalRequest {
        ApprovalRequest {
            request_id: 7,
            package: package.to_string(),
            app_label: "Root Checker".to_string(),
            icon_png: vec![0x89, b'P', b'N', b'G'],
            uid: 10_123,
            command: vec!["id".to_string()],
            interactive: false,
        }
    }

    fn logged() -> (DecisionLog, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        (
            Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_string())),
            lines,
        )
    }

    fn elevation(prompt: Arc<dyn AdminPrompt>, source: LevelSource) -> RootElevation {
        RootElevation::new(prompt, source)
    }

    #[test]
    fn fr35_each_prompt_answer_maps_to_approved_or_denied() {
        let cases = [
            (PromptAnswer::Approved, Outcome::Approved, Ok(())),
            (
                PromptAnswer::Denied,
                Outcome::Denied,
                Err(DecisionDenied::Refused),
            ),
            (
                PromptAnswer::Cancelled,
                Outcome::Cancelled,
                Err(DecisionDenied::Refused),
            ),
            (
                PromptAnswer::Unavailable("no polkit agent".into()),
                Outcome::Unavailable("no polkit agent".into()),
                Err(DecisionDenied::Refused),
            ),
        ];
        for (answer, outcome, result) in cases {
            let prompt: Arc<dyn AdminPrompt> = FakePrompt::new(answer);
            let got = ask_admin(&prompt, &request("com.ask.app"), Duration::from_secs(5));
            assert_eq!(got, outcome);
            assert_eq!(got.result(), result);
        }
    }

    #[test]
    fn fr35_a_prompt_that_does_not_answer_in_time_times_out_and_is_withdrawn() {
        let fake = FakePrompt::slow(PromptAnswer::Approved, Duration::from_secs(2));
        let prompt: Arc<dyn AdminPrompt> = fake.clone();
        let got = ask_admin(&prompt, &request("com.ask.app"), Duration::from_millis(50));
        assert_eq!(got, Outcome::TimedOut);
        assert_eq!(got.result(), Err(DecisionDenied::TimedOut));
        assert_eq!(fake.withdrawn.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fr35_a_prompt_that_breaks_denies_and_the_host_keeps_running() {
        let prompt: Arc<dyn AdminPrompt> = Arc::new(PanickingPrompt);
        let got = ask_admin(&prompt, &request("com.ask.app"), Duration::from_secs(5));
        assert!(matches!(got, Outcome::Unavailable(_)));
        assert_eq!(got.result(), Err(DecisionDenied::Refused));
    }

    #[test]
    fn fr35_an_ask_app_is_approved_by_the_prompt_and_then_within_grace() {
        let fake = FakePrompt::new(PromptAnswer::Approved);
        let (log, lines) = logged();
        let root =
            elevation(fake.clone(), levels(true, &[("com.ask.app", Level::Ask)])).with_log(log);
        let first = root.answer(&request("com.ask.app"));
        assert!(first.approved);
        assert_eq!(first.request_id, 7);
        assert!(first.reason.is_empty());
        let second = root.answer(&request("com.ask.app"));
        assert!(second.approved);
        assert_eq!(fake.asked.load(Ordering::SeqCst), 1);
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen[0].app_name(), "Root Checker");
        assert_eq!(
            seen[0].message(),
            "The Android app Root Checker (com.ask.app) wants to run `id` as root."
        );
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("Root Checker (com.ask.app, uid 10123)"));
        assert!(lines[0].contains("`id`"));
        assert!(lines[0].contains("approved by the computer's administrator"));
        assert!(lines[0].contains(" UTC "));
        assert!(lines[1].contains("within the grace"));
    }

    #[test]
    fn fr35_a_refused_dismissed_or_timed_out_prompt_fails_su_with_its_reason() {
        for (answer, delay, words) in [
            (PromptAnswer::Denied, Duration::ZERO, "refused"),
            (PromptAnswer::Cancelled, Duration::ZERO, "dismissed"),
            (PromptAnswer::Approved, Duration::from_secs(2), "timed out"),
        ] {
            let fake = FakePrompt::slow(answer, delay);
            let (log, lines) = logged();
            let root = elevation(fake.clone(), levels(true, &[("com.ask.app", Level::Ask)]))
                .with_timeout(Duration::from_millis(100))
                .with_log(log);
            let decision = root.answer(&request("com.ask.app"));
            assert!(!decision.approved);
            assert!(decision.reason.contains(words), "{}", decision.reason);
            assert!(lines.lock().unwrap()[0].contains("refused"));
            // No grace was started: the next request asks again.
            let _ = root.answer(&request("com.ask.app"));
            assert_eq!(fake.asked.load(Ordering::SeqCst), 2);
        }
    }

    #[test]
    fn fr35_remember_and_off_apps_never_reach_the_prompt() {
        let fake = FakePrompt::new(PromptAnswer::Denied);
        let (log, lines) = logged();
        let root = elevation(
            fake.clone(),
            levels(true, &[("com.remember.app", Level::Remember)]),
        )
        .with_log(log);
        assert!(root.answer(&request("com.remember.app")).approved);
        let off = root.answer(&request("com.other.app"));
        assert!(!off.approved);
        assert_eq!(off.reason, DecisionDenied::NotAllowed.describe());
        assert_eq!(fake.asked.load(Ordering::SeqCst), 0);
        let lines = lines.lock().unwrap();
        assert!(lines[0].contains("without asking"));
        assert!(lines[1].contains("may not use root"));
    }

    #[test]
    fn fr35_a_restart_clears_the_grace_and_asks_again() {
        let fake = FakePrompt::new(PromptAnswer::Approved);
        let root = elevation(fake.clone(), levels(true, &[("com.ask.app", Level::Ask)]))
            .with_log(Arc::new(|_: &str| {}));
        root.answer(&request("com.ask.app"));
        root.restart();
        root.answer(&request("com.ask.app"));
        assert_eq!(fake.asked.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn fr35_a_level_change_in_the_settings_clears_the_grace() {
        let fake = FakePrompt::new(PromptAnswer::Approved);
        let level = Arc::new(Mutex::new(Level::Ask));
        let current = Arc::clone(&level);
        let source: LevelSource = Arc::new(move || {
            (
                true,
                vec![AppLevel {
                    package: "com.ask.app".into(),
                    level: *current.lock().unwrap(),
                }],
            )
        });
        let root = elevation(fake.clone(), source).with_log(Arc::new(|_: &str| {}));
        root.answer(&request("com.ask.app"));
        *level.lock().unwrap() = Level::Off;
        assert!(!root.answer(&request("com.ask.app")).approved);
        *level.lock().unwrap() = Level::Ask;
        assert!(root.answer(&request("com.ask.app")).approved);
        assert_eq!(fake.asked.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn fr35_requests_that_arrive_together_get_one_prompt() {
        let fake = FakePrompt::slow(PromptAnswer::Approved, Duration::from_millis(200));
        let root = Arc::new(
            elevation(fake.clone(), levels(true, &[("com.ask.app", Level::Ask)]))
                .with_log(Arc::new(|_: &str| {})),
        );
        let others: Vec<_> = (0..3)
            .map(|_| {
                let root = Arc::clone(&root);
                thread::spawn(move || root.answer(&request("com.ask.app")).approved)
            })
            .collect();
        for other in others {
            assert!(other.join().unwrap());
        }
        assert_eq!(fake.asked.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fr35_the_levels_follow_the_computers_switch_and_the_users_apps() {
        use crate::settings::{MemorySettings, SettingsStore};
        let store = Arc::new(MemorySettings::default());
        store.set_root_app("ann", "com.ask.app", "ask").unwrap();
        store
            .set_root_app("ann", "com.keep.app", "remember")
            .unwrap();
        store.set_root_app("bob", "com.bob.app", "ask").unwrap();
        let source = levels_from_settings(store.clone(), "ann");
        let (enabled, apps) = source();
        assert!(!enabled);
        assert_eq!(apps.len(), 2);
        let mut machine = store.machine();
        machine.root_elevation = true;
        store.set_machine(machine).unwrap();
        let (enabled, apps) = source();
        assert!(enabled);
        assert!(apps.contains(&AppLevel {
            package: "com.keep.app".into(),
            level: Level::Remember,
        }));
        assert!(!apps.iter().any(|a| a.package == "com.bob.app"));
    }

    #[test]
    fn fr35_the_prompt_names_a_shell_and_quotes_the_command() {
        let mut shell = request("com.ask.app");
        shell.command.clear();
        shell.interactive = true;
        shell.app_label.clear();
        assert_eq!(
            shell.message(),
            "The Android app com.ask.app wants to open a root shell."
        );
        let mut quoted = request("com.ask.app");
        quoted.command = vec!["sh".into(), "-c".into(), "echo 'hi' > /x".into()];
        assert_eq!(quoted.command_line(), r"sh -c 'echo '\''hi'\'' > /x'");
    }
}
