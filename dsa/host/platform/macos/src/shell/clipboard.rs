//! The text clipboard the Mac and the guest share, through the general `NSPasteboard`.
//!
//! macOS sends no notice when the pasteboard changes; its change count goes up instead, so
//! the host reads [`Pasteboard::change_count`] on a timer and passes text on when it moved.
//! Text the host put there itself is not sent back to the guest.

use dsa_host_core::clipboard::fits;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;

use crate::objc::{class, ns_string, string_of};

/// `NSPasteboardTypeString`.
const TEXT_TYPE: &str = "public.utf8-plain-text";

/// The general pasteboard.
pub struct Pasteboard {
    board: Retained<AnyObject>,
    /// The change count of the text the host itself last wrote, which is not passed back.
    written: Option<isize>,
    /// The change count last seen.
    seen: isize,
}

// The pasteboard is a process-wide object that may be used from any thread.
unsafe impl Send for Pasteboard {}

impl Pasteboard {
    pub fn general() -> Result<Pasteboard, String> {
        unsafe {
            let board: Option<Retained<AnyObject>> =
                msg_send![class(c"NSPasteboard")?, generalPasteboard];
            let board = board.ok_or("there is no general pasteboard")?;
            let seen: isize = msg_send![&*board, changeCount];
            Ok(Pasteboard {
                board,
                written: None,
                seen,
            })
        }
    }

    /// The pasteboard's change count, which goes up with every change.
    pub fn change_count(&self) -> isize {
        unsafe { msg_send![&*self.board, changeCount] }
    }

    /// The text on the pasteboard, if any.
    pub fn text(&self) -> Option<String> {
        unsafe {
            let text: *mut AnyObject =
                msg_send![&*self.board, stringForType: &*ns_string(TEXT_TYPE)];
            (!text.is_null()).then(|| string_of(text))
        }
    }

    /// Text another program copied since the last call, to pass to the guest. `None` when
    /// nothing changed, the change was the host's own, the pasteboard holds no text, or the
    /// text is too long for the guest's clipboard.
    pub fn changed_text(&mut self) -> Option<String> {
        let count = self.change_count();
        if count == self.seen {
            return None;
        }
        self.seen = count;
        if self.written == Some(count) {
            return None;
        }
        self.text().filter(|text| fits(text))
    }

    /// Puts `text`, which the guest copied, on the pasteboard.
    pub fn set_text(&mut self, text: &str) -> Result<(), String> {
        unsafe {
            let _: isize = msg_send![&*self.board, clearContents];
            let ok: bool = msg_send![
                &*self.board,
                setString: &*ns_string(text),
                forType: &*ns_string(TEXT_TYPE)
            ];
            if !ok {
                return Err("the pasteboard did not take the text".into());
            }
        }
        let count = self.change_count();
        self.written = Some(count);
        self.seen = count;
        Ok(())
    }
}
