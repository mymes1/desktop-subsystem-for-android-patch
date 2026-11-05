//! Showing the shared storage of the user's Android in Explorer.
//!
//! The service bridges a loopback port to the guest, which serves its shared storage as
//! WebDAV, and answers with the address (`http://127.0.0.1:<port>/<token>/`). Explorer opens
//! WebDAV through Windows' WebDAV redirector, which takes the address as a UNC path,
//! `\\127.0.0.1@<port>\<token>`. The token is the first part of the path and the service
//! makes a new one for each session, so only someone the service told can open the files.
//!
//! The "DSA Files" Start menu shortcut runs the installed window host with
//! [`ANDROID_FILES_ARGUMENT`], and the settings app's button runs `dsa files open`; both ask
//! the service for the address, starting Android when it is not running, and open it.

/// The argument that has the window host open Android's files in Explorer.
pub const ANDROID_FILES_ARGUMENT: &str = "--android-files";

/// The UNC path the WebDAV redirector takes for `url`, an `http://host:port/path/` address on
/// the loopback address. `None` for any other address.
pub fn unc_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = authority.rsplit_once(':')?;
    if host != "127.0.0.1" || port.parse::<u16>().is_err() {
        return None;
    }
    let path = path.trim_end_matches('/');
    let mut unc = format!(r"\\{host}@{port}");
    if path.is_empty() {
        unc.push_str(r"\DavWWWRoot");
    } else {
        unc.push('\\');
        unc.push_str(&path.replace('/', "\\"));
    }
    Some(unc)
}

/// Opens `path` in a new Explorer window.
#[cfg(windows)]
pub fn open_in_explorer(path: &str) -> Result<(), String> {
    let windows = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
    std::process::Command::new(windows.join("explorer.exe"))
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cannot open {path} in Explorer: {e}"))
}

/// Asks the service at `api` where the caller's Android files are, as a path Explorer opens.
#[cfg(windows)]
pub fn locate(api: &mut dyn dsa_transport::Connector) -> Result<String, String> {
    let url = dsa_host_core::host_api::HostApiClient::connect(
        api,
        "dsa-app-window files",
        std::time::Duration::from_secs(5),
    )
    .map_err(|e| format!("the DSA service cannot be reached: {e}"))?
    .android_files()
    .map_err(|e| e.to_string())?;
    unc_path(&url).ok_or_else(|| format!("Explorer cannot open {url}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr29_the_webdav_address_becomes_the_redirectors_unc_path() {
        assert_eq!(
            unc_path("http://127.0.0.1:52714/3f9a/").as_deref(),
            Some(r"\\127.0.0.1@52714\3f9a")
        );
        assert_eq!(
            unc_path("http://127.0.0.1:52714/").as_deref(),
            Some(r"\\127.0.0.1@52714\DavWWWRoot")
        );
        assert_eq!(unc_path("http://example.com:80/x/"), None);
        assert_eq!(unc_path("file:///C:/"), None);
    }
}
