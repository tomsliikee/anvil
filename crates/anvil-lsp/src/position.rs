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
    fn utf16_columns() {
        let t = "ab\nü😀x\nend";
        let p = to_position(t, t.find('x').unwrap());
        assert_eq!(p, Position { line: 1, character: 3 });
        assert_eq!(to_offset(t, p), t.find('x').unwrap());
        assert_eq!(to_offset(t, Position { line: 0, character: 99 }), 2);
        assert_eq!(to_offset(t, Position { line: 9, character: 0 }), t.len());
    }
}
