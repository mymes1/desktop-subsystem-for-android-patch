//! Acceptance tests for the host's answer to an Android app's su call: the policy, the
//! administrator prompt behind a fake, and the host log.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::root_approval::{
    command_text, ApprovalPrompt, ApprovalRequest, LogLine, PromptAnswer, RootApproval,
};
use dsa_host_core::settings::{MachineSettings, MemorySettings, SettingsStore};
use dsa_host_core::tasks::AppInfo;
use dsa_protocol::control::SuRequest;

const USER: &str = "S-1-5-21-1-2-3-1001";
const ASK_APP: &str = "com.example.backup";

/// A prompt that answers what the test says, after an optional delay, and remembers what it
/// was asked.
#[derive(Default)]
struct FakePrompt {
    answers: Mutex<Vec<PromptAnswer>>,
    delay: Mutex<Duration>,
    asked: Mutex<Vec<(ApprovalRequest, Duration)>>,
}

impl FakePrompt {
    fn answering(answers: Vec<PromptAnswer>) -> Arc<FakePrompt> {
        Arc::new(FakePrompt {
            answers: Mutex::new(answers),
            ..Default::default()
        })
    }

    fn asked(&self) -> Vec<(ApprovalRequest, Duration)> {
        self.asked.lock().unwrap().clone()
    }
}

impl ApprovalPrompt for FakePrompt {
    fn ask(&self, request: &ApprovalRequest, timeout: Duration) -> PromptAnswer {
        self.asked.lock().unwrap().push((request.clone(), timeout));
        // The answer is taken when the prompt opens, so a prompt the host gave up on does not
        // take the answer meant for the next one.
        let answer = {
            let mut answers = self.answers.lock().unwrap();
            if answers.is_empty() {
                PromptAnswer::Unavailable("the test gave no answer".into())
            } else {
                answers.remove(0)
            }
        };
        let delay = *self.delay.lock().unwrap();
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        answer
    }
}

