//! The file server that shares the user's host folders with their Android, spoken to the way
//! the guest kernel's 9p file system speaks to it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dsa_host_core::file_share::ninep::{errno, Input, Output, Server};
use dsa_host_core::file_share::{
    serve_connection, share_proof, PlainShareHost, Sharing, SharingNow,
};
use dsa_protocol::auth::BootSecret;
use dsa_transport::memory;
use dsa_transport::{Connector, Listener};

const RLERROR: u8 = 7;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dsa-file-share-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Speaks 9P to a server, as the guest kernel does.
struct Guest<'a, 'b> {
    server: &'a mut Server<'b>,
    tag: u16,
}

impl Guest<'_, '_> {
    fn call(&mut self, kind: u8, body: Output) -> Result<Vec<u8>, u32> {
        self.tag += 1;
        let reply = self.server.handle(&body.message(kind, self.tag));
        let mut input = Input::new(&reply);
        let _size = input.u32().unwrap();
        let answered = input.u8().unwrap();
        assert_eq!(input.u16().unwrap(), self.tag);
        let rest = reply[7..].to_vec();
        if answered == RLERROR {
            let code = Input::new(&rest).u32().unwrap();
            Err(code)
        } else {
            assert_eq!(answered, kind + 1);
            Ok(rest)
        }
    }

    fn version(&mut self) {
        let mut body = Output::default();
        body.u32(65536);
        body.string("9P2000.L");
        let reply = self.call(100, body).unwrap();
        let mut input = Input::new(&reply);
        assert_eq!(input.u32().unwrap(), 65536);
        assert_eq!(input.string().unwrap(), "9P2000.L");
    }

    fn attach(&mut self, fid: u32, share: &str) -> Result<(), u32> {
        let mut body = Output::default();
        body.u32(fid);
        body.u32(u32::MAX);
        body.string("root");
        body.string(share);
        body.u32(0);
        self.call(104, body).map(|_| ())
    }

    /// Walks from `fid` to `newfid` through `names`; the number of names walked.
    fn walk(&mut self, fid: u32, newfid: u32, names: &[&str]) -> Result<u16, u32> {
        let mut body = Output::default();
        body.u32(fid);
        body.u32(newfid);
        body.u16(names.len() as u16);
        for name in names {
            body.string(name);
        }
        self.call(110, body)
            .map(|reply| Input::new(&reply).u16().unwrap())
    }

    fn lopen(&mut self, fid: u32, flags: u32) -> Result<(), u32> {
        let mut body = Output::default();
        body.u32(fid);
        body.u32(flags);
        self.call(12, body).map(|_| ())
    }

    fn lcreate(&mut self, fid: u32, name: &str) -> Result<(), u32> {
        let mut body = Output::default();
        body.u32(fid);
        body.string(name);
        body.u32(0o1 | 0o100);
        body.u32(0o644);
        body.u32(0);
        self.call(14, body).map(|_| ())
    }

    fn read(&mut self, fid: u32, offset: u64, count: u32) -> Result<Vec<u8>, u32> {
        let mut body = Output::default();
        body.u32(fid);
        body.u64(offset);
        body.u32(count);
        self.call(116, body).map(|reply| {
            let mut input = Input::new(&reply);
            let len = input.u32().unwrap() as usize;
            input.take(len).unwrap().to_vec()
        })
    }

    fn write(&mut self, fid: u32, offset: u64, data: &[u8]) -> Result<u32, u32> {
        let mut body = Output::default();
        body.u32(fid);
        body.u64(offset);
        body.u32(data.len() as u32);
        body.bytes(data);
        self.call(118, body)
            .map(|reply| Input::new(&reply).u32().unwrap())
    }

    fn readdir(&mut self, fid: u32) -> Vec<String> {
        let mut body = Output::default();
        body.u32(fid);
        body.u64(0);
        body.u32(8192);
        let reply = self.call(40, body).unwrap();
        let mut input = Input::new(&reply);
        let len = input.u32().unwrap() as usize;
        let data = input.take(len).unwrap();
        let mut entries = Input::new(data);
        let mut names = Vec::new();
        while entries.take(13).is_ok() {
            entries.u64().unwrap();
            entries.u8().unwrap();
            names.push(entries.string().unwrap());
        }
        names
    }

