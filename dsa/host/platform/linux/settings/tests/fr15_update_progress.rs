//! DSA Settings for Linux follows an update of DSA while it runs: the phase the service
//! reports, the gap while the service is replaced, and the version that runs once it is back.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dsa_settings_linux::api::{Dsa, Failure, Problem};
use dsa_settings_linux::model::{Client, ProgramUpdateAnswer, ProgramUpdatePhase, Status};
use dsa_settings_linux::view::{ProgramUpdateFollower, PROGRAM_RESTART_LIMIT};
use serde_json::json;

/// A stand-in for the service that records each command line and answers with `answer`.
struct Fake {
    calls: Mutex<Vec<Vec<String>>>,
    answer: String,
}

impl Dsa for Fake {
    fn run(&self, args: &[String]) -> Result<String, Failure> {
        self.calls.lock().unwrap().push(args.to_vec());
        Ok(self.answer.clone())
    }
}

fn client(answer: serde_json::Value) -> (Client, Arc<Fake>) {
    let fake = Arc::new(Fake {
        calls: Mutex::new(Vec::new()),
        answer: answer.to_string(),
    });
    (Client::new(fake.clone(), fake.clone(), false), fake)
}

fn status(phase: Option<&str>) -> Status {
    Status::read(&json!({
        "state": "Stopped",
        "last_reason": null,
        "update_notice": null,
        "problem": null,
        "program_update": phase.map(|p| json!({"phase": p, "version": "0.1.11", "in_progress": true})),
    }))
}

fn unreachable() -> Failure {
    Failure::new(Problem::ServiceUnreachable, "cannot reach the DSA service")
}

fn installing() -> Result<ProgramUpdateAnswer, Failure> {
    Ok(ProgramUpdateAnswer {
        installing: Some("0.1.11".into()),
        detail: "installing DSA 0.1.11".into(),
    })
}

#[test]
fn fr15_the_status_is_read_from_a_running_service_only_with_its_update_phase() {
    let (client, fake) = client(json!({
        "state": "Stopped",
        "program_update": {"phase": "stopping_android", "version": "0.1.11", "in_progress": true},
    }));
    let read = client.running_status().unwrap();
    let update = read.program_update.unwrap();
    assert_eq!(update.phase, ProgramUpdatePhase::StoppingAndroid);
    assert_eq!(update.version, "0.1.11");
    assert_eq!(
        fake.calls.lock().unwrap()[0],
        ["status", "--no-start", "--json"]
    );
    assert_eq!(status(None).program_update, None);
    assert_eq!(status(Some("something newer")).program_update, None);
}

#[test]
fn fr15_install_now_answers_with_the_version_or_the_package_manager() {
    let (client, fake) = client(
        json!({"installing": null, "detail": "DSA is updated by the system's package manager"}),
    );
    let answer = client.update_now().unwrap();
    assert_eq!(answer.installing, None);
    assert!(answer.detail.contains("package manager"));
    assert_eq!(fake.calls.lock().unwrap()[0], ["update", "--json"]);
}

#[test]
fn fr15_each_phase_the_service_reports_is_shown_with_its_version() {
    for (name, phase, said) in [
        (
            "downloading",
            ProgramUpdatePhase::Downloading,
            "Downloading DSA 0.1.11",
        ),
        (
            "stopping_android",
            ProgramUpdatePhase::StoppingAndroid,
            "Stopping Android to install DSA 0.1.11",
        ),
        (
            "installing",
            ProgramUpdatePhase::Installing,
            "Installing DSA 0.1.11",
        ),
        (
            "restarting",
            ProgramUpdatePhase::Restarting,
            "DSA is restarting to finish the update",
        ),
    ] {
        let mut follower = ProgramUpdateFollower::default();
        assert!(!follower.polled(Ok(&status(Some(name))), Instant::now()));
        assert_eq!(follower.phase(), phase, "{name}");
        assert!(follower.in_progress(), "{name}");
        assert_eq!(follower.phase_text(), said);
        assert_eq!(follower.version_text().as_deref(), Some("Version 0.1.11"));
        assert!(ProgramUpdateFollower::keep_open_text().contains("Do not close DSA"));
        assert_eq!(follower.ended_text(), None);
    }
}

