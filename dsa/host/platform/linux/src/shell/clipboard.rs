//! The clipboard formats that carry text, as `wl_data_offer` names them.
//!
//! The guest's clipboard holds text. The host offers what the guest copied under every
//! name a Linux program looks for text by, and takes text from an offer under the first of
//! those it has, preferring UTF-8.

use dsa_host_core::clipboard;

/// The names text goes by, best first.
pub const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
    "TEXT",
];

/// The type to read text from an offer that has `offered`; `None` when it has no text.
pub fn text_type(offered: &[String]) -> Option<&str> {
    TEXT_TYPES.iter().find_map(|wanted| {
        offered
            .iter()
            .find(|offered| offered.eq_ignore_ascii_case(wanted))
            .map(String::as_str)
    })
}

/// The text a desktop program copied, as the guest takes it: `None` when it is not UTF-8 or
/// too long for the guest's clipboard.
pub fn host_text(bytes: Vec<u8>) -> Option<String> {
    let text = String::from_utf8(bytes).ok()?;
    clipboard::fits(&text).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr13_text_is_read_in_utf8_when_the_program_offers_it() {
        let offered = vec![
            "text/html".to_string(),
            "STRING".into(),
            "UTF8_STRING".into(),
        ];
        assert_eq!(text_type(&offered), Some("UTF8_STRING"));
        let offered = vec!["TEXT/PLAIN;CHARSET=UTF-8".to_string()];
        assert_eq!(text_type(&offered), Some("TEXT/PLAIN;CHARSET=UTF-8"));
        assert_eq!(text_type(&["image/png".to_string()]), None);
    }

    #[test]
    fn fr13_text_too_long_for_the_guest_or_not_utf8_is_left_out() {
        assert_eq!(host_text(b"hello".to_vec()), Some("hello".into()));
        assert_eq!(host_text(vec![0xff, 0xfe]), None);
        assert_eq!(
            host_text(vec![b'a'; clipboard::MAX_CLIPBOARD_BYTES + 1]),
            None
        );
    }
}
