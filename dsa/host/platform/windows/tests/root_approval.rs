//! Acceptance tests for the Windows administrator approval of an Android app's su call, with
//! the in-memory local transport standing in for the approval pipe and a fake standing in for
//! the app's window program, so no real UAC prompt is shown.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::root_approval::PromptAnswer;
use dsa_host_windows::root_approval::{
    answer_of_exit, approval_arguments, exit, helper_mode, is_pipe_name, pipe_name,
    wait_for_answer, HelperMode, PromptProcess, APPROVAL_ARGUMENT, APPROVED_ARGUMENT, RECEIVED,
};
use dsa_transport::local::{self, Peer};
use dsa_transport::Connection;

/// A window program that ends with `code` after `after`, or runs until it is ended.
struct FakeProgram {
    started: Instant,
    ends: Option<(Duration, u32)>,
    ended: Arc<AtomicBool>,
}

impl FakeProgram {
    fn ending(after: Duration, code: u32) -> FakeProgram {
        FakeProgram {
            started: Instant::now(),
            ends: Some((after, code)),
            ended: Arc::default(),
        }
    }

    fn running() -> FakeProgram {
        FakeProgram {
            started: Instant::now(),
            ends: None,
            ended: Arc::default(),
        }
    }
}

impl PromptProcess for FakeProgram {
    fn exit_code(&mut self) -> Option<u32> {
        if self.ended.load(Ordering::Relaxed) {
            return Some(exit::FAILED);
        }
        self.ends
            .filter(|(after, _)| self.started.elapsed() >= *after)
            .map(|(_, code)| code)
    }

    fn end(&mut self) {
        self.ended.store(true, Ordering::Relaxed);
    }
}

fn secret() -> String {
    pipe_name(&[0xab; 16])
}

fn administrator() -> Peer {
    Peer {
        user: "S-1-5-21-1-2-3-500".into(),
        administrator: true,
    }
}

#[test]
fn fr35_an_elevated_administrators_answer_approves() {
    let (mut listener, connector) = local::pair();
    let pipe = secret();
    let answer = {
        let pipe = pipe.clone();
        let mut connector = connector.clone();
        thread::spawn(move || {
            let mut elevated = connector.connect_as(administrator()).unwrap();
            elevated.send(pipe.as_bytes()).unwrap();
            elevated.recv(Some(Duration::from_secs(5))).unwrap()
        })
    };
    let mut program = FakeProgram::running();
    let got = wait_for_answer(
        &mut listener,
        pipe.as_bytes(),
        &mut program,
        Duration::from_secs(5),
    );
    assert_eq!(got, PromptAnswer::Approved);
    // The elevated copy is told the approval arrived.
    assert_eq!(answer.join().unwrap(), RECEIVED);
    drop(connector);
}

#[test]
fn fr35_an_answer_without_administrator_rights_does_not_approve() {
    let (mut listener, mut connector) = local::pair();
    let pipe = secret();
    // An unelevated process of the user, and an administrator that does not know the pipe's
    // name, both try.
    let mut unelevated = connector
        .connect_as(Peer {
            user: "S-1-5-21-1-2-3-1001".into(),
            administrator: false,
        })
        .unwrap();
    unelevated.send(pipe.as_bytes()).unwrap();
    let mut guesser = connector.connect_as(administrator()).unwrap();
    guesser.send(b"dsa-root-approval-00").unwrap();
    let mut program = FakeProgram::running();
    let got = wait_for_answer(
        &mut listener,
        pipe.as_bytes(),
        &mut program,
        Duration::from_millis(600),
    );
    assert_eq!(got, PromptAnswer::TimedOut);
}

#[test]
fn fr35_a_cancelled_uac_prompt_is_refused() {
    let (mut listener, _connector) = local::pair();
    let mut program = FakeProgram::ending(Duration::from_millis(50), exit::REFUSED);
    let started = Instant::now();
    let got = wait_for_answer(
        &mut listener,
        secret().as_bytes(),
        &mut program,
        Duration::from_secs(10),
    );
    assert_eq!(got, PromptAnswer::Refused);
    // The refusal comes as soon as the prompt closes, not at the limit.
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn fr35_a_standard_user_with_uac_off_is_refused() {
    let (mut listener, _connector) = local::pair();
    let mut program = FakeProgram::ending(Duration::ZERO, exit::NOT_ADMINISTRATOR);
    let got = wait_for_answer(
        &mut listener,
        secret().as_bytes(),
        &mut program,
        Duration::from_secs(10),
    );
    assert_eq!(got, PromptAnswer::NotAdministrator);
}

#[test]
fn fr35_an_unanswered_prompt_times_out_and_is_ended() {
    let (mut listener, _connector) = local::pair();
    let mut program = FakeProgram::running();
    let ended = Arc::clone(&program.ended);
    let started = Instant::now();
    let got = wait_for_answer(
        &mut listener,
        secret().as_bytes(),
        &mut program,
        Duration::from_millis(400),
    );
    assert_eq!(got, PromptAnswer::TimedOut);
    assert!(started.elapsed() >= Duration::from_millis(400));
    assert!(ended.load(Ordering::Relaxed), "the program was ended");
}

#[test]
fn fr35_a_program_that_ends_without_an_approval_never_approves() {
    // Only the pipe approves: an exit code, even the one the elevated copy ends with after
    // delivering, does not.
    for code in [
        exit::APPROVED,
        exit::FAILED,
        exit::NOT_DELIVERED,
        exit::ELEVATION_FAILED,
        exit::BAD_ARGUMENTS,
        77,
    ] {
        assert!(
            matches!(answer_of_exit(code), PromptAnswer::Failed(_)),
            "code {code}"
        );
    }
    assert_eq!(answer_of_exit(exit::REFUSED), PromptAnswer::Refused);
    assert_eq!(
        answer_of_exit(exit::NOT_ADMINISTRATOR),
        PromptAnswer::NotAdministrator
    );
}

#[test]
fn fr35_the_window_program_reads_what_the_service_asks() {
    let pipe = secret();
    assert!(is_pipe_name(&pipe));
    let line = approval_arguments(&pipe, "Backup \"Pro\"", "sh -c 'echo hi'");
    let args: Vec<String> = line.split(' ').map(str::to_string).collect();
    assert_eq!(
        helper_mode(&args),
        Some(HelperMode::Ask {
            pipe: pipe.clone(),
            label: "Backup \"Pro\"".into(),
            command: "sh -c 'echo hi'".into(),
        })
    );
    let elevated = vec![APPROVED_ARGUMENT.to_string(), pipe.clone()];
    assert_eq!(
        helper_mode(&elevated),
        Some(HelperMode::Deliver { pipe: pipe.clone() })
    );
    // A pipe that is not an approval pipe is refused, so nothing else can be dialled.
    let bad = vec![APPROVAL_ARGUMENT.to_string(), r"..\other-pipe".to_string()];
    assert_eq!(helper_mode(&bad), Some(HelperMode::Invalid));
    assert!(!is_pipe_name("dsa-root-approval-"));
    assert!(!is_pipe_name("dsa-root-approval-AB"));
    // A window host started for its app asks for nothing.
    assert_eq!(
        helper_mode(&["--package".into(), "com.example".into()]),
        None
    );
}
