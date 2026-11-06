//! Opening the web links apps in the user's Android open, in the user's default browser.
//!
//! The follower watches the links over the local API and hands each to `xdg-open`, which
//! starts the browser the user chose on their desktop. Only http and https links it has
//! checked itself are handed on: given a file path or another scheme, `xdg-open` would start
//! whatever program handles it, and an app in Android must never be able to do that. The
//! link is one argument after `--`, never shell text.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use dsa_host_core::host_api::HostApiClient;
use dsa_transport::Connector;

use super::Log;

/// The build the follower names itself by.
const BUILD: &str = "dsa-devices links";

/// How long the watch waits for a link before it looks at `stop` again.
const POLL: Duration = Duration::from_millis(200);

/// How long the watch waits before it reconnects to the service.
const RETRY: Duration = Duration::from_secs(2);

/// Opens a link the user's Android opened.
pub trait LinkOpener: Send {
    fn open(&mut self, url: &str) -> Result<(), String>;
}

/// Checks a link from Android before it is opened: an http or https link, with a host and
/// nothing a program could take for something else.
pub fn openable(url: &str) -> Result<&str, String> {
    dsa_host_core::links::web_url(url)
}

/// The command that opens `url` in the user's browser.
pub fn open_command(url: &str) -> Result<Command, String> {
    let url = openable(url)?;
    let mut command = Command::new("xdg-open");
    // xdg-open takes no options, so the link cannot be read as one; it starts with http.
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

/// Opens links with `xdg-open`.
pub struct XdgOpener;

impl LinkOpener for XdgOpener {
    fn open(&mut self, url: &str) -> Result<(), String> {
        let mut child = open_command(url)?
            .spawn()
            .map_err(|e| format!("xdg-open cannot run: {e}"))?;
        // Reaped on a thread of its own, so a browser that takes its time holds nothing up.
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

/// Watches the links of the user's Android through `connector` and opens each with `opener`,
/// until `stop` is set. A lost connection is made again; a link that fails the check is
/// dropped and logged.
pub fn follow(
    mut connector: Box<dyn Connector>,
    opener: &mut dyn LinkOpener,
    stop: Arc<AtomicBool>,
    log: Log,
) {
    while !stop.load(Ordering::Relaxed) {
        let stream = HostApiClient::connect(&mut *connector, BUILD, Duration::from_secs(5))
            .and_then(|client| client.watch_links());
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(why) => {
                log(&format!("links: cannot watch Android's links yet: {why}"));
                wait(&stop, RETRY);
                continue;
            }
        };
        while !stop.load(Ordering::Relaxed) {
            match stream.next(POLL) {
                Ok(None) => {}
                Ok(Some(url)) => match openable(&url) {
                    Ok(url) => {
                        if let Err(why) = opener.open(url) {
                            log(&format!("links: {why}"));
                        }
                    }
                    Err(why) => log(&format!("links: dropped a link from Android: {why}")),
                },
                Err(why) => {
                    log(&format!("links: the watch of Android's links ended: {why}"));
                    wait(&stop, RETRY);
                    break;
                }
            }
        }
    }
}

fn wait(stop: &AtomicBool, time: Duration) {
    let step = Duration::from_millis(50);
    let mut waited = Duration::ZERO;
    while waited < time && !stop.load(Ordering::Relaxed) {
        thread::sleep(step);
        waited += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr9_web_links_go_to_xdg_open_as_one_argument() {
        let command = open_command("https://example.com/a?q=1;rm$(id)").unwrap();
        assert_eq!(command.get_program(), "xdg-open");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn fr9_links_that_are_not_web_links_never_reach_xdg_open() {
        for url in [
            "file:///etc/passwd",
            "/usr/bin/xterm",
            "javascript:alert(1)",
            "--help",
            "smb://server/share",
        ] {
            assert!(open_command(url).is_err(), "{url} was let through");
        }
    }
}
