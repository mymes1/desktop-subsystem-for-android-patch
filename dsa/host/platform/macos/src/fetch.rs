//! Downloads for image updates on macOS, through the system's `curl`.
//!
//! Every Mac has `/usr/bin/curl`, which uses the system's certificate store, so the host
//! needs no TLS library of its own. What is downloaded is checked against the signed
//! release list by the update store, whatever fetched it.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use dsa_host_core::update::store::{Fetch, FileFetch};

/// Fetches `https:` URLs with curl, and `file:` URLs and paths directly.
pub struct CurlFetch;

impl Fetch for CurlFetch {
    fn fetch(&self, url: &str, out: &mut dyn Write) -> Result<u64, String> {
        if !url.starts_with("https://") && !url.starts_with("http://") {
            return FileFetch.fetch(url, out);
        }
        let mut child = Command::new("/usr/bin/curl")
            .args(["--fail", "--silent", "--show-error", "--location", "--proto", "=https,http"])
            .arg(url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("running curl for {url}: {e}"))?;
        let mut stdout = child.stdout.take().ok_or("curl gave no output")?;
        let mut written = 0u64;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = stdout
                .read(&mut buf)
                .map_err(|e| format!("reading {url}: {e}"))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .map_err(|e| format!("writing what {url} sent: {e}"))?;
            written += n as u64;
        }
        let output = child
            .wait_with_output()
            .map_err(|e| format!("waiting for curl: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "downloading {url}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(written)
    }
}