#[test]
fn fr15_install_now_shows_progress_at_once_and_ignores_news_of_an_earlier_update() {
    let mut follower = ProgramUpdateFollower::default();
    follower.asked();
    assert_eq!(follower.phase(), ProgramUpdatePhase::Starting);
    assert!(follower.in_progress());
    assert_eq!(follower.phase_text(), "Getting the update ready");
    assert_eq!(follower.version_text(), None);
    follower.polled(Ok(&status(None)), Instant::now());
    follower.polled(Ok(&status(Some("failed"))), Instant::now());
    assert_eq!(follower.phase(), ProgramUpdatePhase::Starting);
    follower.answered(installing(), Instant::now());
    assert_eq!(follower.phase(), ProgramUpdatePhase::Installing);
}

#[test]
fn fr15_the_restart_gap_reads_as_restarting_and_the_new_version_is_shown_once_it_answers() {
    let mut follower = ProgramUpdateFollower::default();
    let now = Instant::now();
    follower.polled(Ok(&status(Some("installing"))), now);
    assert!(!follower.polled(Err(&unreachable()), now));
    assert_eq!(follower.phase(), ProgramUpdatePhase::Restarting);
    assert_eq!(
        follower.phase_text(),
        "DSA is restarting to finish the update"
    );
    assert_eq!(
        follower.error(),
        None,
        "the service being away is the restart"
    );
    assert!(!follower.polled(Err(&unreachable()), now + Duration::from_secs(5)));
    assert!(follower.in_progress());
    assert!(follower.polled(Ok(&status(Some("applied"))), now + Duration::from_secs(9)));
    assert_eq!(follower.phase(), ProgramUpdatePhase::Applied);
    assert_eq!(
        follower.ended_text(),
        Some((true, "Updated to 0.1.11".to_string()))
    );
    assert!(!follower.in_progress());

    // A new service with nothing to report runs the version that was installed.
    let mut quiet = ProgramUpdateFollower::default();
    quiet.polled(Ok(&status(Some("installing"))), now);
    quiet.polled(Err(&unreachable()), now);
    assert!(quiet.polled(Ok(&status(None)), now));
    assert_eq!(
        quiet.ended_text(),
        Some((true, "Updated to 0.1.11".to_string()))
    );
}

#[test]
fn fr15_a_failed_update_a_lost_answer_and_a_service_away_too_long() {
    let now = Instant::now();
    let mut failed = ProgramUpdateFollower::default();
    failed.polled(Ok(&status(Some("installing"))), now);
    failed.polled(Ok(&status(Some("failed"))), now);
    assert_eq!(
        failed.ended_text(),
        Some((
            false,
            "Updating to 0.1.11 failed. The version before it is still installed.".to_string()
        ))
    );

    let mut lost = ProgramUpdateFollower::default();
    lost.asked();
    lost.polled(Ok(&status(Some("installing"))), now);
    lost.answered(
        Err(Failure::new(Problem::Refused, "the connection closed")),
        now,
    );
    assert_eq!(lost.phase(), ProgramUpdatePhase::Restarting);
    assert_eq!(lost.error(), None);

    let mut refused = ProgramUpdateFollower::default();
    refused.asked();
    refused.answered(
        Err(Failure::new(
            Problem::Refused,
            "Android did not stop in time",
        )),
        now,
    );
    assert_eq!(refused.phase(), ProgramUpdatePhase::None);
    assert!(refused.error().unwrap().contains("did not stop in time"));

    let mut away = ProgramUpdateFollower::default();
    away.polled(Ok(&status(Some("installing"))), now);
    away.polled(Err(&unreachable()), now);
    away.polled(
        Err(&unreachable()),
        now + PROGRAM_RESTART_LIMIT + Duration::from_secs(1),
    );
    assert_eq!(away.phase(), ProgramUpdatePhase::Failed);
    assert!(away.ended_text().unwrap().1.contains("has not come back"));
    assert!(away.polled(
        Ok(&status(Some("applied"))),
        now + PROGRAM_RESTART_LIMIT * 2
    ));
    assert_eq!(away.phase(), ProgramUpdatePhase::Applied);

    // Without an update, a service that is not running shows nothing.
    let mut idle = ProgramUpdateFollower::default();
    idle.polled(Err(&unreachable()), now);
    assert_eq!(idle.phase(), ProgramUpdatePhase::None);
    assert!(!idle.in_progress());
}
