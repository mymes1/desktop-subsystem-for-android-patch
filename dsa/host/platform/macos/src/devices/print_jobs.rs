//! The print jobs of the user's Android, printed on the Mac.
//!
//! The follower watches the print jobs over the local API ([`serve`]). Each job comes whole:
//! a PDF that Android's print dialog laid out, with the settings the user chose there. It is
//! handed to the [`Print`] function (on a Mac, the print panel through PDFKit), which lets
//! the user pick the printer, starting from Android's choices, and prints it. What became of
//! the job goes back to Android: printed, canceled in the panel, or failed with the reason. A
//! job Android withdraws before its panel opens is not printed. Nothing about a job ends the
//! follower.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use dsa_host_core::host_api::HostApiClient;
use dsa_host_core::print::{PrintEvent, PrintJob, PrintOutcome};
use dsa_transport::Connector;

use super::Log;

/// What the follower calls itself to the service.
const BUILD: &str = "dsa-devices print";

/// How long a request to the service is waited for.
const ANSWER: Duration = Duration::from_secs(5);

/// How long to wait before connecting again after the service could not be reached.
const RETRY: Duration = Duration::from_secs(1);

/// How long the watch waits for the service before it looks for finished jobs again.
const POLL: Duration = Duration::from_millis(100);

/// Prints one job and says what became of it. It may block until the user answers.
pub type Print = Arc<dyn Fn(PrintJob) -> PrintOutcome + Send + Sync>;

/// Prints `job` with `print` unless it was withdrawn first. A panic in the printing code is
/// a failed job, never the end of the follower.
pub fn print_one(job: PrintJob, print: &Print, withdrawn: &AtomicBool) -> PrintOutcome {
    if withdrawn.load(Ordering::Relaxed) {
        return PrintOutcome::Canceled;
    }
    if job.pdf.is_empty() {
        return PrintOutcome::Failed("the document is empty".into());
    }
    catch_unwind(AssertUnwindSafe(|| print(job)))
        .unwrap_or_else(|_| PrintOutcome::Failed("the printing code failed".into()))
}

/// Prints the documents of the user's Android until `stop`, reconnecting to the service
/// whenever the connection ends. Each job prints on a thread of its own.
pub fn serve(mut connector: Box<dyn Connector>, print: Print, stop: Arc<AtomicBool>, log: Log) {
    let (done_tx, done) = mpsc::channel();
    let mut printing: HashMap<u32, Arc<AtomicBool>> = HashMap::new();
    let mut last_error = String::new();
    while !stop.load(Ordering::Relaxed) {
        let stream = HostApiClient::connect(&mut *connector, BUILD, ANSWER)
            .and_then(|client| client.watch_print_jobs());
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                let error = error.to_string();
                if error != last_error {
                    log(&format!("printing cannot reach the service: {error}"));
                    last_error = error;
                }
                report(&mut *connector, &done, &mut printing, &log);
                super::wait(&stop, RETRY);
                continue;
            }
        };
        last_error.clear();
        while !stop.load(Ordering::Relaxed) {
            report(&mut *connector, &done, &mut printing, &log);
            match stream.next(POLL) {
                Ok(Some(PrintEvent::Job(job))) => {
                    let withdrawn = Arc::new(AtomicBool::new(false));
                    printing.insert(job.job_id, Arc::clone(&withdrawn));
                    start_job(
                        job,
                        Arc::clone(&print),
                        withdrawn,
                        done_tx.clone(),
                        Arc::clone(&log),
                    );
                }
                Ok(Some(PrintEvent::Withdrawn { job_id })) => {
                    if let Some(withdrawn) = printing.get(&job_id) {
                        withdrawn.store(true, Ordering::Relaxed);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    log(&format!(
                        "the print jobs' connection to the service ended: {error}"
                    ));
                    break;
                }
            }
        }
    }
    for withdrawn in printing.values() {
        withdrawn.store(true, Ordering::Relaxed);
    }
}

/// Passes what became of each finished job to the service.
fn report(
    connector: &mut dyn Connector,
    done: &Receiver<(u32, PrintOutcome)>,
    printing: &mut HashMap<u32, Arc<AtomicBool>>,
    log: &Log,
) {
    while let Ok((job_id, outcome)) = done.try_recv() {
        let withdrawn = printing
            .remove(&job_id)
            .is_some_and(|withdrawn| withdrawn.load(Ordering::Relaxed));
        // A withdrawn job's outcome is not wanted.
        if withdrawn {
            continue;
        }
        let result = HostApiClient::connect(connector, BUILD, ANSWER)
            .and_then(|mut client| client.report_print_job(job_id, &outcome));
        if let Err(error) = result {
            log(&format!(
                "what became of print job {job_id} did not reach Android: {error}"
            ));
        }
    }
}

fn start_job(
    job: PrintJob,
    print: Print,
    withdrawn: Arc<AtomicBool>,
    done: Sender<(u32, PrintOutcome)>,
    log: Log,
) {
    let job_id = job.job_id;
    let failed = done.clone();
    let spawned = thread::Builder::new()
        .name("dsa-print-job".into())
        .spawn(move || {
            let title = job.title();
            let outcome = print_one(job, &print, &withdrawn);
            match &outcome {
                PrintOutcome::Failed(why) => log(&format!("{title} was not printed: {why}")),
                PrintOutcome::Canceled => log(&format!("printing {title} was canceled")),
                PrintOutcome::Printed => {}
            }
            let _ = done.send((job_id, outcome));
        });
    if let Err(error) = spawned {
        let why = format!("printing could not start: {error}");
        let _ = failed.send((job_id, PrintOutcome::Failed(why)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsa_host_core::print::PrintSettings;

    fn job() -> PrintJob {
        PrintJob {
            job_id: 7,
            package_name: "com.example.notes".into(),
            app_label: "Notes".into(),
            document_name: "List".into(),
            settings: PrintSettings {
                copies: 2,
                ..PrintSettings::default()
            },
            pdf: Arc::new(b"%PDF-1.7\n".to_vec()),
        }
    }

    #[test]
    fn fr26_a_job_reaches_the_mac_with_its_settings() {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let print: Print = {
            let seen = Arc::clone(&seen);
            Arc::new(move |job: PrintJob| {
                *seen.lock().unwrap() = Some(job);
                PrintOutcome::Printed
            })
        };
        let outcome = print_one(job(), &print, &AtomicBool::new(false));
        assert_eq!(outcome, PrintOutcome::Printed);
        assert_eq!(seen.lock().unwrap().as_ref(), Some(&job()));
    }

    #[test]
    fn fr26_a_job_withdrawn_before_its_panel_opens_is_not_printed() {
        let print: Print = Arc::new(|_| panic!("a withdrawn job was printed"));
        let outcome = print_one(job(), &print, &AtomicBool::new(true));
        assert_eq!(outcome, PrintOutcome::Canceled);
    }

    #[test]
    fn fr26_a_failure_in_the_printing_code_fails_only_that_job() {
        let print: Print = Arc::new(|_| panic!("the panel failed"));
        let outcome = print_one(job(), &print, &AtomicBool::new(false));
        assert!(matches!(outcome, PrintOutcome::Failed(_)));
    }
}
