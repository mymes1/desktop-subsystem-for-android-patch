//! Sharing the user's host folders with their Android, and showing Android's shared storage on
//! the host.
//!
//! Folders go to the guest over a file server of the host's own: 9P2000.L, the protocol the
//! Linux kernel's `9p` file system speaks, on a vsock port ([`SHARE_PORT`]). The guest's control
//! daemon connects once per folder, proves it knows the boot's secret ([`share_proof`]), and
//! hands the connection to the kernel, which mounts it under Android's shared storage. Every
//! request is checked against the folder it attached to, so no path leaves that folder: names
//! with a separator, `..` and symbolic links that point outside are refused ([`ninep`]).
//!
//! The server touches the user's files only through the platform's [`ShareHost`], which runs
//! it with the signed-in user's own rights, so what the host's file system lets that user do is
//! what the guest may do, and nothing more.
//!
//! The other way, the guest's daemon serves Android's shared storage as WebDAV on its vsock
//! port [`ANDROID_FILES_PORT`], and the host bridges a loopback port to it, which the host's
//! file manager opens. Each session gets its own token, the first segment of every path, so
//! only someone the service told the address can read the files.

pub mod ninep;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dsa_protocol::auth::BootSecret;
use dsa_transport::{Connection, Listener, TransportError};

/// The vsock port of the host's file server.
pub const SHARE_PORT: u32 = 5590;

/// The vsock port the guest serves its shared storage on.
pub const ANDROID_FILES_PORT: u32 = 5591;

/// The folders a user can share, by the names the guest shows them under, in the order a
/// client lists them.
pub const FOLDERS: [&str; 6] = [
    "Documents",
    "Downloads",
    "Pictures",
    "Music",
    "Videos",
    "Desktop",
];

/// How long a connection may take to prove it comes from the guest's daemon.
const PROOF_WAIT: Duration = Duration::from_secs(10);

/// What a connection's proof is computed over, ahead of the host's nonce, so a proof made for
/// the control channel never passes here and the reverse.
const PROOF_CONTEXT: &[u8] = b"file-share:";

/// Every folder, as the default setting has them.
pub fn all_folders() -> Vec<String> {
    FOLDERS.iter().map(|name| name.to_string()).collect()
}

/// The folder names in `names` that are known, spelt as [`FOLDERS`] spells them, each once,
/// in the order of [`FOLDERS`].
pub fn known_folders(names: &[String]) -> Vec<String> {
    FOLDERS
        .iter()
        .filter(|known| {
            names
                .iter()
                .any(|name| name.trim().eq_ignore_ascii_case(known))
        })
        .map(|known| known.to_string())
        .collect()
}

/// Checks a list of folders a client sent: each must be one of [`FOLDERS`], in any case. The
/// result has each once, in the order of [`FOLDERS`].
pub fn normalise_folders(names: &[String]) -> Result<Vec<String>, String> {
    for name in names {
        if !FOLDERS
            .iter()
            .any(|known| name.trim().eq_ignore_ascii_case(known))
        {
            return Err(format!(
                "{name:?} is not a folder that can be shared; use any of {}",
                FOLDERS.join(", ")
            ));
        }
    }
    Ok(known_folders(names))
}

/// The proof the guest sends for `nonce`: the HMAC-SHA256 of [`PROOF_CONTEXT`] and the nonce,
/// keyed with the boot's secret.
pub fn share_proof(secret: &BootSecret, nonce: &[u8]) -> [u8; 32] {
    secret.proof(&[PROOF_CONTEXT, nonce].concat())
}

/// Whether `proof` is the guest's proof for `nonce`.
pub fn proof_matches(secret: &BootSecret, nonce: &[u8], proof: &[u8]) -> bool {
    secret.verify(&[PROOF_CONTEXT, nonce].concat(), proof)
}

/// What the platform gives the file server: where each folder is, and the user's rights.
pub trait ShareHost: Send + Sync {
    /// The host folder shared under `name` (one of [`FOLDERS`]), if the user has one.
    fn folder(&self, name: &str) -> Option<PathBuf>;
    /// Runs `serve` with the signed-in user's own rights, so the host's file system decides
    /// what they may read and write. An error means it could not, and `serve` did not run.
    fn as_user(&self, serve: &mut dyn FnMut()) -> Result<(), String>;
    /// The size of the volume `path` is on and the bytes free on it, if known.
    fn space(&self, path: &Path) -> Option<(u64, u64)> {
        let _ = path;
        None
    }
    /// The folder in Android's shared storage the shared folders go under, such as "Windows".
    fn location(&self) -> String;
}

