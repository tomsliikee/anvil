//! Semantic-looking highlighting from the syntax tree alone, before (and
//! without) Roslyn: type names, method names and property names in C#.
//! Lexical colours (keywords, strings, comments) stay with the
//! GtkSourceView language definition; these spans go on top of it.
//!
//! Spans are in character offsets, as `GtkTextBuffer` counts.

use tree_sitter::{Node, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Declared or referenced type: `class Person`, `Person p`, `new Person()`.
    Type,
    /// Declared or called method, local function or constructor call target.
    Method,
    /// Declared property, or a member read through `.` that is not called.
    Property,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// All spans of `text`, sorted by start, not overlapping.
pub fn spans(text: &str) -> Vec<Span> {
    let mut parser = Parser::new();
    parser.set_language(&crate::csharp::language()).expect("the bundled C# grammar matches tree-sitter");
    let Some(tree) = parser.parse(text, None) else { return Vec::new() };
    let mut bytes = Vec::new();
    walk(tree.root_node(), &mut bytes);
    bytes.sort_by_key(|(r, _)| r.start);
    bytes.dedup_by(|b, a| b.0.start < a.0.end);
    to_chars(text, &bytes)
}

type ByteSpan = (std::ops::Range<usize>, Kind);

fn walk(node: Node, out: &mut Vec<ByteSpan>) {
    classify(node, out);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, out);
    }
}

fn mark(node: Option<Node>, kind: Kind, out: &mut Vec<ByteSpan>) {
    if let Some(n) = node
        && n.kind() == "identifier"
    {
        out.push((n.byte_range(), kind));
    }
}

/// A node in a type position: the identifier that names the type, without
/// namespaces (`System.Text.StringBuilder` marks `StringBuilder`).
fn mark_type(node: Option<Node>, out: &mut Vec<ByteSpan>) {
    let Some(n) = node else { return };
    match n.kind() {
        "identifier" => out.push((n.byte_range(), Kind::Type)),
        "generic_name" => mark(n.named_child(0), Kind::Type, out),
        "qualified_name" => mark_type(n.child_by_field_name("name"), out),
        // array_type and nullable_type mark their element through their own
        // `type` field when the walk reaches them.
        _ => {}
    }
}

/// The name a call goes to: `Foo()`, `x.Foo()`, `Foo<T>()`.
fn mark_callee(node: Option<Node>, out: &mut Vec<ByteSpan>) {
    let Some(n) = node else { return };
    match n.kind() {
        "identifier" => out.push((n.byte_range(), Kind::Method)),
        "generic_name" => mark(n.named_child(0), Kind::Method, out),
        "member_access_expression" => mark_callee(n.child_by_field_name("name"), out),
        _ => {}
    }
}

fn classify(n: Node, out: &mut Vec<ByteSpan>) {
    match n.kind() {
        "class_declaration" | "struct_declaration" | "interface_declaration" | "enum_declaration" | "record_declaration" | "delegate_declaration" => {
            mark(n.child_by_field_name("name"), Kind::Type, out);
        }
        // VS colours a constructor's name as its class.
        "constructor_declaration" | "destructor_declaration" => mark(n.child_by_field_name("name"), Kind::Type, out),
        "method_declaration" | "local_function_statement" => mark(n.child_by_field_name("name"), Kind::Method, out),
        "property_declaration" => mark(n.child_by_field_name("name"), Kind::Property, out),
        "invocation_expression" => mark_callee(n.child_by_field_name("function"), out),
        "member_access_expression" => {
            let called = n.parent().is_some_and(|p| p.kind() == "invocation_expression" && p.child_by_field_name("function") == Some(n));
            if !called {
                mark(n.child_by_field_name("name"), Kind::Property, out);
            }
        }
        "attribute" => mark_type(n.child_by_field_name("name"), out),
        "base_list" | "type_argument_list" => {
            let mut c = n.walk();
            for child in n.named_children(&mut c) {
                mark_type(Some(child), out);
            }
        }
        "using_directive" => {}
        _ => {
            mark_type(n.child_by_field_name("type"), out);
            mark_type(n.child_by_field_name("returns"), out);
        }
    }
}

fn to_chars(text: &str, spans: &[ByteSpan]) -> Vec<Span> {
    let mut out = Vec::with_capacity(spans.len());
    let (mut byte, mut chars) = (0usize, 0usize);
    let mut advance = |to: usize| {
        chars += text[byte..to].chars().count();
        byte = to;
        chars
    };
    for (r, kind) in spans {
        let start = advance(r.start);
        let end = advance(r.end);
        out.push(Span { start, end, kind: *kind });
    }
    out
}

