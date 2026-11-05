//! An input method's text (text-input-v3) as compositions and commits for the guest's
//! editor.
//!
//! The protocol sends a batch of events and then `done`: the text to delete around the
//! cursor, the text to commit, and the new preedit (the composition) with its cursor in
//! bytes. Only `done` applies the batch, in that order. The guest's editor receives the
//! composition whole each time, with its cursor in UTF-16 code units, as the Windows input
//! method's compositions reach it, so Hangul, kana and pinyin compose in the guest exactly as
//! they do there.

use dsa_host_core::ime::{ImeEvent, TextInput};

/// One batch of text-input-v3 events, waiting for its `done`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Pending {
    preedit: Option<(String, i32)>,
    commit: Option<String>,
}

/// The input method's state for one task window.
#[derive(Debug)]
pub struct TextInputV3 {
    input: TextInput,
    pending: Pending,
}

/// The UTF-16 length of the first `bytes` bytes of `text`; the whole text when `bytes` is
/// negative (the protocol's "hide the cursor") or past its end, or not on a character
/// boundary.
pub fn utf16_cursor(text: &str, bytes: i32) -> u32 {
    let end = match usize::try_from(bytes) {
        Ok(bytes) if bytes <= text.len() && text.is_char_boundary(bytes) => bytes,
        _ => text.len(),
    };
    text[..end].encode_utf16().count() as u32
}

impl TextInputV3 {
    pub fn new(task_id: i32) -> TextInputV3 {
        TextInputV3 {
            input: TextInput::new(task_id),
            pending: Pending::default(),
        }
    }

    /// `preedit_string`: the composition is `text` (none when `None`), with the cursor
    /// starting `cursor_begin` bytes in.
    pub fn preedit(&mut self, text: Option<String>, cursor_begin: i32) {
        self.pending.preedit = text.map(|text| (text, cursor_begin));
    }

    /// `commit_string`: `text` is committed.
    pub fn commit(&mut self, text: Option<String>) {
        self.pending.commit = text;
    }

    /// `done`: applies the batch, and says what the guest's editor receives, in order.
    pub fn done(&mut self) -> Vec<ImeEvent> {
        let batch = std::mem::take(&mut self.pending);
        let mut events = Vec::new();
        if let Some(text) = batch.commit.filter(|text| !text.is_empty()) {
            events.extend(self.input.commit(&text));
        }
        match batch.preedit.filter(|(text, _)| !text.is_empty()) {
            Some((text, cursor)) => {
                let cursor = utf16_cursor(&text, cursor);
                events.extend(self.input.composition(&text, cursor));
            }
            None => events.extend(self.input.end()),
        }
        events
    }

    /// The window lost the keyboard, or the input method was turned off: a composition in
    /// progress goes, as on Windows.
    pub fn leave(&mut self) -> Vec<ImeEvent> {
        self.pending = Pending::default();
        self.input.end().into_iter().collect()
    }

    /// A key typed `text` with no input method composing it.
    pub fn typed(&mut self, text: &str) -> Vec<ImeEvent> {
        self.input.commit(text).into_iter().collect()
    }

    /// Whether the guest's editor holds a composition.
    pub fn is_composing(&self) -> bool {
        self.input.is_composing()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composition(text: &str, cursor: u32) -> ImeEvent {
        ImeEvent::Composition {
            task_id: 4,
            text: text.into(),
            cursor,
        }
    }

    fn commit(text: &str) -> ImeEvent {
        ImeEvent::Commit {
            task_id: 4,
            text: text.into(),
        }
    }

    #[test]
    fn fr6_hangul_composes_jamo_by_jamo_and_commits_on_the_next_syllable() {
        let mut ime = TextInputV3::new(4);
        ime.preedit(Some("\u{314e}".into()), 3);
        assert_eq!(ime.done(), vec![composition("\u{314e}", 1)]);
        ime.preedit(Some("\u{d55c}".into()), 3);
        assert_eq!(ime.done(), vec![composition("\u{d55c}", 1)]);
        // Typing the next syllable's first jamo commits 한 and starts a new composition.
        ime.commit(Some("\u{d55c}".into()));
        ime.preedit(Some("\u{3131}".into()), 3);
        assert_eq!(
            ime.done(),
            vec![commit("\u{d55c}"), composition("\u{3131}", 1)]
        );
        assert!(ime.is_composing());
    }

    #[test]
    fn fr6_a_commit_without_a_preedit_ends_the_composition() {
        let mut ime = TextInputV3::new(4);
        ime.preedit(Some("ni".into()), 2);
        ime.done();
        ime.commit(Some("\u{4f60}".into()));
        assert_eq!(ime.done(), vec![commit("\u{4f60}")]);
        assert!(!ime.is_composing());
        assert_eq!(ime.done(), vec![], "nothing left to end");
    }

    #[test]
    fn fr6_an_empty_preedit_clears_the_composition() {
        let mut ime = TextInputV3::new(4);
        ime.preedit(Some("ka".into()), 2);
        ime.done();
        ime.preedit(None, 0);
        assert_eq!(ime.done(), vec![composition("", 0)]);
    }

    #[test]
    fn fr6_the_cursor_is_counted_in_utf16_code_units() {
        assert_eq!(utf16_cursor("\u{d55c}a", 3), 1);
        assert_eq!(utf16_cursor("\u{1f600}x", 4), 2, "an emoji is two units");
        assert_eq!(utf16_cursor("abc", -1), 3, "a hidden cursor stands at the end");
        assert_eq!(utf16_cursor("\u{d55c}", 1), 1, "inside a character means the end");
    }

    #[test]
    fn fr6_losing_the_keyboard_drops_the_composition() {
        let mut ime = TextInputV3::new(4);
        ime.preedit(Some("ka".into()), 2);
        ime.done();
        assert_eq!(ime.leave(), vec![composition("", 0)]);
    }
}