/// A [`ShareHost`] over plain folders with the process's own rights, for tests and for hosts
/// whose service runs as the user.
pub struct PlainShareHost {
    pub folders: Vec<(String, PathBuf)>,
    pub location: String,
}

impl ShareHost for PlainShareHost {
    fn folder(&self, name: &str) -> Option<PathBuf> {
        self.folders
            .iter()
            .find(|(folder, _)| folder == name)
            .map(|(_, path)| path.clone())
    }

    fn as_user(&self, serve: &mut dyn FnMut()) -> Result<(), String> {
        serve();
        Ok(())
    }

    fn location(&self) -> String {
        self.location.clone()
    }
}

/// Which folders are shared now, as the user's setting says. The server reads it at every
/// request, so a folder the user stops sharing is closed to the guest at once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sharing {
    pub on: bool,
    pub folders: Vec<String>,
}

impl Sharing {
    /// Whether `name` is shared now.
    pub fn shares(&self, name: &str) -> bool {
        self.on && self.folders.iter().any(|folder| folder == name)
    }
}

/// The sharing setting as the file server reads it.
pub type SharingNow = Arc<Mutex<Sharing>>;

/// Serves the file server on `listener` until `stop` says so: each connection that proves it
/// comes from the guest's daemon (with the boot's secret that `secret` gives) gets a 9P server
/// of its own, on a thread of its own, run with the user's rights. A connection that fails or
/// breaks the rules is closed; the others and the session go on.
pub fn serve_shares(
    mut listener: Box<dyn Listener>,
    host: Arc<dyn ShareHost>,
    sharing: SharingNow,
    secret: Arc<dyn Fn() -> Option<BootSecret> + Send + Sync>,
    stop: Arc<dyn Fn() -> bool + Send + Sync>,
) {
    let mut reported = false;
    while !stop() {
        let connection = match listener.accept(Duration::from_millis(200)) {
            Ok(connection) => connection,
            Err(TransportError::Timeout) => continue,
            Err(error) => {
                if !reported {
                    eprintln!("file sharing: accepting on the file server's port failed: {error}");
                    reported = true;
                }
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        reported = false;
        let Some(secret) = secret() else {
            let mut connection = connection;
            connection.close();
            continue;
        };
        let (host, sharing, stop) = (Arc::clone(&host), Arc::clone(&sharing), Arc::clone(&stop));
        let spawned = thread::Builder::new()
            .name("dsa-file-share".into())
            .spawn(move || {
                let served = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    serve_connection(connection, &*host, &sharing, &secret, &*stop)
                }));
                match served {
                    Ok(Ok(())) => {}
                    Ok(Err(why)) => eprintln!("file sharing: closed a connection: {why}"),
                    Err(_) => eprintln!(
                        "file sharing: a connection failed; the other folders and Android go on"
                    ),
                }
            });
        if let Err(error) = spawned {
            eprintln!("file sharing: a connection cannot be served: {error}");
        }
    }
}

/// Serves one connection: the proof, then 9P until the guest closes it.
pub fn serve_connection(
    mut connection: Box<dyn Connection>,
    host: &dyn ShareHost,
    sharing: &SharingNow,
    secret: &BootSecret,
    stop: &dyn Fn() -> bool,
) -> Result<(), String> {
    let nonce = dsa_protocol::auth::nonce()?;
    connection
        .send(&nonce)
        .map_err(|e| format!("sending the nonce: {e}"))?;
    let mut bytes = ByteReader::new(connection.as_mut());
    let proof = bytes
        .take(32, Some(Instant::now() + PROOF_WAIT), stop)
        .map_err(|e| format!("no proof arrived: {e}"))?;
    if !proof_matches(secret, &nonce, &proof) {
        bytes.connection.close();
        return Err("it did not prove it comes from Android's own daemon".into());
    }
    let roots = |name: &str| {
        if sharing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .shares(name)
        {
            host.folder(name)
        } else {
            None
        }
    };
    let still_shared = |name: &str| {
        sharing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .shares(name)
    };
    let space = |path: &Path| host.space(path);
    let mut outcome = Ok(());
    host.as_user(&mut || {
        let mut server = ninep::Server::new(&roots, &still_shared, &space);
        outcome = serve_requests(&mut bytes, &mut server, stop);
    })?;
    bytes.connection.close();
    outcome
}

