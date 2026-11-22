//! Moving and renaming files and folders inside and between projects.
//!
//! An SDK-style project includes its files by globbing, so a move is
//! usually only a move on disk. Items that name the path explicitly
//! (`<Compile Remove>`, `<None Update>`, `<EmbeddedResource Include>`, …)
//! follow it: inside one project their path attribute is rewritten; to
//! another project the whole element (with its metadata) moves along. All
//! changes are [`TextEdit`]s, so the rest of each project file keeps its
//! bytes.

use std::ops::Range;
use std::path::Path;

use crate::csproj::Csproj;
use crate::edit::{self, TextEdit};
use crate::xml::{Doc, Id, XmlError, escape_attr};

/// Item kinds whose `Include` is not a path.
const NOT_PATHS: [&str; 7] = ["PackageReference", "PackageVersion", "Reference", "FrameworkReference", "Using", "ProjectCapability", "InternalsVisibleTo"];
const PATH_ATTRS: [&str; 3] = ["Include", "Remove", "Update"];

/// `/`-separated path of `path` relative to `dir`.
pub fn rel(dir: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(dir).ok().map(|r| r.to_string_lossy().replace('\\', "/"))
}

fn norm(part: &str) -> String {
    let p = part.trim().replace('\\', "/");
    p.strip_prefix("./").unwrap_or(&p).to_owned()
}

/// The rest of `part` after the moved path when `part` names it (or, for a
/// folder, something inside it), keeping the case as written.
fn matched_suffix(part: &str, old_rel: &str, is_dir: bool) -> Option<String> {
    let n = norm(part);
    let (nl, ol) = (n.to_lowercase(), old_rel.to_lowercase());
    if nl == ol {
        return Some(String::new());
    }
    if is_dir && nl.starts_with(&format!("{ol}/")) {
        return Some(n[old_rel.len()..].to_owned());
    }
    None
}

/// `new_rel` + `suffix` written with the separators of `like`; VS writes
/// backslashes, so a value without any separator gets those.
fn styled(like: &str, new_rel: &str, suffix: &str) -> String {
    let joined = format!("{new_rel}{suffix}");
    if like.contains('/') && !like.contains('\\') { joined } else { joined.replace('/', "\\") }
}

/// The path-valued item elements of a project, with their path attribute.
fn path_items(doc: &Doc) -> Vec<(Id, usize)> {
    let mut out = Vec::new();
    for g in doc.children_named(0, "ItemGroup") {
        for i in doc.children(g) {
            if NOT_PATHS.contains(&doc.el(i).name.as_str()) {
                continue;
            }
            if let Some(a) = doc.el(i).attrs.iter().position(|a| PATH_ATTRS.contains(&a.name.as_str())) {
                out.push((i, a));
            }
        }
    }
    out
}

/// The `;`-separated parts of a raw attribute value with their ranges.
fn parts(raw: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in raw.char_indices() {
        if c == ';' {
            out.push(start..i);
            start = i + 1;
        }
    }
    out.push(start..raw.len());
    out
}

/// The trimmed part of `raw[r]`, so spaces around `;` stay.
fn trimmed(raw: &str, r: &Range<usize>) -> Range<usize> {
    let s = &raw[r.clone()];
    let lead = s.len() - s.trim_start().len();
    let trail = s.len() - s.trim_end().len();
    r.start + lead..r.end - trail
}

/// Edits that point every explicit item of one project from `old_rel` to
/// `new_rel` (both relative to the project folder, `/`-separated).
pub fn retarget(c: &Csproj, old_rel: &str, new_rel: &str, is_dir: bool) -> Vec<TextEdit> {
    let doc = &c.doc;
    let mut edits = Vec::new();
    for (id, a) in path_items(doc) {
        let span = doc.el(id).attrs[a].value_span.clone();
        let raw = &doc.text[span.clone()];
        for r in parts(raw) {
            let t = trimmed(raw, &r);
            let part = crate::xml::unescape(&raw[t.clone()]);
            if let Some(suffix) = matched_suffix(&part, old_rel, is_dir) {
                edits.push(TextEdit::new(span.start + t.start, span.start + t.end, escape_attr(&styled(&part, new_rel, &suffix))));
            }
        }
    }
    edits
}

