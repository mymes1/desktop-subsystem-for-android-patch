//! Android's shared storage, served to the host as WebDAV, so the user sees it in the host's
//! file manager.
//!
//! The host bridges a loopback port to this server and gives the user an address whose first
//! path segment is a token of the session; a request with another first segment is refused.
//! Every other segment is checked on its own (no `.`, `..`, separators or NUL), and every
//! path is resolved before it is used, so neither a name nor a symbolic link leads outside the
//! storage. The folder the host's own folders are mounted under is left out, so the host never
//! sees its own files again through Android.
//!
//! It speaks what the host's WebDAV client asks for: OPTIONS, PROPFIND (depth 0 and 1), GET,
//! HEAD, PUT, DELETE, MKCOL, COPY, MOVE, PROPPATCH (taken, nothing kept), and LOCK and UNLOCK
//! (granted, since only one client ever writes). Nothing here knows about sockets: it runs on
//! any stream, so it is tested on any platform.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The longest request head taken: request line and headers.
const MAX_HEAD: usize = 64 * 1024;

/// The longest request body read into memory (PROPFIND, PROPPATCH, LOCK). A file's content
/// goes straight to the file instead.
const MAX_XML_BODY: u64 = 1024 * 1024;

/// What the server serves.
#[derive(Debug, Clone)]
pub struct Config {
    /// The storage, such as `/data/media/0`.
    pub root: PathBuf,
    /// The first segment of every path.
    pub token: String,
    /// Names at the top of the storage that are not served.
    pub hidden: Vec<String>,
}

/// One request's head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Serves requests on `stream` until the client closes it or a request breaks the rules.
pub fn serve(stream: impl Read + Write, config: &Config) -> io::Result<()> {
    let mut stream = BufReader::new(stream);
    loop {
        let Some(request) = read_head(&mut stream)? else {
            return Ok(());
        };
        let close = request
            .header("Connection")
            .is_some_and(|value| value.eq_ignore_ascii_case("close"));
        let mut body = Body::of(&request, &mut stream)?;
        let response = handle(&request, &mut body, config);
        // A body the handler did not read is skipped, so the next request starts where it
        // should.
        body.drain()?;
        response.write_to(stream.get_mut(), request.method == "HEAD")?;
        if close {
            return Ok(());
        }
    }
}

/// Reads one request head; `None` when the client closed the connection between requests.
pub fn read_head(stream: &mut impl BufRead) -> io::Result<Option<Request>> {
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let read = stream.read_until(b'\n', &mut line)?;
        if read == 0 {
            if head.is_empty() {
                return Ok(None);
            }
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if head.is_empty() && (line == b"\r\n" || line == b"\n") {
            // Blank lines before a request are allowed.
            continue;
        }
        head.extend_from_slice(&line);
        if head.len() > MAX_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too long",
            ));
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "no request line",
        ));
    };
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    // An absolute target names the host too; only the path counts.
    let path = target
        .strip_prefix("http://")
        .and_then(|rest| rest.find('/').map(|at| &rest[at..]))
        .unwrap_or(target);
    let path = path.split('?').next().unwrap_or_default().to_string();
    Ok(Some(Request {
        method: method.to_ascii_uppercase(),
        path,
        headers,
    }))
}

/// A request's body, read on demand: by its length, or in chunks.
pub struct Body<'a> {
    stream: &'a mut dyn BufRead,
    left: u64,
    chunked: bool,
    done: bool,
}