/// Answers 9P requests until the guest closes the connection.
fn serve_requests(
    bytes: &mut ByteReader<'_>,
    server: &mut ninep::Server<'_>,
    stop: &dyn Fn() -> bool,
) -> Result<(), String> {
    loop {
        let header = match bytes.take(4, None, stop) {
            Ok(header) => header,
            Err(TransportError::Closed) => return Ok(()),
            Err(error) => return Err(format!("reading a request: {error}")),
        };
        let size = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        if !(7..=ninep::MAX_MESSAGE as usize).contains(&size) {
            return Err(format!("it sent a request of {size} bytes"));
        }
        let rest = match bytes.take(size - 4, None, stop) {
            Ok(rest) => rest,
            Err(TransportError::Closed) => return Ok(()),
            Err(error) => return Err(format!("reading a request: {error}")),
        };
        let request = [header, rest].concat();
        let reply = server.handle(&request);
        match bytes.connection.send(&reply) {
            Ok(()) => {}
            Err(TransportError::Closed) => return Ok(()),
            Err(error) => return Err(format!("answering: {error}")),
        }
    }
}

/// Reads exact byte counts from a connection whose frames are arbitrary pieces of a stream.
struct ByteReader<'a> {
    connection: &'a mut dyn Connection,
    pending: Vec<u8>,
}

impl<'a> ByteReader<'a> {
    fn new(connection: &'a mut dyn Connection) -> ByteReader<'a> {
        ByteReader {
            connection,
            pending: Vec::new(),
        }
    }

    /// The next `count` bytes, waiting until `deadline` if there is one, else until they come
    /// or `stop` says so.
    fn take(
        &mut self,
        count: usize,
        deadline: Option<Instant>,
        stop: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, TransportError> {
        while self.pending.len() < count {
            if stop() {
                return Err(TransportError::Closed);
            }
            let wait = match deadline {
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(TransportError::Timeout);
                    }
                    left.min(Duration::from_millis(500))
                }
                None => Duration::from_millis(500),
            };
            match self.connection.recv(Some(wait)) {
                Ok(piece) => self.pending.extend_from_slice(&piece),
                Err(TransportError::Timeout) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(self.pending.drain(..count).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr29_folder_names_are_checked_and_put_in_order() {
        assert_eq!(
            normalise_folders(&["videos".into(), "Documents".into(), "VIDEOS".into()]).unwrap(),
            ["Documents", "Videos"]
        );
        assert!(normalise_folders(&["..".into()]).is_err());
        assert!(normalise_folders(&["C:\\Users".into()]).is_err());
        assert_eq!(normalise_folders(&[]).unwrap(), Vec::<String>::new());
        assert_eq!(all_folders().len(), FOLDERS.len());
    }

    #[test]
    fn fr29_a_share_proof_is_bound_to_the_secret_and_the_nonce() {
        let secret = BootSecret::from_bytes([7; 32]);
        let other = BootSecret::from_bytes([8; 32]);
        let proof = share_proof(&secret, b"nonce");
        assert!(proof_matches(&secret, b"nonce", &proof));
        assert!(!proof_matches(&secret, b"other", &proof));
        assert!(!proof_matches(&other, b"nonce", &proof));
        // A proof for the control channel's handshake does not pass here.
        assert!(!proof_matches(&secret, b"nonce", &secret.proof(b"nonce")));
    }

    #[test]
    fn fr29_only_named_folders_are_shared_and_none_while_off() {
        let sharing = Sharing {
            on: true,
            folders: vec!["Pictures".into()],
        };
        assert!(sharing.shares("Pictures"));
        assert!(!sharing.shares("Documents"));
        let off = Sharing {
            on: false,
            ..sharing
        };
        assert!(!off.shares("Pictures"));
    }
}