struct Fixture {
    store: Arc<MemorySettings>,
    prompt: Arc<FakePrompt>,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    /// The feature on, and `levels` set for the user.
    fn new(levels: &[(&str, &str)], answers: Vec<PromptAnswer>) -> Fixture {
        let store = Arc::new(MemorySettings::default());
        store
            .set_machine(MachineSettings {
                root_elevation: true,
                ..Default::default()
            })
            .unwrap();
        for (package, level) in levels {
            store.set_root_app(USER, package, level).unwrap();
        }
        Fixture {
            store,
            prompt: FakePrompt::answering(answers),
            lines: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn approval(&self) -> RootApproval {
        let lines = Arc::clone(&self.lines);
        let log: LogLine = Arc::new(move |line: &str| lines.lock().unwrap().push(line.into()));
        RootApproval::new(
            USER,
            Arc::clone(&self.store) as Arc<dyn SettingsStore>,
            Arc::clone(&self.prompt) as Arc<dyn ApprovalPrompt>,
            log,
        )
    }

    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

fn su(request_id: u64, package: &str, command: &[&str]) -> SuRequest {
    SuRequest {
        request_id,
        package: package.into(),
        uid: 10_123,
        command: command.iter().map(|word| word.to_string()).collect(),
        interactive: command.is_empty(),
    }
}

fn backup_app() -> AppInfo {
    AppInfo {
        package_name: ASK_APP.into(),
        label: "Backup".into(),
        icon_png: vec![0x89, b'P', b'N', b'G'],
        ..Default::default()
    }
}

#[test]
fn fr35_ask_app_request_reaches_the_prompt_with_its_name_and_icon() {
    let fixture = Fixture::new(&[(ASK_APP, "ask")], vec![PromptAnswer::Approved]);
    let approval = fixture.approval();
    let decision = approval.decide(&su(7, ASK_APP, &["id"]), Some(&backup_app()));

    assert!(decision.approved, "{decision:?}");
    assert_eq!(decision.request_id, 7);
    assert!(decision.reason.is_empty());
    let asked = fixture.prompt.asked();
    assert_eq!(asked.len(), 1);
    let (request, timeout) = &asked[0];
    assert_eq!(request.app_name(), "Backup");
    assert_eq!(request.app.icon_png, backup_app().icon_png);
    assert_eq!(request.command, vec!["id".to_string()]);
    assert_eq!(request.uid, 10_123);
    // The prompt is given no more than the host's limit, which ends before su's own.
    assert!(*timeout <= dsa_host_core::root_approval::PROMPT_TIMEOUT);
    assert!(
        dsa_host_core::root_approval::PROMPT_TIMEOUT
            < dsa_host_core::root_elevation::ANSWER_TIMEOUT
    );
}

#[test]
fn fr35_an_app_the_guest_has_not_listed_is_asked_for_by_its_package() {
    let fixture = Fixture::new(&[(ASK_APP, "ask")], vec![PromptAnswer::Approved]);
    let approval = fixture.approval();
    assert!(approval.decide(&su(1, ASK_APP, &["id"]), None).approved);
    assert_eq!(fixture.prompt.asked()[0].0.app_name(), ASK_APP);
}

#[test]
fn fr35_refused_prompt_fails_su_with_a_reason() {
    let fixture = Fixture::new(&[(ASK_APP, "ask")], vec![PromptAnswer::Denied]);
    let decision = fixture
        .approval()
        .decide(&su(2, ASK_APP, &["id"]), Some(&backup_app()));
    assert!(!decision.approved);
    assert!(decision.reason.contains("refused"), "{}", decision.reason);
}

#[test]
fn fr35_cancelled_or_failed_prompt_is_denied() {
    // A cancelled prompt comes back as a refusal; one that could not be shown as a failure.
    // Neither runs anything.
    let fixture = Fixture::new(
        &[(ASK_APP, "ask")],
        vec![
            PromptAnswer::Unavailable("no signed-in session".into()),
            PromptAnswer::NotAdministrator,
        ],
    );
    let approval = fixture.approval();
    let failed = approval.decide(&su(3, ASK_APP, &["id"]), Some(&backup_app()));
    assert!(!failed.approved);
    assert!(
        failed.reason.contains("no signed-in session"),
        "{}",
        failed.reason
    );
    let not_admin = approval.decide(&su(4, ASK_APP, &["id"]), Some(&backup_app()));
    assert!(!not_admin.approved);
    assert!(
        not_admin.reason.contains("administrators"),
        "{}",
        not_admin.reason
    );
    // Neither started a grace: the next call is asked again.
    assert_eq!(fixture.prompt.asked().len(), 2);
}

#[test]
fn fr35_unanswered_prompt_times_out() {
    let fixture = Fixture::new(&[(ASK_APP, "ask")], vec![PromptAnswer::TimedOut]);
    let decision = fixture
        .approval()
        .decide(&su(5, ASK_APP, &["id"]), Some(&backup_app()));
    assert!(!decision.approved);
    assert!(decision.reason.contains("timed out"), "{}", decision.reason);
}

#[test]
fn fr35_an_approval_after_the_limit_counts_as_timed_out_and_starts_no_grace() {
    let fixture = Fixture::new(
        &[(ASK_APP, "ask")],
        vec![PromptAnswer::Approved, PromptAnswer::Denied],
    );
    *fixture.prompt.delay.lock().unwrap() = Duration::from_millis(300);
    let approval = fixture.approval().with_timeout(Duration::from_millis(100));
    let late = approval.decide(&su(6, ASK_APP, &["id"]), Some(&backup_app()));
    assert!(!late.approved);
    assert!(late.reason.contains("timed out"), "{}", late.reason);
    // No grace: the next call is asked, and refused.
    *fixture.prompt.delay.lock().unwrap() = Duration::ZERO;
    let next = approval.decide(&su(7, ASK_APP, &["id"]), Some(&backup_app()));
    assert!(!next.approved);
    assert_eq!(fixture.prompt.asked().len(), 2);
}

#[test]
fn fr35_second_request_within_the_grace_is_not_asked() {
    let fixture = Fixture::new(
        &[(ASK_APP, "ask"), ("com.other.app", "ask")],
        vec![PromptAnswer::Approved, PromptAnswer::Denied],
    );
    let approval = fixture.approval();
    assert!(
        approval
            .decide(&su(1, ASK_APP, &["id"]), Some(&backup_app()))
            .approved
    );
    assert!(
        approval
            .decide(&su(2, ASK_APP, &["ls"]), Some(&backup_app()))
            .approved
    );
    assert_eq!(fixture.prompt.asked().len(), 1);
    // Another app's grace is its own: it is asked.
    assert!(
        !approval
            .decide(&su(3, "com.other.app", &["id"]), None)
            .approved
    );
    assert_eq!(fixture.prompt.asked().len(), 2);
}

#[test]
fn fr35_a_restart_of_android_asks_again() {
    let fixture = Fixture::new(
        &[(ASK_APP, "ask")],
        vec![PromptAnswer::Approved, PromptAnswer::Approved],
    );
    let approval = fixture.approval();
    assert!(
        approval
            .decide(&su(1, ASK_APP, &["id"]), Some(&backup_app()))
            .approved
    );
    approval.restart();
    assert!(
        approval
            .decide(&su(2, ASK_APP, &["id"]), Some(&backup_app()))
            .approved
    );
    assert_eq!(fixture.prompt.asked().len(), 2);
}

#[test]
fn fr35_changing_the_apps_level_asks_again() {
    let fixture = Fixture::new(
        &[(ASK_APP, "ask")],
        vec![PromptAnswer::Approved, PromptAnswer::Approved],
    );
    let approval = fixture.approval();
    assert!(
        approval
            .decide(&su(1, ASK_APP, &["id"]), Some(&backup_app()))
            .approved
    );
    // Off and back to ask between two calls clears the grace.
    fixture.store.set_root_app(USER, ASK_APP, "off").unwrap();
    let off = approval.decide(&su(2, ASK_APP, &["id"]), Some(&backup_app()));
    assert!(!off.approved);
    fixture.store.set_root_app(USER, ASK_APP, "ask").unwrap();
    assert!(
        approval
            .decide(&su(3, ASK_APP, &["id"]), Some(&backup_app()))
            .approved
    );
    assert_eq!(fixture.prompt.asked().len(), 2);
}

#[test]
fn fr35_off_app_is_denied_without_a_prompt() {
    let fixture = Fixture::new(&[], vec![PromptAnswer::Approved]);
    let decision = fixture
        .approval()
        .decide(&su(1, "com.not.allowed", &["id"]), None);
    assert!(!decision.approved);
    assert!(!decision.reason.is_empty());
    assert!(fixture.prompt.asked().is_empty());
}

#[test]
fn fr35_remember_app_is_approved_without_a_prompt() {
    let fixture = Fixture::new(&[("com.remember.app", "remember")], vec![]);
    let decision = fixture
        .approval()
        .decide(&su(1, "com.remember.app", &["id"]), None);
    assert!(decision.approved);
    assert!(fixture.prompt.asked().is_empty());
}

#[test]
fn fr35_the_feature_turned_off_stops_approvals_and_turned_on_waits_for_a_restart() {
    let fixture = Fixture::new(&[("com.remember.app", "remember")], vec![]);
    let approval = fixture.approval();
    fixture
        .store
        .set_machine(MachineSettings {
            root_elevation: false,
            ..Default::default()
        })
        .unwrap();
    assert!(
        !approval
            .decide(&su(1, "com.remember.app", &["id"]), None)
            .approved
    );
    // On again, but this boot was told it is off: still denied until Android restarts.
    fixture
        .store
        .set_machine(MachineSettings {
            root_elevation: true,
            ..Default::default()
        })
        .unwrap();
    assert!(
        !approval
            .decide(&su(2, "com.remember.app", &["id"]), None)
            .approved
    );
    approval.restart();
    assert!(
        approval
            .decide(&su(3, "com.remember.app", &["id"]), None)
            .approved
    );
}

#[test]
fn fr35_one_prompt_at_a_time_and_a_waiting_call_finds_the_grace() {
    let fixture = Fixture::new(&[(ASK_APP, "ask")], vec![PromptAnswer::Approved]);
    *fixture.prompt.delay.lock().unwrap() = Duration::from_millis(200);
    let approval = Arc::new(fixture.approval());
    let calls: Vec<_> = (1..=3)
        .map(|id| {
            let approval = Arc::clone(&approval);
            thread::spawn(move || approval.decide(&su(id, ASK_APP, &["id"]), Some(&backup_app())))
        })
        .collect();
    let started = Instant::now();
    for call in calls {
        assert!(call.join().unwrap().approved);
    }
    // One prompt answered all three: the others waited for it and found the grace.
    assert_eq!(fixture.prompt.asked().len(), 1);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn fr35_requests_and_decisions_are_in_the_host_log() {
    let fixture = Fixture::new(
        &[(ASK_APP, "ask")],
        vec![PromptAnswer::Approved, PromptAnswer::Denied],
    );
    let approval = fixture.approval();
    approval.decide(
        &su(11, ASK_APP, &["sh", "-c", "echo hi"]),
        Some(&backup_app()),
    );
    approval.restart();
    approval.decide(&su(12, ASK_APP, &[]), Some(&backup_app()));
    let lines = fixture.lines();
    let about = |id: u64| -> Vec<String> {
        lines
            .iter()
            .filter(|line| line.contains(&format!("request {id}")))
            .cloned()
            .collect()
    };
    let first = about(11);
    assert!(first.iter().all(|line| line.starts_with("root: ")
        && line.contains(USER)
        && line.contains("Backup")
        && line.contains(ASK_APP)));
    assert!(
        first.iter().any(|line| line.contains("sh -c 'echo hi'")),
        "{first:?}"
    );
    assert!(first
        .iter()
        .any(|line| line.contains("approved by the computer's administrator")));
    // Each line carries the time, as an ISO 8601 UTC stamp.
    assert!(first
        .iter()
        .all(|line| line.contains('T') && line.contains("Z ")));
    let second = about(12);
    assert!(
        second
            .iter()
            .any(|line| line.contains("an interactive shell")),
        "{second:?}"
    );
    assert!(second.iter().any(|line| line.contains("refused")));
}

#[test]
fn fr35_commands_are_written_as_a_shell_would_read_them() {
    assert_eq!(command_text(&[], true), "an interactive shell");
    assert_eq!(command_text(&["id".into()], false), "id");
    assert_eq!(
        command_text(&["sh".into(), "-c".into(), "it's".into()], false),
        r"sh -c 'it'\''s'"
    );
    assert_eq!(command_text(&["".into()], false), "''");
}
