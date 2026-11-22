//! LSP positions count UTF-16 code units per line; GTK and Rust strings
//! count bytes or chars. These convert between them on the document text.

use lsp_types::{Position, Range, TextDocumentContentChangeEvent};

/// Byte offset of an LSP position in `text` (clamped to the line's end).
pub fn to_offset(text: &str, pos: Position) -> usize {
    let mut line_start = 0;
    for _ in 0..pos.line {
        match text[line_start..].find('\n') {
            Some(i) => line_start += i + 1,
            None => return text.len(),
        }
    }
    let line_end = text[line_start..].find('\n').map_or(text.len(), |i| line_start + i);
    let mut units = 0u32;
    for (i, c) in text[line_start..line_end].char_indices() {
        if units >= pos.character {
            return line_start + i;
        }
        units += c.len_utf16() as u32;
    }
    line_end
}

/// Line starts of one text, for many [`to_offset`] lookups without
/// walking the text from its start for each (diagnostics come in hundreds).
pub struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    pub fn new(text: &str) -> Lines {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Lines { starts }
    }

    /// The same as [`to_offset`] on the text the lines were taken from.
    pub fn offset(&self, text: &str, pos: Position) -> usize {
        let Some(&line_start) = self.starts.get(pos.line as usize) else { return text.len() };
        let line_end = self.starts.get(pos.line as usize + 1).map_or(text.len(), |s| s - 1);
        let mut units = 0u32;
        for (i, c) in text[line_start..line_end].char_indices() {
            if units >= pos.character {
                return line_start + i;
            }
            units += c.len_utf16() as u32;
        }
        line_end
    }
}

pub fn to_position(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let character = text[line_start..offset].chars().map(|c| c.len_utf16() as u32).sum();
    Position { line, character }
}

/// The didChange event that turns `old` into `new`: the differing middle
/// with its range in `old`. Roslyn reads every change as ranged (a change
/// without a range throws in its DidChangeHandler and stops the server), and
/// a small range keeps the message small while typing.
pub fn change_event(old: &str, new: &str) -> TextDocumentContentChangeEvent {
    let mut prefix = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = old.len().min(new.len()) - prefix;
    let mut suffix = old.bytes().rev().zip(new.bytes().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    TextDocumentContentChangeEvent {
        range: Some(Range { start: to_position(old, prefix), end: to_position(old, old.len() - suffix) }),
        range_length: None,
        text: new[prefix..new.len() - suffix].to_owned(),
    }
}

/// Where `pos` is after `change` (one ranged edit): before the edit it
/// stays, after it it moves with the text, inside it it goes to the edit's
/// start. Diagnostics computed for slightly older text land on the same
/// code instead of being dropped.
pub fn shift(pos: Position, change: &TextDocumentContentChangeEvent) -> Position {
    let Some(r) = change.range else { return pos };
    // At the very place of an insertion the text goes in before `pos`.
    if pos < r.start || (pos == r.start && r.start != r.end) {
        return pos;
    }
    if pos < r.end {
        return r.start;
    }
    let units = |t: &str| t.chars().map(|c| c.len_utf16() as u32).sum::<u32>();
    let new_end = match change.text.rfind('\n') {
        None => Position { line: r.start.line, character: r.start.character + units(&change.text) },
        Some(i) => Position { line: r.start.line + change.text.matches('\n').count() as u32, character: units(&change.text[i + 1..]) },
    };
    if pos.line == r.end.line {
        Position { line: new_end.line, character: new_end.character + (pos.character - r.end.character) }
    } else {
        Position { line: pos.line - r.end.line + new_end.line, character: pos.character }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(old: &str, e: &TextDocumentContentChangeEvent) -> String {
        let r = e.range.expect("always ranged");
        let (a, b) = (to_offset(old, r.start), to_offset(old, r.end));
        format!("{}{}{}", &old[..a], e.text, &old[b..])
    }

    #[test]
    fn change_event_is_ranged_and_minimal() {
        let old = "class A\r\n{\r\n}\r\n";
        let new = "class A\r\n{\r\n    int x;\r\n}\r\n";
        let e = change_event(old, new);
        assert_eq!(e.range, Some(Range { start: Position { line: 2, character: 0 }, end: Position { line: 2, character: 0 } }));
        assert_eq!(e.text, "    int x;\r\n");
        assert_eq!(apply(old, &e), new);
    }

    #[test]
    fn change_event_handles_deletion_and_wide_chars() {
        for (old, new) in [("ab😀cd", "abcd"), ("x", ""), ("", "neu"), ("same", "same"), ("äöü", "äxü"), ("aaa", "aa")] {
            let e = change_event(old, new);
            assert!(e.range.is_some());
            assert_eq!(apply(old, &e), new, "{old} -> {new}");
        }
    }

    #[test]
    fn lines_agree_with_to_offset() {
        let t = "ab\nü😀x\n\nend";
        let lines = Lines::new(t);
        for line in 0..6 {
            for character in 0..6 {
                let p = Position { line, character };
                assert_eq!(lines.offset(t, p), to_offset(t, p), "{p:?}");
            }
        }
    }

    #[test]
    fn shift_follows_the_text() {
        let old = "a\nint x = 1;\nfoo();\n";
        let new = "a\nint x = 1;\nvar y = 2;\nfoo();\n";
        let e = change_event(old, new);
        let foo = to_position(old, old.find("foo").unwrap());
        assert_eq!(shift(foo, &e), to_position(new, new.find("foo").unwrap()));
        let x = to_position(old, old.find('x').unwrap());
        assert_eq!(shift(x, &e), x);
        // Same line after a typed character.
        let (o, n) = ("ab cd", "abX cd");
        let e = change_event(o, n);
        assert_eq!(shift(to_position(o, 3), &e), to_position(n, 4));
        // Inside a deleted range: the edit's start.
        let e = change_event("abcdef", "af");
        assert_eq!(shift(Position { line: 0, character: 3 }, &e), Position { line: 0, character: 1 });
    }

    #[test]
    fn utf16_columns() {
        let t = "ab\nü😀x\nend";
        let p = to_position(t, t.find('x').unwrap());
        assert_eq!(p, Position { line: 1, character: 3 });
        assert_eq!(to_offset(t, p), t.find('x').unwrap());
        assert_eq!(to_offset(t, Position { line: 0, character: 99 }), 2);
        assert_eq!(to_offset(t, Position { line: 9, character: 0 }), t.len());
    }
}