/// The part of the buffer whose spans changed after an edit that grew the
/// text by `delta` characters: spans before it are equal, spans after it
/// are equal once shifted by `delta`. `None` when nothing changed. The
/// range is in new-text offsets and covers every old and new span in it.
pub fn changed_range(old: &[Span], new: &[Span], delta: isize) -> Option<(usize, usize)> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let shifted = |s: &Span| Span { start: (s.start as isize + delta) as usize, end: (s.end as isize + delta) as usize, kind: s.kind };
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old.iter().rev().zip(new.iter().rev()).take(max_suffix).take_while(|(a, b)| shifted(a) == **b).count();
    let old_mid = &old[prefix..old.len() - suffix];
    let new_mid = &new[prefix..new.len() - suffix];
    if old_mid.is_empty() && new_mid.is_empty() {
        return None;
    }
    let starts = old_mid.iter().map(|s| shifted(s).start).chain(new_mid.iter().map(|s| s.start));
    let ends = old_mid.iter().map(|s| shifted(s).end).chain(new_mid.iter().map(|s| s.end));
    Some((starts.min()?, ends.max()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(text: &str, kind: Kind) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        spans(text).into_iter().filter(|s| s.kind == kind).map(|s| chars[s.start..s.end].iter().collect()).collect()
    }

    const SAMPLE: &str = "using System.Text;\n\nnamespace Demo\n{\n    [Serializable]\n    internal class Person : IComparable<Person>\n    {\n        private readonly List<string> names = new List<string>();\n\n        public Person(string name) { }\n\n        public string Name { get; set; }\n\n        public int CompareTo(Person? other)\n        {\n            Console.WriteLine(other.Name);\n            var b = new System.Text.StringBuilder();\n            return Helper<int>(names.Count);\n        }\n    }\n}\n";

    #[test]
    fn types_are_declarations_and_type_positions() {
        assert_eq!(named(SAMPLE, Kind::Type), ["Serializable", "Person", "IComparable", "Person", "List", "List", "Person", "Person", "StringBuilder"]);
    }

    #[test]
    fn methods_are_declarations_and_call_targets() {
        assert_eq!(named(SAMPLE, Kind::Method), ["CompareTo", "WriteLine", "Helper"]);
    }

    #[test]
    fn properties_are_declarations_and_member_reads() {
        assert_eq!(named(SAMPLE, Kind::Property), ["Name", "Name", "Count"]);
    }

    #[test]
    fn namespaces_and_usings_stay_plain() {
        let all: Vec<String> = [Kind::Type, Kind::Method, Kind::Property].into_iter().flat_map(|k| named(SAMPLE, k)).collect();
        assert!(!all.iter().any(|n| n == "Demo" || n == "System" || n == "Text"));
    }

    #[test]
    fn offsets_count_characters_not_bytes() {
        let text = "// Größe\nclass Ä { void Ö() { } }\n";
        let s = spans(text);
        let chars: Vec<char> = text.chars().collect();
        let got: Vec<String> = s.iter().map(|s| chars[s.start..s.end].iter().collect()).collect();
        assert_eq!(got, ["Ä", "Ö"]);
    }

    #[test]
    fn broken_code_still_highlights_what_parses() {
        let text = "class A {\n    void M() {\n        Foo(\n    }\n}\n";
        assert!(named(text, Kind::Method).contains(&"M".to_owned()));
    }

    fn sp(start: usize, end: usize) -> Span {
        Span { start, end, kind: Kind::Type }
    }

    #[test]
    fn changed_range_is_none_for_a_pure_shift() {
        // Typing one character before both spans shifts them.
        assert_eq!(changed_range(&[sp(5, 8), sp(10, 12)], &[sp(6, 9), sp(11, 13)], 1), None);
    }

    #[test]
    fn changed_range_covers_only_the_edited_span() {
        let old = [sp(0, 3), sp(10, 14), sp(20, 22)];
        let new = [sp(0, 3), sp(10, 15), sp(21, 23)];
        assert_eq!(changed_range(&old, &new, 1), Some((10, 15)));
    }

    #[test]
    fn changed_range_covers_removed_and_added_spans() {
        assert_eq!(changed_range(&[sp(0, 3), sp(6, 9)], &[sp(0, 3)], 0), Some((6, 9)));
        assert_eq!(changed_range(&[sp(0, 3)], &[sp(0, 3), sp(4, 7)], 4), Some((4, 7)));
    }
}