impl<'a> Body<'a> {
    fn of(request: &Request, stream: &'a mut dyn BufRead) -> io::Result<Body<'a>> {
        let chunked = request
            .header("Transfer-Encoding")
            .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"));
        let left = match request.header("Content-Length") {
            Some(value) if !chunked => value
                .parse::<u64>()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad Content-Length"))?,
            _ => 0,
        };
        Ok(Body {
            stream,
            left,
            chunked,
            done: !chunked && left == 0,
        })
    }

    /// An empty body, for tests.
    pub fn empty(stream: &'a mut dyn BufRead) -> Body<'a> {
        Body {
            stream,
            left: 0,
            chunked: false,
            done: true,
        }
    }

    /// The next piece of the body; empty at its end.
    fn piece(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        if self.chunked && self.left == 0 {
            let mut line = String::new();
            self.stream.read_line(&mut line)?;
            if line.trim().is_empty() {
                line.clear();
                self.stream.read_line(&mut line)?;
            }
            let size = line.trim().split(';').next().unwrap_or_default();
            self.left = u64::from_str_radix(size, 16)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad chunk size"))?;
            if self.left == 0 {
                // The trailer, up to its blank line.
                loop {
                    line.clear();
                    if self.stream.read_line(&mut line)? == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                self.done = true;
                return Ok(0);
            }
        }
        let want = buffer.len().min(self.left.min(usize::MAX as u64) as usize);
        let read = self.stream.read(&mut buffer[..want])?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.left -= read as u64;
        if !self.chunked && self.left == 0 {
            self.done = true;
        }
        Ok(read)
    }

    /// The whole body, when it is small enough to hold.
    fn text(&mut self) -> io::Result<String> {
        let mut all = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let read = self.piece(&mut buffer)?;
            if read == 0 {
                break;
            }
            all.extend_from_slice(&buffer[..read]);
            if all.len() as u64 > MAX_XML_BODY {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "body too long"));
            }
        }
        Ok(String::from_utf8_lossy(&all).into_owned())
    }

    /// Copies the body into `out`.
    fn copy_to(&mut self, out: &mut impl Write) -> io::Result<()> {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = self.piece(&mut buffer)?;
            if read == 0 {
                return Ok(());
            }
            out.write_all(&buffer[..read])?;
        }
    }

    fn drain(&mut self) -> io::Result<()> {
        self.copy_to(&mut io::sink())
    }
}

/// A response, whole.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Content,
}

/// What a response carries.
#[derive(Debug)]
pub enum Content {
    Bytes(Vec<u8>),
    File(File, u64),
}

impl Response {
    fn new(status: u16) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: Content::Bytes(Vec::new()),
        }
    }

    fn with(mut self, name: &str, value: impl Into<String>) -> Response {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    fn xml(status: u16, xml: String) -> Response {
        Response {
            status,
            headers: vec![(
                "Content-Type".into(),
                "application/xml; charset=utf-8".into(),
            )],
            body: Content::Bytes(xml.into_bytes()),
        }
    }

    /// The body as bytes, for tests: a file's content is read.
    pub fn bytes(self) -> Vec<u8> {
        match self.body {
            Content::Bytes(bytes) => bytes,
            Content::File(mut file, _) => {
                let mut all = Vec::new();
                let _ = file.read_to_end(&mut all);
                all
            }
        }
    }

    fn write_to(self, out: &mut impl Write, head_only: bool) -> io::Result<()> {
        let length = match &self.body {
            Content::Bytes(bytes) => bytes.len() as u64,
            Content::File(_, length) => *length,
        };
        let mut head = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status));
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(&format!(
            "Content-Length: {length}\r\nDAV: 1, 2\r\nMS-Author-Via: DAV\r\n\r\n"
        ));
        out.write_all(head.as_bytes())?;
        if !head_only {
            match self.body {
                Content::Bytes(bytes) => out.write_all(&bytes)?,
                Content::File(file, length) => {
                    io::copy(&mut file.take(length), out)?;
                }
            }
        }
        out.flush()
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        207 => "Multi-Status",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        412 => "Precondition Failed",
        415 => "Unsupported Media Type",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        507 => "Insufficient Storage",
        _ => "Unknown",
    }
}

fn status_of(error: &io::Error) -> u16 {
    match error.kind() {
        io::ErrorKind::NotFound => 404,
        io::ErrorKind::PermissionDenied => 403,
        io::ErrorKind::AlreadyExists => 405,
        io::ErrorKind::StorageFull => 507,
        _ => 500,
    }
}

