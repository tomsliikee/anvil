//! A text edit: replace a byte range of a document with new text. Edits are
//! what every writer in Anvil produces, so a file only ever changes where
//! something was meant to change.

/// Replace `start..end` (byte offsets) with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl TextEdit {
    pub fn new(start: usize, end: usize, text: impl Into<String>) -> Self {
        TextEdit { start, end, text: text.into() }
    }

    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        TextEdit::new(at, at, text)
    }
}

/// Applies non-overlapping edits. The order of `edits` does not matter;
/// they are applied back to front so earlier offsets stay valid.
///
/// # Panics
/// When two edits overlap or a range is outside `text`: both are bugs in the
/// code that built the edits, never something a user can cause.
pub fn apply(text: &str, edits: &[TextEdit]) -> String {
    let mut sorted: Vec<&TextEdit> = edits.iter().collect();
    sorted.sort_by_key(|e| (e.start, e.end));
    for pair in sorted.windows(2) {
        assert!(pair[0].end <= pair[1].start, "overlapping edits {:?} and {:?}", pair[0], pair[1]);
    }
    let mut out = text.to_owned();
    for e in sorted.iter().rev() {
        assert!(e.start <= e.end && e.end <= out.len(), "edit {e:?} outside text of {} bytes", out.len());
        out.replace_range(e.start..e.end, &e.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_in_any_order() {
        let edits = [TextEdit::new(6, 11, "Anvil"), TextEdit::insert(0, ">> ")];
        assert_eq!(apply("hello world", &edits), ">> hello Anvil");
    }

    #[test]
    #[should_panic(expected = "overlapping")]
    fn overlap_is_a_bug() {
        apply("abcdef", &[TextEdit::new(0, 3, "x"), TextEdit::new(2, 4, "y")]);
    }
}
