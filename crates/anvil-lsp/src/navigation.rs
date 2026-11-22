//! Shapes of Roslyn's navigation answers, made plain: locations from
//! definition, implementation and references (any of the three forms LSP
//! allows), references grouped by file, workspace symbols for Go To All,
//! workspace edits flattened per file, and LSP edits applied to text.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lsp_types as lt;
use serde_json::Value;

use crate::position::to_offset;
use crate::roslyn;

/// `Location`, `Location[]` or `LocationLink[]` (or null) as locations.
pub fn locations(v: Value) -> Vec<lt::Location> {
    if v.is_null() {
        return Vec::new();
    }
    if let Ok(one) = serde_json::from_value::<lt::Location>(v.clone()) {
        return vec![one];
    }
    if let Ok(many) = serde_json::from_value::<Vec<lt::Location>>(v.clone()) {
        return many;
    }
    serde_json::from_value::<Vec<lt::LocationLink>>(v)
        .map(|links| links.into_iter().map(|l| lt::Location { uri: l.target_uri, range: l.target_selection_range }).collect())
        .unwrap_or_default()
}

/// Locations per file (files sorted by path, places by position), as the
/// Find All References panel shows them. Duplicates are dropped: Roslyn
/// reports a partial or linked definition more than once.
pub fn group_by_file(locs: &[lt::Location]) -> Vec<(PathBuf, Vec<lt::Range>)> {
    let mut map: BTreeMap<PathBuf, Vec<lt::Range>> = BTreeMap::new();
    for l in locs {
        if let Some(p) = roslyn::path_from_uri(l.uri.as_str()) {
            map.entry(p).or_default().push(l.range);
        }
    }
    map.into_iter()
        .map(|(p, mut r)| {
            r.sort_by_key(|r| (r.start.line, r.start.character));
            r.dedup();
            (p, r)
        })
        .collect()
}

/// A file Roslyn wrote for a definition in a referenced assembly
/// (metadata as source, decompiled): shown read-only.
pub fn is_metadata(path: &Path, home: &Path) -> bool {
    path.starts_with(roslyn::temp_dir(home)) || path.components().any(|c| c.as_os_str() == "MetadataAsSource")
}

/// One Go To All entry from `workspace/symbol`.
#[derive(Clone, Debug, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub container: String,
    pub kind: lt::SymbolKind,
    pub path: PathBuf,
    /// 0-based.
    pub line: u32,
    pub character: u32,
}

/// `SymbolInformation[]` or `WorkspaceSymbol[]` as symbols with a file;
/// symbols outside the file system (metadata) are left out.
pub fn symbols(v: Value) -> Vec<Symbol> {
    let mut out = Vec::new();
    match serde_json::from_value::<lt::WorkspaceSymbolResponse>(v) {
        Ok(lt::WorkspaceSymbolResponse::Flat(list)) => {
            for s in list {
                if let Some(path) = roslyn::path_from_uri(s.location.uri.as_str()) {
                    out.push(Symbol { name: s.name, container: s.container_name.unwrap_or_default(), kind: s.kind, path, line: s.location.range.start.line, character: s.location.range.start.character });
                }
            }
        }
        Ok(lt::WorkspaceSymbolResponse::Nested(list)) => {
            for s in list {
                let (uri, start) = match s.location {
                    lt::OneOf::Left(l) => (l.uri, l.range.start),
                    lt::OneOf::Right(w) => (w.uri, lt::Position::default()),
                };
                if let Some(path) = roslyn::path_from_uri(uri.as_str()) {
                    out.push(Symbol { name: s.name, container: s.container_name.unwrap_or_default(), kind: s.kind, path, line: start.line, character: start.character });
                }
            }
        }
        Err(_) => {}
    }
    out
}

/// What a workspace edit changes, per file: `changes` and the text edits
/// of `documentChanges` (resource operations are not text edits and are
/// left to the caller).
pub fn workspace_edit_files(edit: &lt::WorkspaceEdit) -> Vec<(PathBuf, Vec<lt::TextEdit>)> {
    let mut out: Vec<(PathBuf, Vec<lt::TextEdit>)> = Vec::new();
    let mut push = |p: PathBuf, edits: Vec<lt::TextEdit>| match out.iter_mut().find(|(q, _)| *q == p) {
        Some((_, e)) => e.extend(edits),
        None => out.push((p, edits)),
    };
    if let Some(changes) = &edit.changes {
        for (u, edits) in changes {
            if let Some(p) = roslyn::path_from_uri(u.as_str()) {
                push(p, edits.clone());
            }
        }
    }
    let ops: Vec<lt::DocumentChangeOperation> = match &edit.document_changes {
        Some(lt::DocumentChanges::Edits(e)) => e.iter().cloned().map(lt::DocumentChangeOperation::Edit).collect(),
        Some(lt::DocumentChanges::Operations(o)) => o.clone(),
        None => Vec::new(),
    };
    for op in ops {
        if let lt::DocumentChangeOperation::Edit(e) = op
            && let Some(p) = roslyn::path_from_uri(e.text_document.uri.as_str())
        {
            let edits = e
                .edits
                .into_iter()
                .map(|x| match x {
                    lt::OneOf::Left(t) => t,
                    lt::OneOf::Right(a) => a.text_edit,
                })
                .collect();
            push(p, edits);
        }
    }
    out
}

/// LSP edits as byte edits on `text` (the buffer's text, LF line ends):
/// Roslyn writes `\r\n` in new text when the file had CRLF on disk; the
/// buffer keeps `\n` and the file's line ending is restored on save.
pub fn byte_edits(text: &str, edits: &[lt::TextEdit]) -> Vec<(usize, usize, String)> {
    edits.iter().map(|e| (to_offset(text, e.range.start), to_offset(text, e.range.end), e.new_text.replace("\r\n", "\n"))).collect()
}