/// The names of `path` (a request path) under the token, decoded and checked; `None` when the
/// path does not start with the token or a name leads anywhere but down.
pub fn names_of(path: &str, token: &str) -> Option<Vec<String>> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    if segments.next()? != token {
        return None;
    }
    let mut names = Vec::new();
    for segment in segments {
        let name = percent_decode(segment)?;
        if name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
            return None;
        }
        names.push(name);
    }
    Some(names)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = text.get(at + 1..at + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// How many symbolic links one path may follow before it is refused as a loop.
const MAX_LINKS: usize = 40;

/// Where `names` lead in the storage, checked to stay inside it once every link is followed.
/// A path that does not exist yet is checked through its folder.
///
/// The walk starts at the storage and goes down one name at a time, following each link by
/// hand. It never resolves the storage's own folders above it, nor asks the C library's
/// `realpath`, which on Android reads `/proc/self/fd`: the server's sandbox may not look at
/// `/data` or `/proc`, and a check that needs them refused every request with 403.
fn resolve(config: &Config, names: &[String]) -> Result<PathBuf, u16> {
    if names
        .first()
        .is_some_and(|first| config.hidden.iter().any(|hidden| hidden == first))
    {
        return Err(404);
    }
    let root = lexical(&config.root);
    fs::symlink_metadata(&root).map_err(|e| status_of(&e))?;
    // The names still to walk, last one at the end of the list.
    let mut pending: Vec<std::ffi::OsString> = names.iter().rev().map(Into::into).collect();
    let mut at = root.clone();
    let mut links = 0;
    while let Some(name) = pending.pop() {
        if name == ".." {
            if at == root {
                return Err(403);
            }
            at.pop();
            continue;
        }
        if name == "." || name.is_empty() {
            continue;
        }
        let next = at.join(&name);
        match fs::symlink_metadata(&next) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                links += 1;
                if links > MAX_LINKS {
                    return Err(403);
                }
                let target = fs::read_link(&next).map_err(|e| status_of(&e))?;
                let rest: Vec<std::ffi::OsString> = if target.is_absolute() {
                    let target = lexical(&target);
                    let Ok(inside) = target.strip_prefix(&root) else {
                        return Err(403);
                    };
                    at = root.clone();
                    inside.iter().map(Into::into).collect()
                } else {
                    target.iter().map(Into::into).collect()
                };
                pending.extend(rest.into_iter().rev());
            }
            Ok(_) => at = next,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !pending.is_empty() {
                    // A folder on the way is missing.
                    return Err(409);
                }
                return Ok(next);
            }
            Err(error) => return Err(status_of(&error)),
        }
    }
    Ok(at)
}

/// `path` with `.` and `..` taken out by their names alone.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The href of `names`, with a trailing slash for a folder.
fn href(config: &Config, names: &[String], folder: bool) -> String {
    let mut href = format!("/{}/", percent_encode(&config.token));
    href.push_str(
        &names
            .iter()
            .map(|name| percent_encode(name))
            .collect::<Vec<_>>()
            .join("/"),
    );
    if folder && !names.is_empty() {
        href.push('/');
    }
    href
}

/// Answers one request.
pub fn handle(request: &Request, body: &mut Body<'_>, config: &Config) -> Response {
    if request.method == "OPTIONS" {
        return Response::new(200).with(
            "Allow",
            "OPTIONS, PROPFIND, PROPPATCH, GET, HEAD, PUT, DELETE, MKCOL, COPY, MOVE, LOCK, UNLOCK",
        );
    }
    // Windows' WebDAV redirector looks at the server's root before the share under it, and
    // takes a refusal there as "no WebDAV server here". The root is answered as an empty
    // folder: it names nothing, not even the token.
    if request.path.split('/').all(str::is_empty) && request.method == "PROPFIND" {
        let _ = body.text();
        return Response::xml(
            207,
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><D:multistatus xmlns:D=\"DAV:\">\
             <D:response><D:href>/</D:href><D:propstat><D:prop><D:displayname/>\
             <D:resourcetype><D:collection/></D:resourcetype></D:prop>\
             <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"
                .into(),
        );
    }
    let Some(names) = names_of(&request.path, &config.token) else {
        return Response::new(403);
    };
    let path = match resolve(config, &names) {
        Ok(path) => path,
        Err(status) => return Response::new(status),
    };
    let outcome = match request.method.as_str() {
        "PROPFIND" => propfind(request, body, config, &names, &path),
        "PROPPATCH" => proppatch(body, config, &names, &path),
        "GET" | "HEAD" => get(&path),
        "PUT" => put(body, &path),
        "DELETE" => delete(&names, &path),
        "MKCOL" => mkcol(&path),
        "COPY" | "MOVE" => copy_or_move(request, config, &names, &path),
        "LOCK" => lock(config, &names, &path),
        "UNLOCK" => Ok(Response::new(204)),
        _ => Ok(Response::new(501)),
    };
    outcome.unwrap_or_else(|error| Response::new(status_of(&error)))
}

