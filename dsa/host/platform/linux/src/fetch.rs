//! Downloads for image updates, through curl, which every distribution has and which uses the
//! system's certificate store and proxy settings.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use dsa_host_core::update::store::{Fetch, FileFetch};

/// Fetches `https:` URLs with curl, and `file:` URLs and paths directly.
#[derive(Default)]
pub struct CurlFetch;

/// curl's arguments for `url`: HTTPS only, following redirects within HTTPS, failing on an
/// HTTP error rather than saving the error page.
pub fn curl_args(url: &str) -> Vec<String> {
    [
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--retry",
        "3",
        "--connect-timeout",
        "30",
        "--",
        url,
    ]
    .map(str::to_string)
    .to_vec()
}

impl Fetch for CurlFetch {
    fn fetch(&self, url: &str, out: &mut dyn Write) -> Result<u64, String> {
        if !url.starts_with("https://") {
            return FileFetch.fetch(url, out);
        }
        let curl = crate::network::find_program("curl")
            .ok_or("curl is not installed, so DSA cannot download Android images")?;
        let mut child = Command::new(curl)
            .args(curl_args(url))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("running curl: {e}"))?;
        let mut body = child.stdout.take().ok_or("curl gave no output")?;
        let copied = std::io::copy(&mut body, out).map_err(|e| format!("downloading {url}: {e}"));
        let mut error = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut error);
        }
        let status = child.wait().map_err(|e| format!("running curl: {e}"))?;
        if !status.success() {
            return Err(format!("downloading {url}: {}", error.trim()));
        }
        copied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr14_images_are_downloaded_over_https_only() {
        let args = curl_args("https://example.invalid/releases.json");
        assert!(args.windows(2).any(|w| w == ["--proto", "=https"]));
        assert!(args.windows(2).any(|w| w == ["--proto-redir", "=https"]));
        assert_eq!(args.last().unwrap(), "https://example.invalid/releases.json");
    }
}