/// An item element carried from one project file to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carried {
    /// Element name (`None`, `Compile`, …).
    pub kind: String,
    /// The `Condition` of its `ItemGroup`, kept with it.
    pub condition: Option<String>,
    /// The element as written, its path already pointing at the new place,
    /// continuation lines without the element's own indentation.
    pub text: String,
}

/// Removes an element; when it was the last of its `ItemGroup`, the group
/// goes, with the empty line before it that Anvil and VS put there.
fn remove_with_group(doc: &Doc, id: Id, removed: &[Id]) -> Option<TextEdit> {
    let g = doc.el(id).parent?;
    let all_gone = doc.children(g).all(|c| removed.contains(&c));
    if !all_gone {
        return Some(doc.remove(id));
    }
    // Only one edit for the group, made for its first child.
    if doc.children(g).next() != Some(id) {
        return None;
    }
    let mut e = doc.remove(g);
    if e.start >= 2 && doc.text[..e.start].ends_with("\n\n") {
        e.start -= 1;
    }
    Some(e)
}

/// Takes every explicit item for `old_rel` out of a project: edits for the
/// source and the elements to add to the destination, their path rewritten
/// to `new_rel` (relative to the destination project).
pub fn take(c: &Csproj, old_rel: &str, new_rel: &str, is_dir: bool) -> (Vec<TextEdit>, Vec<Carried>) {
    let doc = &c.doc;
    let mut carried = Vec::new();
    let mut partial = Vec::new();
    let mut whole: Vec<Id> = Vec::new();
    for (id, a) in path_items(doc) {
        let el = doc.el(id);
        let span = el.attrs[a].value_span.clone();
        let raw = &doc.text[span.clone()];
        let mut moved = Vec::new();
        let mut kept = Vec::new();
        for r in parts(raw) {
            let t = trimmed(raw, &r);
            let part = crate::xml::unescape(&raw[t.clone()]);
            match matched_suffix(&part, old_rel, is_dir) {
                Some(suffix) => moved.push(escape_attr(&styled(&part, new_rel, &suffix))),
                None => kept.push(raw[t].to_owned()),
            }
        }
        if moved.is_empty() {
            continue;
        }
        let indent = doc.indent_of(id).to_owned();
        let mut text = String::new();
        text.push_str(&doc.text[el.span.start..span.start]);
        text.push_str(&moved.join(";"));
        text.push_str(&doc.text[span.end..el.span.end]);
        let text = text.split('\n').map(|l| l.strip_prefix(indent.as_str()).unwrap_or(l)).collect::<Vec<_>>().join("\n");
        let condition = el.parent.and_then(|g| doc.attr(g, "Condition")).map(str::to_owned);
        carried.push(Carried { kind: el.name.clone(), condition, text });
        if kept.is_empty() {
            whole.push(id);
        } else {
            partial.push(TextEdit::new(span.start, span.end, kept.join(";")));
        }
    }
    let mut edits: Vec<TextEdit> = whole.iter().filter_map(|&id| remove_with_group(doc, id, &whole)).collect();
    edits.extend(partial);
    (edits, carried)
}