fn entry_xml(config: &Config, names: &[String], path: &Path) -> io::Result<String> {
    let metadata = fs::metadata(path)?;
    let folder = metadata.is_dir();
    let name = names.last().map(String::as_str).unwrap_or_default();
    let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
    let created = metadata.created().unwrap_or(modified);
    let mut prop = format!(
        "<D:displayname>{}</D:displayname><D:getlastmodified>{}</D:getlastmodified>\
         <D:creationdate>{}</D:creationdate>",
        xml_escape(name),
        http_date(modified),
        iso_date(created)
    );
    if folder {
        prop.push_str("<D:resourcetype><D:collection/></D:resourcetype>");
    } else {
        prop.push_str(&format!(
            "<D:resourcetype/><D:getcontentlength>{}</D:getcontentlength>\
             <D:getcontenttype>{}</D:getcontenttype>",
            metadata.len(),
            content_type(name)
        ));
    }
    Ok(format!(
        "<D:response><D:href>{}</D:href><D:propstat><D:prop>{prop}</D:prop>\
         <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
        xml_escape(&href(config, names, folder))
    ))
}

fn propfind(
    request: &Request,
    body: &mut Body<'_>,
    config: &Config,
    names: &[String],
    path: &Path,
) -> io::Result<Response> {
    let _ = body.text()?;
    let mut xml =
        String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?><D:multistatus xmlns:D=\"DAV:\">");
    xml.push_str(&entry_xml(config, names, path)?);
    let depth = request.header("Depth").unwrap_or("1");
    if depth != "0" && path.is_dir() {
        let mut children: Vec<String> = fs::read_dir(path)?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        children.sort();
        for child in children {
            if names.is_empty() && config.hidden.contains(&child) {
                continue;
            }
            let child_path = path.join(&child);
            let mut child_names = names.to_vec();
            child_names.push(child);
            // A link that leads outside the storage is not listed.
            if resolve(config, &child_names).is_err() {
                continue;
            }
            if let Ok(entry) = entry_xml(config, &child_names, &child_path) {
                xml.push_str(&entry);
            }
        }
    }
    xml.push_str("</D:multistatus>");
    Ok(Response::xml(207, xml))
}

/// Takes the properties a client sets (the host's client sets its file times) and says each
/// is set, keeping none of them: the storage keeps its own times.
fn proppatch(
    body: &mut Body<'_>,
    config: &Config,
    names: &[String],
    path: &Path,
) -> io::Result<Response> {
    let text = body.text()?;
    fs::metadata(path)?;
    let mut props = String::new();
    for (namespace, local) in set_properties(&text) {
        props.push_str(&format!("<{local} xmlns=\"{}\"/>", xml_escape(&namespace)));
    }
    let folder = path.is_dir();
    Ok(Response::xml(
        207,
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><D:multistatus xmlns:D=\"DAV:\">\
             <D:response><D:href>{}</D:href><D:propstat><D:prop>{props}</D:prop>\
             <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>",
            xml_escape(&href(config, names, folder))
        ),
    ))
}

