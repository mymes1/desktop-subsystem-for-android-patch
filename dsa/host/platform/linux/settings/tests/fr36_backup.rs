//! The Backup section of DSA Settings for Linux, without a display: the requests it makes
//! through the local API, what it reads from the answers, which buttons work, and what it
//! says.

use std::sync::{Arc, Mutex};

use dsa_settings_linux::api::{Dsa, Failure, Problem};
use dsa_settings_linux::model::{
    backup_actions, backup_key_words, Backup, BackupActions, BackupJob, Client, KeyImport,
};
use dsa_settings_linux::view;
use serde_json::json;

/// A stand-in for the service: it records each command line and answers with `answer`.
struct Fake {
    calls: Mutex<Vec<Vec<String>>>,
    answer: Result<String, Failure>,
}

impl Fake {
    fn new(answer: serde_json::Value) -> Arc<Fake> {
        Arc::new(Fake {
            calls: Mutex::new(Vec::new()),
            answer: Ok(answer.to_string()),
        })
    }

    fn failing(failure: Failure) -> Arc<Fake> {
        Arc::new(Fake {
            calls: Mutex::new(Vec::new()),
            answer: Err(failure),
        })
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
}

impl Dsa for Fake {
    fn run(&self, args: &[String]) -> Result<String, Failure> {
        self.calls.lock().unwrap().push(args.to_vec());
        self.answer.clone()
    }
}

fn line(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

fn client(fake: &Arc<Fake>) -> Client {
    Client::new(fake.clone(), fake.clone(), true)
}

/// What `dsa backup --json` prints for a backup of 12 apps made on 10 November 2026.
fn report() -> serde_json::Value {
    json!({
        "available": true,
        "problem": "",
        "folder": "/home/ada/DSA Backup",
        "last_backup_unix_ms": 1_794_312_000_000_i64,
        "last_restore_unix_ms": 0,
        "packages": 12,
        "size": 50 * 1024 * 1024,
        "last_error": "",
        "succeeded": 0,
        "failed": 0,
        "installed": 0
    })
}

#[test]
fn fr36_backup_status_reads_the_service_report() {
    let backup = Backup::read(&report());
    assert!(backup.available);
    assert_eq!(backup.problem, None);
    assert_eq!(backup.folder, "/home/ada/DSA Backup");
    assert_eq!(backup.last_backup(), Some(1_794_312_000_000));
    assert_eq!(backup.last_restore(), None);
    assert_eq!(backup.packages, 12);
    assert_eq!(backup.size, 50 * 1024 * 1024);
    assert_eq!(backup.last_error, None);
    assert_eq!(backup.shown_problem(), None);

    // The section shows the time in local time, or that there was none.
    let local = |ms: i64| format!("local {ms}");
    assert_eq!(
        view::backup_when(backup.last_backup(), local),
        "local 1794312000000"
    );
    assert_eq!(view::backup_when(backup.last_restore(), local), "Never");
    assert_eq!(view::backup_contents(&backup), "12 apps, 50 MB");

    // A backup with nothing in it yet, and an empty answer, read as never made.
    let empty = Backup::read(&json!({}));
    assert!(!empty.available);
    assert_eq!(empty.last_backup(), None);
    assert_eq!(view::backup_contents(&empty), "No apps yet");

    // What went wrong last is shown.
    let mut failed = report();
    failed["last_error"] = json!("Android did not answer");
    assert_eq!(
        Backup::read(&failed).last_error.as_deref(),
        Some("Android did not answer")
    );
}

#[test]
fn fr36_an_unavailable_backup_shows_its_problem() {
    let why = "/home/ada/DSA Backup holds a backup, but this computer has no key for it; enter \
               its recovery key (dsa backup key import) to use it here";
    let backup = Backup::read(&json!({
        "available": false,
        "problem": why,
        "folder": "/home/ada/DSA Backup",
        "last_backup_unix_ms": 0,
        "packages": 0,
        "size": 0
    }));
    assert_eq!(backup.shown_problem(), Some(why));
    // Only a recovery key can be entered: it is what makes the folder readable.
    assert_eq!(
        backup_actions(Some(&backup), None),
        BackupActions {
            back_up: false,
            restore: false,
            show_key: false,
            import_key: true,
        }
    );
    // An available backup never shows a problem, whatever the field holds.
    let mut odd = report();
    odd["problem"] = json!("stale words");
    assert_eq!(Backup::read(&odd).shown_problem(), None);
}

#[test]
fn fr36_backup_requests_are_the_dsa_command_lines() {
    let fake = Fake::new(report());
    let c = client(&fake);
    c.backup().unwrap();
    c.back_up_now().unwrap();
    c.restore_backup().unwrap();
    assert_eq!(
        fake.calls(),
        vec![
            line(&["backup", "status", "--json"]),
            line(&["backup", "now", "--json"]),
            line(&["backup", "restore", "--json"]),
        ]
    );

    let fake = Fake::new(json!({ "recovery_key": "ABCDE-FGHIJ-KMNPQ" }));
    assert_eq!(
        client(&fake).backup_recovery_key().unwrap(),
        "ABCDE-FGHIJ-KMNPQ"
    );
    assert_eq!(
        fake.calls(),
        vec![line(&["backup", "key", "show", "--json"])]
    );
}

#[test]
fn fr36_a_recovery_key_import_carries_the_words() {
    let fake = Fake::new(report());
    let kept = client(&fake)
        .import_backup_key("  abcde-fghij  KMNPQ -RSTVW-\n")
        .unwrap();
    assert_eq!(kept, KeyImport::Kept(Backup::read(&report())));
    assert_eq!(
        fake.calls(),
        vec![line(&[
            "backup",
            "key",
            "import",
            "abcde-fghij",
            "KMNPQ",
            "RSTVW",
            "--json"
        ])]
    );
    // Nothing that can be a key is not sent at all.
    assert!(backup_key_words(" - -- ").is_empty());
    let fake = Fake::new(report());
    assert_eq!(
        client(&fake).import_backup_key("  ").unwrap(),
        KeyImport::Refused(String::new())
    );
    assert!(fake.calls().is_empty());
    assert_eq!(view::backup_key_refusal(""), "Enter the recovery key.");
}

#[test]
fn fr36_a_wrong_recovery_key_says_why_it_was_refused() {
    let fake = Fake::failing(Failure::from_client(
        "dsa: the DSA service: this recovery key is not the one /home/ada/DSA Backup was made with",
    ));
    let refused = client(&fake).import_backup_key("ABCDE").unwrap();
    let KeyImport::Refused(reason) = &refused else {
        panic!("a wrong key is a refusal: {refused:?}");
    };
    assert_eq!(
        reason,
        "this recovery key is not the one /home/ada/DSA Backup was made with"
    );
    assert_eq!(
        view::backup_key_refusal(reason),
        "The key was not used: this recovery key is not the one /home/ada/DSA Backup was made with"
    );
    // A service that cannot be reached is not a refusal of the key.
    let fake = Fake::failing(Failure::new(
        Problem::ServiceUnreachable,
        "cannot reach the DSA service",
    ));
    assert_eq!(
        client(&fake)
            .import_backup_key("ABCDE")
            .unwrap_err()
            .problem,
        Problem::ServiceUnreachable
    );
}

#[test]
fn fr36_no_backup_button_works_while_a_request_runs() {
    let backup = Backup::read(&report());
    assert_eq!(
        backup_actions(Some(&backup), None),
        BackupActions {
            back_up: true,
            restore: true,
            show_key: true,
            import_key: true,
        }
    );
    for job in [
        BackupJob::Read,
        BackupJob::BackUp,
        BackupJob::Restore,
        BackupJob::ShowKey,
        BackupJob::ImportKey,
    ] {
        assert_eq!(
            backup_actions(Some(&backup), Some(job)),
            BackupActions::default()
        );
    }
    // Nothing works before the backup is read.
    assert_eq!(backup_actions(None, None), BackupActions::default());
    // Restoring needs a backup with apps in it.
    let mut empty = report();
    empty["packages"] = json!(0);
    let empty = Backup::read(&empty);
    assert!(!backup_actions(Some(&empty), None).restore);
    assert!(backup_actions(Some(&empty), None).back_up);
    // Only backing up and restoring wait for Android, so only they show progress.
    assert!(BackupJob::BackUp.is_long());
    assert!(BackupJob::Restore.is_long());
    assert!(!BackupJob::ShowKey.is_long());
    assert!(!view::backup_busy_text(BackupJob::BackUp).is_empty());
    assert!(!view::backup_busy_text(BackupJob::Restore).is_empty());
}

#[test]
fn fr36_the_result_of_a_backup_and_a_restore_is_shown() {
    let mut done = report();
    done["succeeded"] = json!(11);
    done["failed"] = json!(1);
    done["installed"] = json!(3);
    let done = Backup::read(&done);
    assert_eq!(
        view::backup_result(BackupJob::BackUp, &done),
        "Apps backed up: 11. Apps that could not be backed up: 1."
    );
    assert_eq!(
        view::backup_result(BackupJob::Restore, &done),
        "Apps restored: 11. Apps installed first: 3. Apps that could not be restored: 1."
    );
    let mut clean = report();
    clean["succeeded"] = json!(12);
    clean["installed"] = json!(2);
    let clean = Backup::read(&clean);
    assert_eq!(
        view::backup_result(BackupJob::BackUp, &clean),
        "Apps backed up: 12."
    );
    assert_eq!(
        view::backup_result(BackupJob::Restore, &clean),
        "Apps restored: 12. Apps installed first: 2."
    );
    // The question before a restore says what it replaces.
    assert!(view::backup_restore_warning().contains("replaced"));
    assert!(view::backup_key_advice().contains("Write this key down"));
}