    fn clunk(&mut self, fid: u32) {
        let mut body = Output::default();
        body.u32(fid);
        self.call(120, body).unwrap();
    }
}

fn plain(dir: &Path) -> PlainShareHost {
    PlainShareHost {
        folders: vec![("Documents".into(), dir.to_path_buf())],
        location: "Host".into(),
    }
}

#[test]
fn fr29_the_guest_reads_and_writes_the_files_of_a_shared_folder() {
    let dir = scratch("read-write");
    std::fs::write(dir.join("note.txt"), b"from the host").unwrap();
    std::fs::create_dir(dir.join("Work")).unwrap();
    let roots = |name: &str| (name == "Documents").then(|| dir.clone());
    let shared = |_: &str| true;
    let space = |_: &Path| Some((1 << 30, 1 << 29));
    let mut server = Server::new(&roots, &shared, &space);
    let mut guest = Guest {
        server: &mut server,
        tag: 0,
    };
    guest.version();
    guest.attach(1, "Documents").unwrap();
    assert_eq!(guest.walk(1, 2, &["note.txt"]), Ok(1));
    guest.lopen(2, 0).unwrap();
    assert_eq!(guest.read(2, 0, 4096).unwrap(), b"from the host");
    guest.clunk(2);

    guest.walk(1, 3, &["Work"]).unwrap();
    guest.lcreate(3, "made in android.txt").unwrap();
    assert_eq!(guest.write(3, 0, b"from the guest"), Ok(14));
    guest.clunk(3);
    assert_eq!(
        std::fs::read(dir.join("Work").join("made in android.txt")).unwrap(),
        b"from the guest"
    );

    guest.walk(1, 4, &[]).unwrap();
    guest.lopen(4, 0).unwrap();
    let names = guest.readdir(4);
    assert!(names.contains(&"note.txt".to_string()), "{names:?}");
    assert!(names.contains(&"Work".to_string()), "{names:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fr29_a_walk_out_of_the_folder_with_dotdot_or_a_separator_is_refused() {
    let base = scratch("dotdot");
    let dir = base.join("Documents");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(base.join("secret.txt"), b"not shared").unwrap();
    let roots = |_: &str| Some(dir.clone());
    let shared = |_: &str| true;
    let space = |_: &Path| None;
    let mut server = Server::new(&roots, &shared, &space);
    let mut guest = Guest {
        server: &mut server,
        tag: 0,
    };
    guest.version();
    guest.attach(1, "Documents").unwrap();
    assert_eq!(guest.walk(1, 2, &[".."]), Err(errno::EACCES));
    assert_eq!(guest.walk(1, 2, &["..", "secret.txt"]), Err(errno::EACCES));
    assert!(guest.walk(1, 2, &["../secret.txt"]).is_err());
    assert!(guest.walk(1, 2, &["..\\secret.txt"]).is_err());
    assert!(guest.lcreate(1, "../escaped.txt").is_err());
    assert!(!base.join("escaped.txt").exists());
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[test]
fn fr29_a_symbolic_link_out_of_the_folder_is_refused() {
    let base = scratch("symlink");
    let dir = base.join("Documents");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(base.join("secret.txt"), b"not shared").unwrap();
    std::fs::write(dir.join("inside.txt"), b"shared").unwrap();
    std::os::unix::fs::symlink(base.join("secret.txt"), dir.join("way out")).unwrap();
    std::os::unix::fs::symlink(&base, dir.join("up")).unwrap();
    std::os::unix::fs::symlink(dir.join("inside.txt"), dir.join("way in")).unwrap();
    let roots = |_: &str| Some(dir.clone());
    let shared = |_: &str| true;
    let space = |_: &Path| None;
    let mut server = Server::new(&roots, &shared, &space);
    let mut guest = Guest {
        server: &mut server,
        tag: 0,
    };
    guest.version();
    guest.attach(1, "Documents").unwrap();
    assert_eq!(guest.walk(1, 2, &["way out"]), Err(errno::EACCES));
    assert_eq!(guest.walk(1, 2, &["up"]), Err(errno::EACCES));
    assert_eq!(guest.walk(1, 2, &["way in"]), Ok(1));
    guest.walk(1, 3, &[]).unwrap();
    guest.lopen(3, 0).unwrap();
    let names = guest.readdir(3);
    assert!(!names.contains(&"way out".to_string()), "{names:?}");
    assert!(!names.contains(&"up".to_string()), "{names:?}");
    assert!(names.contains(&"way in".to_string()), "{names:?}");
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(windows)]
#[test]
fn fr29_a_symbolic_link_out_of_the_folder_is_refused() {
    let base = scratch("symlink");
    let dir = base.join("Documents");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(base.join("secret.txt"), b"not shared").unwrap();
    // Making a link needs developer mode or an administrator on Windows; without either there
    // is no link to refuse.
    if std::os::windows::fs::symlink_file(base.join("secret.txt"), dir.join("way out")).is_err() {
        let _ = std::fs::remove_dir_all(&base);
        return;
    }
    let roots = |_: &str| Some(dir.clone());
    let shared = |_: &str| true;
    let space = |_: &Path| None;
    let mut server = Server::new(&roots, &shared, &space);
    let mut guest = Guest {
        server: &mut server,
        tag: 0,
    };
    guest.version();
    guest.attach(1, "Documents").unwrap();
    assert_eq!(guest.walk(1, 2, &["way out"]), Err(errno::EACCES));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn fr29_a_folder_that_is_not_shared_cannot_be_attached_and_closes_when_sharing_stops() {
    let dir = scratch("unshared");
    std::fs::write(dir.join("a.txt"), b"a").unwrap();
    let sharing = Mutex::new(true);
    let roots = |name: &str| (name == "Documents").then(|| dir.clone());
    let shared = |_: &str| *sharing.lock().unwrap();
    let space = |_: &Path| None;
    let mut server = Server::new(&roots, &shared, &space);
    let mut guest = Guest {
        server: &mut server,
        tag: 0,
    };
    guest.version();
    assert_eq!(guest.attach(1, "Pictures"), Err(errno::EACCES));
    guest.attach(1, "Documents").unwrap();
    guest.walk(1, 2, &["a.txt"]).unwrap();
    *sharing.lock().unwrap() = false;
    assert_eq!(guest.lopen(2, 0), Err(errno::EACCES));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fr29_a_connection_is_served_only_after_the_guest_proves_the_boot_secret() {
    let dir = scratch("proof");
    std::fs::write(dir.join("a.txt"), b"shared").unwrap();
    let secret = BootSecret::from_bytes([3; 32]);
    let sharing: SharingNow = Arc::new(Mutex::new(Sharing {
        on: true,
        folders: vec!["Documents".into()],
    }));
    let host = plain(&dir);
    let (mut listener, mut connector) = memory::pair();

    // A guest that does not know the secret is turned away.
    let mut stranger = connector.connect(Duration::from_secs(5)).unwrap();
    let accepted = listener.accept(Duration::from_secs(5)).unwrap();
    std::thread::scope(|scope| {
        let served =
            scope.spawn(|| serve_connection(accepted, &host, &sharing, &secret, &|| false));
        let nonce = stranger.recv(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(nonce.len(), 32);
        stranger.send(&[0u8; 32]).unwrap();
        assert!(served.join().unwrap().is_err());
    });

    // The guest's daemon gets the files.
    let mut daemon = connector.connect(Duration::from_secs(5)).unwrap();
    let accepted = listener.accept(Duration::from_secs(5)).unwrap();
    std::thread::scope(|scope| {
        let served =
            scope.spawn(|| serve_connection(accepted, &host, &sharing, &secret, &|| false));
        let nonce = daemon.recv(Some(Duration::from_secs(5))).unwrap();
        let mut version = Output::default();
        version.u32(8192);
        version.string("9P2000.L");
        // The proof and the first request may arrive in one piece, as on a real socket.
        daemon
            .send(
                &[
                    share_proof(&secret, &nonce).to_vec(),
                    version.message(100, 1),
                ]
                .concat(),
            )
            .unwrap();
        let reply = daemon.recv(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(reply[4], 101);
        daemon.close();
        assert!(served.join().unwrap().is_ok());
    });
    let _ = std::fs::remove_dir_all(&dir);
}