/// The properties named inside the `prop` elements of a PROPPATCH body, as (namespace, local
/// name).
pub fn set_properties(xml: &str) -> Vec<(String, String)> {
    let mut namespaces = Vec::new();
    for part in xml.split("xmlns:").skip(1) {
        if let Some((prefix, rest)) = part.split_once('=') {
            let quote = rest.chars().next().unwrap_or('"');
            let value = rest[1..].split(quote).next().unwrap_or_default();
            namespaces.push((prefix.trim().to_string(), value.to_string()));
        }
    }
    let namespace_of = |prefix: &str| {
        namespaces
            .iter()
            .find(|(known, _)| known == prefix)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    let mut found = Vec::new();
    let mut depth_in_prop = 0usize;
    for tag in xml.split('<').skip(1) {
        let tag = tag.split('>').next().unwrap_or_default();
        let closing = tag.starts_with('/');
        let empty = tag.ends_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or_default();
        if name.is_empty() || name.starts_with('?') || name.starts_with('!') {
            continue;
        }
        let (prefix, local) = name.split_once(':').unwrap_or(("", name));
        if local == "prop" {
            depth_in_prop = usize::from(!closing);
            continue;
        }
        if depth_in_prop == 1 && !closing {
            found.push((namespace_of(prefix), local.to_string()));
            if !empty {
                depth_in_prop = 2;
            }
        } else if depth_in_prop == 2 && closing {
            depth_in_prop = 1;
        }
    }
    found
}

fn get(path: &Path) -> io::Result<Response> {
    let metadata = fs::metadata(path)?;
    if metadata.is_dir() {
        return Ok(Response::new(405));
    }
    let file = File::open(path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    Ok(Response {
        status: 200,
        headers: vec![
            ("Content-Type".into(), content_type(name).into()),
            (
                "Last-Modified".into(),
                http_date(metadata.modified().unwrap_or(UNIX_EPOCH)),
            ),
        ],
        body: Content::File(file, metadata.len()),
    })
}

fn put(body: &mut Body<'_>, path: &Path) -> io::Result<Response> {
    if path.is_dir() {
        return Ok(Response::new(405));
    }
    let existed = path.exists();
    let mut file = File::create(path)?;
    body.copy_to(&mut file)?;
    file.sync_all()?;
    Ok(Response::new(if existed { 204 } else { 201 }))
}

fn delete(names: &[String], path: &Path) -> io::Result<Response> {
    if names.is_empty() {
        // The storage itself stays.
        return Ok(Response::new(403));
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(Response::new(204))
}

fn mkcol(path: &Path) -> io::Result<Response> {
    if path.exists() {
        return Ok(Response::new(405));
    }
    fs::create_dir(path)?;
    Ok(Response::new(201))
}

fn copy_or_move(
    request: &Request,
    config: &Config,
    names: &[String],
    path: &Path,
) -> io::Result<Response> {
    if names.is_empty() {
        return Ok(Response::new(403));
    }
    let Some(destination) = request.header("Destination") else {
        return Ok(Response::new(400));
    };
    let target_path = destination
        .strip_prefix("http://")
        .or_else(|| destination.strip_prefix("https://"))
        .and_then(|rest| rest.find('/').map(|at| &rest[at..]))
        .unwrap_or(destination);
    let Some(target_names) = names_of(target_path, &config.token) else {
        return Ok(Response::new(403));
    };
    if target_names.is_empty() {
        return Ok(Response::new(403));
    }
    let target = match resolve(config, &target_names) {
        Ok(target) => target,
        Err(status) => return Ok(Response::new(status)),
    };
    if target == path {
        return Ok(Response::new(403));
    }
    let overwrite = !request
        .header("Overwrite")
        .is_some_and(|value| value.eq_ignore_ascii_case("F"));
    let existed = target.exists();
    if existed {
        if !overwrite {
            return Ok(Response::new(412));
        }
        if target.is_dir() {
            fs::remove_dir_all(&target)?;
        } else {
            fs::remove_file(&target)?;
        }
    }
    if request.method == "MOVE" {
        fs::rename(path, &target)?;
    } else {
        copy_all(path, &target)?;
    }
    Ok(Response::new(if existed { 204 } else { 201 }))
}

fn copy_all(from: &Path, to: &Path) -> io::Result<()> {
    if fs::metadata(from)?.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            // Links are not followed out of the folder being copied.
            if entry.file_type()?.is_symlink() {
                continue;
            }
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

/// Grants a lock, which only marks the file as the client's to write; a lock on a name that
/// is not there yet makes an empty file, as the client expects.
fn lock(config: &Config, names: &[String], path: &Path) -> io::Result<Response> {
    let created = !path.exists();
    if created {
        File::create(path)?;
    }
    let token = format!(
        "opaquelocktoken:dsa-{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    );
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><D:prop xmlns:D=\"DAV:\"><D:lockdiscovery>\
         <D:activelock><D:locktype><D:write/></D:locktype><D:lockscope><D:exclusive/>\
         </D:lockscope><D:depth>0</D:depth><D:timeout>Second-3600</D:timeout>\
         <D:locktoken><D:href>{token}</D:href></D:locktoken><D:lockroot><D:href>{}</D:href>\
         </D:lockroot></D:activelock></D:lockdiscovery></D:prop>",
        xml_escape(&href(config, names, path.is_dir()))
    );
    Ok(
        Response::xml(if created { 201 } else { 200 }, xml)
            .with("Lock-Token", format!("<{token}>")),
    )
}

fn content_type(name: &str) -> &'static str {
    let extension = name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "txt" => "text/plain",
        "pdf" => "application/pdf",
        "apk" => "application/vnd.android.package-archive",
        _ => "application/octet-stream",
    }
}

/// The civil date of a day count since 1970-01-01: (year, month, day).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn parts_of(time: SystemTime) -> (i64, u32, u32, u64, u64, u64, usize) {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    let days = (seconds / 86_400) as i64;
    let (year, month, day) = civil(days);
    let in_day = seconds % 86_400;
    // 1970-01-01 was a Thursday.
    let weekday = ((days + 4).rem_euclid(7)) as usize;
    (
        year,
        month,
        day,
        in_day / 3600,
        in_day / 60 % 60,
        in_day % 60,
        weekday,
    )
}

/// `time` as HTTP writes dates: `Tue, 15 Nov 1994 08:12:31 GMT`.
pub fn http_date(time: SystemTime) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (year, month, day, hour, minute, second, weekday) = parts_of(time);
    format!(
        "{}, {day:02} {} {year} {hour:02}:{minute:02}:{second:02} GMT",
        DAYS[weekday],
        MONTHS[month as usize - 1]
    )
}

