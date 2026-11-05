//! Opening what the guest hands the host: web links in the user's browser, and files in the
//! app macOS has for them, through `NSWorkspace`.

use std::path::Path;

use dsa_host_core::links::web_url;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;

use crate::objc::{class, file_url, ns_string};

fn workspace() -> Result<Retained<AnyObject>, String> {
    unsafe {
        let workspace: Option<Retained<AnyObject>> =
            msg_send![class(c"NSWorkspace")?, sharedWorkspace];
        workspace.ok_or_else(|| "there is no shared workspace".to_string())
    }
}

fn open(url: &AnyObject) -> Result<(), String> {
    let opened: bool = unsafe { msg_send![&*workspace()?, openURL: url] };
    if opened {
        Ok(())
    } else {
        Err("macOS found no app to open it".into())
    }
}

/// Opens a web link from the guest in the user's default browser. Only `http` and `https`
/// links are opened; anything else is refused with the reason.
pub fn open_link(url: &str) -> Result<(), String> {
    let url = web_url(url)?;
    unsafe {
        let ns_url: Option<Retained<AnyObject>> =
            msg_send![class(c"NSURL")?, URLWithString: &*ns_string(url)];
        let ns_url = ns_url.ok_or("the link is not a valid URL")?;
        open(&ns_url)
    }
}

/// Opens a host file in the app macOS has for its type.
pub fn open_file(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    let url = file_url(path)?;
    open(&url)
}

/// Shows a host file selected in a Finder window.
pub fn reveal_file(path: &Path) -> Result<(), String> {
    unsafe {
        let url = file_url(path)?;
        let array_class = class(c"NSArray")?;
        let urls: *mut AnyObject = msg_send![array_class, arrayWithObject: &*url];
        let _: () = msg_send![&*workspace()?, activateFileViewerSelectingURLs: urls];
    }
    Ok(())
}