/// Adds carried elements: into an item group of the same kind and
/// condition, otherwise into a new group at the end, as VS adds one.
pub fn add(c: &Csproj, items: &[Carried]) -> Vec<TextEdit> {
    let doc = &c.doc;
    let step = doc.indent_step();
    let mut into: Vec<(Id, Vec<&Carried>)> = Vec::new();
    let mut fresh: Vec<(Option<String>, Vec<&Carried>)> = Vec::new();
    for it in items {
        let group = doc.children_named(0, "ItemGroup").find(|&g| {
            doc.attr(g, "Condition").map(str::to_owned) == it.condition && doc.children(g).next().is_some() && doc.children(g).all(|ch| doc.el(ch).name == it.kind)
        });
        match group {
            Some(g) => match into.iter_mut().find(|(id, _)| *id == g) {
                Some((_, v)) => v.push(it),
                None => into.push((g, vec![it])),
            },
            None => match fresh.iter_mut().find(|(cond, _)| *cond == it.condition) {
                Some((_, v)) => v.push(it),
                None => fresh.push((it.condition.clone(), vec![it])),
            },
        }
    }
    let mut edits = Vec::new();
    for (g, v) in into {
        let last = doc.children(g).last();
        let body = v.iter().map(|i| i.text.as_str()).collect::<Vec<_>>().join("\n");
        edits.push(doc.insert_child(g, last, &body));
    }
    let after = doc.children(0).last();
    for (cond, v) in fresh {
        let open = match cond {
            Some(c) => format!("<ItemGroup Condition=\"{}\">", escape_attr(&c)),
            None => "<ItemGroup>".to_owned(),
        };
        let body: Vec<String> = v.iter().flat_map(|i| i.text.split('\n').map(|l| format!("{step}{l}")).collect::<Vec<_>>()).collect();
        edits.push(doc.insert_child(0, after, &format!("\n{open}\n{}\n</ItemGroup>", body.join("\n"))));
    }
    edits
}

/// The new texts of the project files a move changes: `(source, dest)`,
/// each `None` when it stays as it is. `dest` is `None` for a move inside
/// the source project. A `.cs` file moved into an old-style project (no
/// `Sdk`, so no globbing) gets a `<Compile Include>` there.
pub fn project_changes(src_dir: &Path, src_text: &str, dest: Option<(&Path, &str)>, from: &Path, to: &Path, is_dir: bool) -> Result<(Option<String>, Option<String>), XmlError> {
    let old_rel = rel(src_dir, from).ok_or_else(|| XmlError("the moved path is outside its project".into()))?;
    let src = Csproj::parse(src_text)?;
    let changed = |text: &str, edits: &[TextEdit]| if edits.is_empty() { None } else { Some(edit::apply(text, edits)) };
    let Some((dest_dir, dest_text)) = dest else {
        let new_rel = rel(src_dir, to).ok_or_else(|| XmlError("the target is outside the project".into()))?;
        return Ok((changed(src_text, &retarget(&src, &old_rel, &new_rel, is_dir)), None));
    };
    let new_rel = rel(dest_dir, to).ok_or_else(|| XmlError("the target is outside the project".into()))?;
    let dst = Csproj::parse(dest_text)?;
    let (src_edits, mut carried) = take(&src, &old_rel, &new_rel, is_dir);
    if dst.sdk().is_none() && !is_dir && new_rel.ends_with(".cs") && !carried.iter().any(|c| c.kind == "Compile") {
        carried.push(Carried { kind: "Compile".into(), condition: None, text: format!("<Compile Include=\"{}\" />", escape_attr(&new_rel.replace('/', "\\"))) });
    }
    Ok((changed(src_text, &src_edits), changed(dest_text, &add(&dst, &carried))))
}

/// The range of the namespace name of a C# file, when it declares exactly
/// one top-level namespace (block or file-scoped).
pub fn namespace_range(text: &str) -> Option<Range<usize>> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&crate::csharp::language()).ok()?;
    let tree = parser.parse(text, None)?;
    let root = tree.root_node();
    let mut cursor = root.walk();
    let found: Vec<Range<usize>> = root
        .children(&mut cursor)
        .filter(|n| matches!(n.kind(), "namespace_declaration" | "file_scoped_namespace_declaration"))
        .filter_map(|n| n.child_by_field_name("name").map(|x| x.byte_range()))
        .collect();
    if found.len() == 1 { found.into_iter().next() } else { None }
}

