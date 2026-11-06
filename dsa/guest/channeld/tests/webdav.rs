//! Android's shared storage as the host's file manager sees it: WebDAV over a stream.

use std::io::{self, Cursor, Read, Write};
use std::path::PathBuf;

use dsa_guest_channeld::webdav::{serve, Config};

/// A client's requests in, the server's answers out.
struct Duplex {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}

impl Read for Duplex {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf)
    }
}

impl Write for Duplex {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn storage(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dsa-webdav-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Storage").join("Pictures")).unwrap();
    std::fs::create_dir_all(dir.join("Storage").join("Host").join("Documents")).unwrap();
    std::fs::write(
        dir.join("Storage").join("Pictures").join("cat.jpg"),
        b"meow",
    )
    .unwrap();
    std::fs::write(dir.join("secret.txt"), b"not android's").unwrap();
    dir
}

fn config(dir: &std::path::Path) -> Config {
    Config {
        root: dir.join("Storage"),
        token: "tok".into(),
        hidden: vec!["Host".into()],
    }
}

/// Sends `requests` in one go and returns everything the server answered.
fn exchange(config: &Config, requests: &[u8]) -> String {
    let stream = Duplex {
        input: Cursor::new(requests.to_vec()),
        output: Vec::new(),
    };
    let mut answered = Vec::new();
    {
        struct Tee<'a>(Duplex, &'a mut Vec<u8>);
        impl Read for Tee<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.0.read(buf)
            }
        }
        impl Write for Tee<'_> {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.1.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        serve(Tee(stream, &mut answered), config).unwrap();
    }
    String::from_utf8_lossy(&answered).into_owned()
}

#[test]
fn fr29_the_host_lists_reads_and_writes_androids_shared_storage() {
    let dir = storage("list");
    let config = config(&dir);
    let answered = exchange(
        &config,
        b"PROPFIND /tok/ HTTP/1.1\r\nDepth: 1\r\nContent-Length: 0\r\n\r\n\
          GET /tok/Pictures/cat.jpg HTTP/1.1\r\n\r\n\
          PUT /tok/Pictures/new%20note.txt HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello\
          MKCOL /tok/Made HTTP/1.1\r\n\r\n",
    );
    assert!(answered.starts_with("HTTP/1.1 207"), "{answered}");
    assert!(
        answered.contains("<D:href>/tok/Pictures/</D:href>"),
        "{answered}"
    );
    assert!(
        !answered.contains("Host"),
        "the host's own folders are left out: {answered}"
    );
    assert!(answered.contains("HTTP/1.1 200 OK\r\n"), "{answered}");
    assert!(answered.contains("\r\n\r\nmeow"), "{answered}");
    assert!(answered.contains("HTTP/1.1 201 Created"), "{answered}");
    assert_eq!(
        std::fs::read(dir.join("Storage").join("Pictures").join("new note.txt")).unwrap(),
        b"hello"
    );
    assert!(dir.join("Storage").join("Made").is_dir());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fr29_requests_out_of_androids_storage_are_refused() {
    let dir = storage("refused");
    let config = config(&dir);
    for request in [
        "GET /tok/../secret.txt HTTP/1.1\r\n\r\n",
        "GET /tok/%2E%2E/secret.txt HTTP/1.1\r\n\r\n",
        "GET /other/Pictures/cat.jpg HTTP/1.1\r\n\r\n",
        "GET /tok/Host/Documents HTTP/1.1\r\n\r\n",
        "DELETE /tok/ HTTP/1.1\r\n\r\n",
    ] {
        let answered = exchange(&config, request.as_bytes());
        assert!(
            answered.starts_with("HTTP/1.1 403") || answered.starts_with("HTTP/1.1 404"),
            "{request:?} was answered {answered}"
        );
    }
    let moved = exchange(
        &config,
        b"MOVE /tok/Pictures/cat.jpg HTTP/1.1\r\nDestination: http://127.0.0.1:1/tok/../cat.jpg\r\n\r\n",
    );
    assert!(moved.starts_with("HTTP/1.1 403"), "{moved}");
    assert!(dir
        .join("Storage")
        .join("Pictures")
        .join("cat.jpg")
        .is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn fr29_a_symbolic_link_out_of_androids_storage_is_refused() {
    let dir = storage("symlink");
    let config = config(&dir);
    std::os::unix::fs::symlink(dir.join("secret.txt"), dir.join("Storage").join("way out"))
        .unwrap();
    let answered = exchange(&config, b"GET /tok/way%20out HTTP/1.1\r\n\r\n");
    assert!(answered.starts_with("HTTP/1.1 403"), "{answered}");
    let listed = exchange(&config, b"PROPFIND /tok/ HTTP/1.1\r\nDepth: 1\r\n\r\n");
    assert!(!listed.contains("way%20out"), "{listed}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Explorer reaches `\\127.0.0.1@<port>\<token>` through Windows' WebDAV redirector, which
/// looks at the server's root and at the share without a trailing slash, at depth 0, before
/// it lists anything. A 403 to either makes Explorer give up and open another folder.
#[test]
fn fr29_the_webdav_redirectors_first_requests_are_answered() {
    let dir = storage("redirector");
    let config = config(&dir);
    let root = exchange(
        &config,
        b"PROPFIND / HTTP/1.1\r\nDepth: 0\r\nContent-Length: 0\r\n\r\n",
    );
    assert!(root.starts_with("HTTP/1.1 207"), "{root}");
    assert!(!root.contains("tok"), "the root names no token: {root}");
    let share = exchange(
        &config,
        b"PROPFIND /tok HTTP/1.1\r\nDepth: 0\r\nContent-Length: 0\r\n\r\n",
    );
    assert!(share.starts_with("HTTP/1.1 207"), "{share}");
    assert!(share.contains("<D:collection/>"), "{share}");
    let listed = exchange(
        &config,
        b"PROPFIND /tok/ HTTP/1.1\r\nDepth: 1\r\nContent-Length: 0\r\n\r\n",
    );
    assert!(listed.starts_with("HTTP/1.1 207"), "{listed}");
    assert!(listed.contains("/tok/Pictures/"), "{listed}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// On Android the server runs in a sandbox that may not look at `/data` or `/proc`. Resolving
/// a path through the C library's `realpath` needs both, so every request was refused with
/// 403. Here the storage sits under a folder the server may pass through but not read, the
/// nearest a test can come to that sandbox, and a link inside the storage is followed by hand.
#[cfg(unix)]
#[test]
fn fr29_androids_storage_is_served_without_looking_above_it() {
    use std::os::unix::fs::PermissionsExt;
    let dir = storage("above");
    std::os::unix::fs::symlink("Pictures", dir.join("Storage").join("Photos")).unwrap();
    let config = config(&dir);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o100)).unwrap();
    let listed = exchange(
        &config,
        b"PROPFIND /tok/Photos/ HTTP/1.1\r\nDepth: 1\r\nContent-Length: 0\r\n\r\n",
    );
    let read = exchange(&config, b"GET /tok/Photos/cat.jpg HTTP/1.1\r\n\r\n");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(listed.starts_with("HTTP/1.1 207"), "{listed}");
    assert!(listed.contains("cat.jpg"), "{listed}");
    assert!(read.contains("\r\n\r\nmeow"), "{read}");
    let _ = std::fs::remove_dir_all(&dir);
}
