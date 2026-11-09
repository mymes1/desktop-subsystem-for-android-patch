//! Address lookups for the apps in the user's Android.
//!
//! The Linux desktop has no geocoder of its own to ask: GeoClue finds positions, not
//! addresses. The follower asks OpenStreetMap's Nominatim instead
//! ([`dsa_host_core::geocode::Nominatim`]), which needs no key, through `curl`, which every
//! distribution has and which follows the system's certificates and proxy settings. It names
//! itself to the server, as the server's usage policy asks, and sends one lookup at a time.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use dsa_host_core::geocode::{self, Nominatim};
use dsa_host_core::update::store::Fetch;
use dsa_transport::Connector;

use super::Log;

/// The build the follower names itself by.
const BUILD: &str = "dsa-devices geocode";

/// The longest one lookup may take, in seconds. An app waits for the answer.
const LOOKUP_SECONDS: &str = "15";

/// curl's arguments for one lookup of `url`: HTTPS only (or plain HTTP for a server the user
/// named on their own network), failing on an HTTP error, and naming DSA as the client.
pub fn curl_args(url: &str) -> Vec<String> {
    let agent = format!(
        "DSA/{} (Desktop Subsystem for Android; +https://github.com/thisisthepy/desktop-subsystem-for-android)",
        crate::VERSION
    );
    [
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https,http",
        "--max-time",
        LOOKUP_SECONDS,
        "--user-agent",
        agent.as_str(),
        "--",
        url,
    ]
    .map(str::to_string)
    .to_vec()
}

/// Fetches a lookup's answer with curl.
pub struct CurlLookup;

impl Fetch for CurlLookup {
    fn fetch(&self, url: &str, out: &mut dyn Write) -> Result<u64, String> {
        if !url.starts_with("https://") && !url.starts_with("http://") {
            return Err(format!("{url} is not a web address"));
        }
        let curl = crate::network::find_program("curl")
            .ok_or("curl is not installed, so addresses cannot be looked up")?;
        let mut child = Command::new(curl)
            .args(curl_args(url))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("running curl: {e}"))?;
        let mut body = child.stdout.take().ok_or("curl gave no output")?;
        let copied = std::io::copy(&mut body, out).map_err(|e| format!("reading {url}: {e}"));
        let mut error = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut error);
        }
        let status = child.wait().map_err(|e| format!("running curl: {e}"))?;
        if !status.success() {
            return Err(format!("{url}: {}", error.trim()));
        }
        copied
    }
}

/// Answers the address lookups of the user's Android until `stop`.
pub fn serve(mut connector: Box<dyn Connector>, stop: Arc<AtomicBool>, log: Log) {
    let mut geocoder = Nominatim::from_env(CurlLookup);
    let line = move |line: &str| log(line);
    geocode::serve(&mut *connector, BUILD, &mut geocoder, &stop, &line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr36_a_lookup_names_dsa_and_takes_no_other_scheme() {
        let args = curl_args("https://nominatim.openstreetmap.org/search?q=x");
        let agent = args.iter().position(|a| a == "--user-agent").unwrap();
        assert!(args[agent + 1].starts_with("DSA/"));
        assert_eq!(
            args.last().unwrap(),
            "https://nominatim.openstreetmap.org/search?q=x"
        );
        assert_eq!(args[args.len() - 2], "--");
        assert!(CurlLookup
            .fetch("file:///etc/passwd", &mut Vec::new())
            .is_err());
    }
}
