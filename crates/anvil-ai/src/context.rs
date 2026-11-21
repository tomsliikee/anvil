//! What the model sees: for ghost text the file around the cursor plus the
//! signatures of the project's types; for chat the selection, the file and
//! the error the user asks about.

use anvil_core::csharp::{FileStructure, MembersFormat};

/// Prefix and suffix around `cursor`, cut to `max_chars` (more before the
/// cursor than after, at line starts), with the project's type signatures
/// as a comment header so the model knows the user's own types.
pub fn fim(text: &str, cursor: usize, signatures: &str, max_chars: usize) -> (String, String) {
    let before_budget = max_chars * 3 / 4;
    let after_budget = max_chars - before_budget;
    let mut start = cursor.saturating_sub(before_budget);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    if start > 0 {
        start = text[start..cursor].find('\n').map_or(start, |i| start + i + 1);
    }
    let mut end = (cursor + after_budget).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end < text.len() {
        end = text[cursor..end].rfind('\n').map_or(end, |i| cursor + i);
    }
    let header = if signatures.is_empty() { String::new() } else { format!("// Types in this project:\n{signatures}\n") };
    (format!("{header}{}", &text[start..cursor]), text[cursor..end].to_owned())
}

/// Whether ghost text may appear while typing, given the text after the
/// caret on its line: at the end of a line, or before closing brackets,
/// quotes and `;` only (`Foo(|)`), never in the middle of code.
pub fn may_suggest_before(rest_of_line: &str) -> bool {
    rest_of_line.chars().all(|c| c.is_whitespace() || matches!(c, ')' | ']' | '}' | ';' | ',' | '"' | '\''))
}

/// One comment line per type with its members' signatures.
pub fn signatures(files: &[&FileStructure], max_chars: usize) -> String {
    let mut out = String::new();
    for f in files {
        for (outer, t) in f.all_types() {
            let members: Vec<String> = t.members.iter().map(|m| m.display(MembersFormat::FullSignature)).collect();
            let line = format!("// {} {}: {}\n", t.kind.keyword(), t.full_name(&outer), members.join("; "));
            if out.len() + line.len() > max_chars {
                return out;
            }
            out.push_str(&line);
        }
    }
    out
}

pub const SYSTEM_PROMPT: &str = "You are a C# assistant inside Anvil, an IDE for .NET console applications built for university assignments checked with StyleCop.Analyzers. Answer briefly. When you change code, reply with a unified diff in a ```diff block per file, with ---/+++ headers holding the file path relative to the solution folder and @@ hunk headers with three lines of context. Never invent files you were not shown unless asked to create one.";

/// The user message for a chat turn, with the context the user attached.
pub fn chat_message(question: &str, file: Option<(&str, &str)>, selection: Option<&str>, error: Option<&str>) -> String {
    let mut s = String::new();
    if let Some((path, text)) = file {
        s.push_str(&format!("File `{path}`:\n```csharp\n{text}\n```\n\n"));
    }
    if let Some(sel) = selection.filter(|s| !s.is_empty()) {
        s.push_str(&format!("Selected code:\n```csharp\n{sel}\n```\n\n"));
    }
    if let Some(e) = error {
        s.push_str(&format!("Error from the Error List: {e}\n\n"));
    }
    s.push_str(question);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fim_cuts_at_lines() {
        let text = "line1\nline2\nline3 cursor|rest\nline4\nline5\n";
        let cursor = text.find('|').unwrap();
        let (p, s) = fim(text, cursor, "", 24);
        assert!(p.ends_with("line3 cursor"));
        assert!(!p.starts_with("ine"));
        assert!(s.starts_with("|rest"));
        let (p, _) = fim(text, cursor, "// class A: B()", 1000);
        assert!(p.starts_with("// Types in this project:\n// class A: B()\nline1"));
    }

    #[test]
    fn suggests_only_at_line_end_or_before_closers() {
        assert!(may_suggest_before(""));
        assert!(may_suggest_before("   "));
        assert!(may_suggest_before(");"));
        assert!(may_suggest_before("\")"));
        assert!(!may_suggest_before("x + 1;"));
        assert!(!may_suggest_before(" // note"));
    }

    #[test]
    fn signatures_list_types() {
        let f = anvil_core::csharp::parse("namespace N { class A { public int X { get; set; } void M(int a) {} } }");
        assert_eq!(signatures(&[&f], 1000), "// class N.A: X : int; M(int a) : void\n");
    }
}