/// The edit that changes a file's namespace to `ns`; `None` when it already
/// has it or its namespace cannot be told.
pub fn namespace_edit(text: &str, ns: &str) -> Option<TextEdit> {
    let r = namespace_range(text)?;
    (text[r.clone()] != *ns).then(|| TextEdit::new(r.start, r.end, ns))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextFile;
    use std::path::PathBuf;

    fn fixture(rel: &str) -> (PathBuf, TextFile) {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel);
        let f = TextFile::read(&p).unwrap();
        (p, f)
    }

    /// A VS-written project with explicit items (CRLF, BOM).
    const EXPLICIT: &str = "moves/Items/Items.csproj";

    #[test]
    fn plain_glob_move_leaves_the_project_untouched() {
        let (p, f) = fixture("doublyLinkedList/doublyLinkedList/doublyLinkedList.csproj");
        let dir = p.parent().unwrap();
        let (src, dst) = project_changes(dir, &f.text, None, &dir.join("Person.cs"), &dir.join("Model/Person.cs"), false).unwrap();
        assert_eq!((src, dst), (None, None));
    }

    #[test]
    fn rename_inside_project_rewrites_only_the_path() {
        let (p, mut f) = fixture(EXPLICIT);
        let before = String::from_utf8(f.encode()).unwrap();
        let dir = p.parent().unwrap();
        let (src, _) = project_changes(dir, &f.text, None, &dir.join("appsettings.json"), &dir.join("Config/appsettings.json"), false).unwrap();
        f.text = src.unwrap();
        let after = String::from_utf8(f.encode()).unwrap();
        assert_eq!(after, before.replace("<None Update=\"appsettings.json\">", "<None Update=\"Config\\appsettings.json\">"));
    }

    #[test]
    fn folder_move_rewrites_globs_and_lists_keeping_separators() {
        let (p, mut f) = fixture(EXPLICIT);
        let before = String::from_utf8(f.encode()).unwrap();
        let dir = p.parent().unwrap();
        let (src, _) = project_changes(dir, &f.text, None, &dir.join("Old"), &dir.join("Archive/Old"), true).unwrap();
        f.text = src.unwrap();
        let after = String::from_utf8(f.encode()).unwrap();
        let expected = before
            .replace("<Compile Remove=\"Old\\**\" />", "<Compile Remove=\"Archive\\Old\\**\" />")
            .replace("<EmbeddedResource Include=\"Old/Strings.resx; Res/Icons.resx\" />", "<EmbeddedResource Include=\"Archive/Old/Strings.resx; Res/Icons.resx\" />")
            .replace("<Folder Include=\"Old\\\" />", "<Folder Include=\"Archive\\Old\\\" />");
        assert_eq!(after, expected);
    }

    #[test]
    fn package_ids_are_never_paths() {
        let (_, f) = fixture(EXPLICIT);
        let c = Csproj::parse(&f.text).unwrap();
        assert!(retarget(&c, "xunit", "Other/xunit", false).is_empty());
    }

    #[test]
    fn move_to_other_project_carries_the_item_and_drops_the_empty_group() {
        let (p, mut src) = fixture(EXPLICIT);
        let (q, mut dst) = fixture("sdk-samples/My Lib/Lib.csproj");
        let src_before = String::from_utf8(src.encode()).unwrap();
        let (sd, dd) = (p.parent().unwrap(), q.parent().unwrap());
        let (s, d) = project_changes(sd, &src.text, Some((dd, &dst.text)), &sd.join("appsettings.json"), &dd.join("appsettings.json"), false).unwrap();
        src.text = s.unwrap();
        dst.text = d.unwrap();
        let src_after = String::from_utf8(src.encode()).unwrap();
        let group = "\r\n  <ItemGroup>\r\n    <None Update=\"appsettings.json\">\r\n      <CopyToOutputDirectory>PreserveNewest</CopyToOutputDirectory>\r\n    </None>\r\n  </ItemGroup>\r\n";
        assert!(src_before.contains(group));
        assert_eq!(src_after, src_before.replacen(group, "", 1));
        let dst_after = String::from_utf8(dst.encode()).unwrap();
        assert!(dst_after.ends_with("  </PropertyGroup>\n\n  <ItemGroup>\n    <None Update=\"appsettings.json\">\n      <CopyToOutputDirectory>PreserveNewest</CopyToOutputDirectory>\n    </None>\n  </ItemGroup>\n\n</Project>\n"), "{dst_after}");
        assert!(dst_after.starts_with('\u{feff}'));
    }

    /// Moving there and back gives both project files their bytes again.
    #[test]
    fn move_and_back_is_byte_identical() {
        let (p, src) = fixture(EXPLICIT);
        let (q, dst) = fixture("sdk-samples/My Lib/Lib.csproj");
        let (sd, dd) = (p.parent().unwrap(), q.parent().unwrap());
        let (s1, d1) = project_changes(sd, &src.text, Some((dd, &dst.text)), &sd.join("appsettings.json"), &dd.join("appsettings.json"), false).unwrap();
        let (d2, s2) = project_changes(dd, &d1.unwrap(), Some((sd, &s1.unwrap())), &dd.join("appsettings.json"), &sd.join("appsettings.json"), false).unwrap();
        let mut back_src = src.clone();
        back_src.text = s2.unwrap();
        let mut back_dst = dst.clone();
        back_dst.text = d2.unwrap();
        assert_eq!(back_src.encode(), std::fs::read(&p).unwrap());
        assert_eq!(back_dst.encode(), std::fs::read(&q).unwrap());
    }

    #[test]
    fn part_of_a_list_moves_alone() {
        let (p, src) = fixture(EXPLICIT);
        let (q, dst) = fixture("sdk-samples/My Lib/Lib.csproj");
        let (sd, dd) = (p.parent().unwrap(), q.parent().unwrap());
        let (s, d) = project_changes(sd, &src.text, Some((dd, &dst.text)), &sd.join("Res/Icons.resx"), &dd.join("Icons.resx"), false).unwrap();
        let s = s.unwrap();
        assert_eq!(s, src.text.replace("Old/Strings.resx; Res/Icons.resx", "Old/Strings.resx"));
        assert!(d.unwrap().contains("    <EmbeddedResource Include=\"Icons.resx\" />\n"));
    }

    #[test]
    fn old_style_destination_gets_a_compile_include() {
        let (q, dst) = fixture(EXPLICIT);
        let (p, src) = fixture("sdk-samples/My Lib/Lib.csproj");
        let legacy = dst.text.replace("<Project Sdk=\"Microsoft.NET.Sdk\">", "<Project ToolsVersion=\"15.0\">");
        let (sd, dd) = (p.parent().unwrap(), q.parent().unwrap());
        let (s, d) = project_changes(sd, &src.text, Some((dd, &legacy)), &sd.join("Class1.cs"), &dd.join("Lib/Class1.cs"), false).unwrap();
        assert_eq!(s, None);
        assert!(d.unwrap().contains("<Compile Include=\"Lib\\Class1.cs\" />"));
    }

    #[test]
    fn namespace_edits() {
        let file_scoped = "using System;\n\nnamespace App.Models;\n\npublic class A { }\n";
        assert_eq!(edit::apply(file_scoped, &[namespace_edit(file_scoped, "Lib.Core").unwrap()]), file_scoped.replace("App.Models", "Lib.Core"));
        let (_, course) = fixture("doublyLinkedList/doublyLinkedList/Person.cs");
        let e = namespace_edit(&course.text, "Other").unwrap();
        assert_eq!(&course.text[e.start..e.end], "doublyLinkedList");
        assert_eq!(namespace_edit(file_scoped, "App.Models"), None);
        assert_eq!(namespace_edit("namespace A { }\nnamespace B { }\n", "C"), None);
        assert_eq!(namespace_edit("class Top { }\n", "C"), None);
    }
}