fn iso_date(time: SystemTime) -> String {
    let (year, month, day, hour, minute, second, _) = parts_of(time);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr29_android_files_paths_start_with_the_token_and_go_only_down() {
        assert_eq!(
            names_of("/tok/Download/a%20b.txt", "tok"),
            Some(vec!["Download".to_string(), "a b.txt".to_string()])
        );
        assert_eq!(names_of("/tok/", "tok"), Some(Vec::new()));
        assert_eq!(names_of("/other/Download", "tok"), None);
        assert_eq!(names_of("/tok/../etc", "tok"), None);
        assert_eq!(names_of("/tok/%2e%2e/etc", "tok"), None);
        assert_eq!(names_of("/tok/a%2Fb", "tok"), None);
        assert_eq!(names_of("/tok/a%5Cb", "tok"), None);
        assert_eq!(names_of("/tok/a%00", "tok"), None);
    }

    #[test]
    fn fr29_http_dates_are_written_as_http_writes_them() {
        let time = UNIX_EPOCH + std::time::Duration::from_secs(784_887_151);
        assert_eq!(http_date(time), "Tue, 15 Nov 1994 08:12:31 GMT");
        assert_eq!(iso_date(time), "1994-11-15T08:12:31Z");
    }

    #[test]
    fn fr29_the_properties_a_client_sets_are_named_back() {
        let xml = r#"<?xml version="1.0"?><D:propertyupdate xmlns:D="DAV:"
            xmlns:Z="urn:schemas-microsoft-com:"><D:set><D:prop>
            <Z:Win32LastModifiedTime>Tue, 15 Nov 1994</Z:Win32LastModifiedTime>
            <Z:Win32FileAttributes>00000020</Z:Win32FileAttributes>
            </D:prop></D:set></D:propertyupdate>"#;
        assert_eq!(
            set_properties(xml),
            vec![
                (
                    "urn:schemas-microsoft-com:".to_string(),
                    "Win32LastModifiedTime".to_string()
                ),
                (
                    "urn:schemas-microsoft-com:".to_string(),
                    "Win32FileAttributes".to_string()
                ),
            ]
        );
    }
}
