//! The input method on macOS: what `NSTextInputClient` is told, as the platform-neutral
//! text input's compositions and commits, which reach the agent's `InputConnection`.
//!
//! A key goes first to the input method (`interpretKeyEvents:`). If it composes or types
//! text, the text goes to the guest and the key does not; if it is a command (Return, an
//! arrow, delete) or nothing at all, the key goes to the guest as a key event, so the guest
//! never sees a character twice.

use dsa_host_core::ime::{utf16_len, ImeEvent, TextInput};

/// The state `NSTextInputClient` reports back to the input method.
#[derive(Debug)]
pub struct TextInputClient {
    text: TextInput,
    /// The marked (composing) text; empty when there is none.
    marked: String,
    /// Its length in UTF-16 code units.
    marked_units: usize,
    /// The selection within the marked text, as location and length in UTF-16 code units.
    selected: (usize, usize),
    /// During a key: whether the input method typed or composed text for it.
    took_key: bool,
    /// During a key: whether there was a composition when it began.
    composing_before: bool,
}

impl TextInputClient {
    pub fn new(task_id: i32) -> TextInputClient {
        TextInputClient {
            text: TextInput::new(task_id),
            marked: String::new(),
            marked_units: 0,
            selected: (0, 0),
            took_key: false,
            composing_before: false,
        }
    }

    /// `hasMarkedText`.
    pub fn has_marked_text(&self) -> bool {
        self.marked_units > 0
    }

    /// `markedRange`: `None` is `{NSNotFound, 0}`.
    pub fn marked_range(&self) -> Option<(usize, usize)> {
        self.has_marked_text().then_some((0, self.marked_units))
    }

    /// `selectedRange`: the cursor within the composition, or an empty range at its start
    /// when there is none (the guest's own selection is not mirrored on the host).
    pub fn selected_range(&self) -> (usize, usize) {
        self.selected
    }

    /// A key is about to go to the input method.
    pub fn begin_key(&mut self) {
        self.took_key = false;
        self.composing_before = self.has_marked_text();
    }

    /// The input method is done with the key: whether the key itself must go to the guest.
    pub fn end_key(&mut self) -> bool {
        let send = !self.took_key && !self.composing_before && !self.has_marked_text();
        self.took_key = false;
        self.composing_before = false;
        send
    }

    /// `setMarkedText:selectedRange:replacementRange:` with the marked text `text` and its
    /// selection at `selected` (location, length) in UTF-16 code units.
    pub fn set_marked_text(&mut self, text: &str, selected: (usize, usize)) -> Option<ImeEvent> {
        self.took_key = true;
        let units = utf16_len(text);
        let location = selected.0.min(units);
        let end = location.saturating_add(selected.1).min(units);
        self.marked = text.to_string();
        self.marked_units = units;
        self.selected = (location, end - location);
        // The guest's composition has one cursor: at the end of the input method's selection,
        // which is where the next character goes.
        self.text.composition(text, end as u32)
    }

    /// `unmarkText`: the input method ends its composition, keeping the text it composed.
    pub fn unmark_text(&mut self) -> Option<ImeEvent> {
        let marked = std::mem::take(&mut self.marked);
        self.marked_units = 0;
        self.selected = (0, 0);
        if self.text.is_composing() {
            // The composed text stays in the editor as typed text.
            return self.text.commit(&marked);
        }
        None
    }

    /// `insertText:replacementRange:`: `text` is typed, in place of the composition if there
    /// is one.
    pub fn insert_text(&mut self, text: &str) -> Option<ImeEvent> {
        self.took_key = true;
        self.marked.clear();
        self.marked_units = 0;
        self.selected = (0, 0);
        self.text.commit(text)
    }

    /// The window lost the keyboard: a composition left open is ended and removed, as when
    /// the user switched away mid-word.
    pub fn focus_lost(&mut self) -> Option<ImeEvent> {
        self.marked.clear();
        self.marked_units = 0;
        self.selected = (0, 0);
        self.text.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composition(text: &str, cursor: u32) -> Option<ImeEvent> {
        Some(ImeEvent::Composition {
            task_id: 7,
            text: text.into(),
            cursor,
        })
    }

    fn commit(text: &str) -> Option<ImeEvent> {
        Some(ImeEvent::Commit {
            task_id: 7,
            text: text.into(),
        })
    }

    #[test]
    fn fr6_a_plain_key_goes_to_the_guest_as_a_key() {
        let mut client = TextInputClient::new(7);
        client.begin_key();
        // Return: the input method calls doCommandBySelector:, not insertText:.
        assert!(client.end_key());
    }

    #[test]
    fn fr6_a_typed_character_goes_as_text_and_not_as_its_key() {
        let mut client = TextInputClient::new(7);
        client.begin_key();
        assert_eq!(client.insert_text("a"), commit("a"));
        assert!(!client.end_key());
    }

    #[test]
    fn fr6_a_hangul_composition_is_sent_whole_and_committed() {
        let mut client = TextInputClient::new(7);
        client.begin_key();
        assert_eq!(client.set_marked_text("ㅎ", (1, 0)), composition("ㅎ", 1));
        assert!(!client.end_key());
        assert!(client.has_marked_text());
        assert_eq!(client.marked_range(), Some((0, 1)));

        client.begin_key();
        assert_eq!(client.set_marked_text("하", (1, 0)), composition("하", 1));
        assert!(!client.end_key());

        // Space commits the syllable; the space itself is typed after it.
        client.begin_key();
        assert_eq!(client.insert_text("하"), commit("하"));
        assert_eq!(client.insert_text(" "), commit(" "));
        assert!(!client.end_key());
        assert!(!client.has_marked_text());
    }

    #[test]
    fn fr6_a_key_that_ends_a_composition_is_the_input_methods_not_the_guests() {
        let mut client = TextInputClient::new(7);
        client.begin_key();
        client.set_marked_text("か", (1, 0));
        client.end_key();
        // Backspace through the last character: the composition goes, and the key with it.
        client.begin_key();
        assert_eq!(client.set_marked_text("", (0, 0)), composition("", 0));
        assert!(!client.end_key());
    }

    #[test]
    fn fr6_the_cursor_is_at_the_end_of_the_input_methods_selection() {
        let mut client = TextInputClient::new(7);
        assert_eq!(
            client.set_marked_text("にほんご", (0, 2)),
            composition("にほんご", 2)
        );
        assert_eq!(client.selected_range(), (0, 2));
        // A selection past the text is kept inside it.
        assert_eq!(client.set_marked_text("に", (5, 3)), composition("に", 1));
    }

    #[test]
    fn fr6_unmarking_keeps_the_composed_text() {
        let mut client = TextInputClient::new(7);
        client.set_marked_text("漢", (1, 0));
        assert_eq!(client.unmark_text(), commit("漢"));
        assert!(!client.has_marked_text());
        assert_eq!(client.unmark_text(), None);
    }

    #[test]
    fn fr6_losing_the_keyboard_removes_an_open_composition() {
        let mut client = TextInputClient::new(7);
        client.set_marked_text("ㄱ", (1, 0));
        assert_eq!(client.focus_lost(), composition("", 0));
        assert_eq!(client.focus_lost(), None);
    }
}