/// `text` with `edits` applied (all ranges refer to the original text).
/// Overlapping edits are invalid LSP; the later one is skipped.
pub fn apply_lsp_edits(text: &str, edits: &[lt::TextEdit]) -> String {
    let mut b = byte_edits(text, edits);
    b.sort_by_key(|e| (e.0, e.1));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (s, e, t) in b {
        if s < at {
            continue;
        }
        out.push_str(&text[at..s]);
        out.push_str(&t);
        at = e.max(s);
    }
    out.push_str(&text[at..]);
    out
}

/// The changed lines a preview shows: `-` old and `+` new lines around the
/// edits, with the lines between edits elided.
pub fn preview_lines(old: &str, new: &str, context: usize) -> Vec<(char, String)> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a.iter().rev().zip(b.iter().rev()).take(a.len().min(b.len()) - prefix).take_while(|(x, y)| x == y).count();
    let mut out = Vec::new();
    for l in &a[prefix.saturating_sub(context)..prefix] {
        out.push((' ', (*l).to_owned()));
    }
    for l in &a[prefix..a.len() - suffix] {
        out.push(('-', (*l).to_owned()));
    }
    for l in &b[prefix..b.len() - suffix] {
        out.push(('+', (*l).to_owned()));
    }
    let tail = a.len() - suffix;
    for l in &a[tail..(tail + context).min(a.len())] {
        out.push((' ', (*l).to_owned()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn loc(path: &str, line: u32, ch: u32) -> Value {
        json!({"uri": format!("file://{path}"), "range": {"start": {"line": line, "character": ch}, "end": {"line": line, "character": ch + 3}}})
    }

    #[test]
    fn all_location_shapes() {
        assert!(locations(Value::Null).is_empty());
        assert_eq!(locations(loc("/a/A.cs", 1, 2)).len(), 1);
        assert_eq!(locations(json!([loc("/a/A.cs", 1, 2), loc("/a/B.cs", 3, 0)])).len(), 2);
        let link = json!([{"targetUri": "file:///a/C.cs", "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}}, "targetSelectionRange": {"start": {"line": 4, "character": 6}, "end": {"line": 4, "character": 9}}}]);
        let l = locations(link);
        assert_eq!(l[0].range.start, lt::Position { line: 4, character: 6 });
    }

    #[test]
    fn references_grouped_sorted_and_deduplicated() {
        let l = locations(json!([loc("/s/B.cs", 9, 0), loc("/s/A.cs", 5, 4), loc("/s/B.cs", 2, 1), loc("/s/B.cs", 2, 1)]));
        let g = group_by_file(&l);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0, PathBuf::from("/s/A.cs"));
        assert_eq!(g[1].1.iter().map(|r| r.start.line).collect::<Vec<_>>(), [2, 9]);
    }

    #[test]
    fn metadata_files() {
        let home = Path::new("/home/t");
        assert!(is_metadata(&roslyn::temp_dir(home).join("MetadataAsSource/x/Console.cs"), home));
        assert!(is_metadata(Path::new("/tmp/MetadataAsSource/abc/Console.cs"), home));
        assert!(!is_metadata(Path::new("/home/t/src/A.cs"), home));
    }

    #[test]
    fn workspace_symbols_both_forms() {
        let flat = json!([{"name": "Circle", "kind": 5, "containerName": "IdeDemo", "location": loc("/s/Shapes.cs", 7, 17)}]);
        let s = symbols(flat);
        assert_eq!((s[0].name.as_str(), s[0].container.as_str(), s[0].line), ("Circle", "IdeDemo", 7));
        let nested = json!([{"name": "Area", "kind": 6, "location": {"uri": "file:///s/Shapes.cs"}}]);
        assert_eq!(symbols(nested)[0].path, PathBuf::from("/s/Shapes.cs"));
    }

    #[test]
    fn edits_apply_with_crlf_new_text() {
        let text = "a\nint   x=1;\n";
        let e = |l1, c1, l2, c2, t: &str| lt::TextEdit { range: lt::Range { start: lt::Position { line: l1, character: c1 }, end: lt::Position { line: l2, character: c2 } }, new_text: t.into() };
        let edits = [e(1, 3, 1, 6, " "), e(1, 7, 1, 8, " = "), e(0, 0, 0, 0, "using X;\r\n")];
        assert_eq!(apply_lsp_edits(text, &edits), "using X;\na\nint x = 1;\n");
    }

    #[test]
    fn workspace_edit_flattens_changes_and_document_changes() {
        let edit: lt::WorkspaceEdit = serde_json::from_value(json!({
            "documentChanges": [{"textDocument": {"uri": "file:///s/A.cs", "version": 3}, "edits": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "using System.Text;\n"}]}]
        }))
        .unwrap();
        let f = workspace_edit_files(&edit);
        assert_eq!(f[0].0, PathBuf::from("/s/A.cs"));
        assert_eq!(f[0].1[0].new_text, "using System.Text;\n");
    }

    #[test]
    fn preview_shows_only_changed_lines_with_context() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nb\nC\nd\ne\n";
        assert_eq!(preview_lines(old, new, 1), vec![(' ', "b".into()), ('-', "c".into()), ('+', "C".into()), (' ', "d".into())]);
        assert_eq!(preview_lines("x\n", "using Y;\nx\n", 1), vec![('+', "using Y;".into()), (' ', "x".into())]);
    }
}
