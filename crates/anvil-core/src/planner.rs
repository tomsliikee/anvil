//! The edit planner: a Class Designer action becomes text edits on the C#
//! files, built from the structure model, so the change is one undo step
//! and touches only the lines it must.
//!
//! The generated text follows Visual Studio's Class Designer where the
//! course fixture shows it (a new type file: the four `using` lines, block
//! namespace, `public` type, as in `ListNode.cs`) and the design document
//! where it does not (member stubs, *not verified against VS*).

use crate::csharp::{Access, FileStructure, Member, MemberKind, TypeDecl, TypeKind};
use crate::edit::TextEdit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewMember {
    Method,
    Property,
    Field,
    Event,
    Constructor,
    Destructor,
    Constant,
}

impl NewMember {
    /// VS' default name and type for a new member.
    pub fn defaults(self, type_name: &str) -> (String, &'static str) {
        match self {
            NewMember::Method => ("Method".into(), "void"),
            NewMember::Property => ("Property".into(), "int"),
            NewMember::Field => ("Field".into(), "int"),
            NewMember::Event => ("Event".into(), "EventHandler"),
            NewMember::Constructor => (type_name.to_owned(), ""),
            NewMember::Destructor => (format!("~{type_name}"), ""),
            NewMember::Constant => ("Constant".into(), "int"),
        }
    }

    fn kind(self) -> MemberKind {
        match self {
            NewMember::Method => MemberKind::Method,
            NewMember::Property => MemberKind::Property,
            NewMember::Field => MemberKind::Field,
            NewMember::Event => MemberKind::Event,
            NewMember::Constructor => MemberKind::Constructor,
            NewMember::Destructor => MemberKind::Destructor,
            NewMember::Constant => MemberKind::Constant,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PlanError {
    TypeNotFound(String),
    MemberNotFound(String),
    /// The type has no complete body yet (a brace is missing).
    Incomplete(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::TypeNotFound(t) => write!(f, "type {t} not found"),
            PlanError::MemberNotFound(m) => write!(f, "member {m} not found"),
            PlanError::Incomplete(t) => write!(f, "{t} has no complete body"),
        }
    }
}

impl std::error::Error for PlanError {}

/// The text of a new type file as the Class Designer writes it (normalised
/// `\n`; the caller writes it with BOM and CRLF).
pub fn new_type_file(kind: TypeKind, name: &str, namespace: &str, access: Access) -> String {
    let body = match kind {
        TypeKind::Delegate => format!("    {} delegate void {name}();", access.keyword()),
        TypeKind::Record => format!("    {} record {name}\n    {{\n    }}", access.keyword()),
        _ => format!("    {} {} {name}\n    {{\n    }}", access.keyword(), kind.keyword()),
    };
    format!("using System;\nusing System.Collections.Generic;\nusing System.Linq;\nusing System.Text;\n\nnamespace {namespace}\n{{\n{body}\n}}")
}

fn find<'a>(file: &'a FileStructure, full_name: &str) -> Result<&'a TypeDecl, PlanError> {
    file.find(full_name).ok_or_else(|| PlanError::TypeNotFound(full_name.to_owned()))
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn indent_at(text: &str, at: usize) -> String {
    let ls = line_start(text, at);
    text[ls..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// One indentation step of the file: a tab when it indents with tabs.
fn step(text: &str) -> &'static str {
    if text.lines().any(|l| l.starts_with('\t')) { "\t" } else { "    " }
}

/// The member stub text, without indentation on the first line and with
/// `indent` on the following ones.
fn member_text(m: NewMember, name: &str, ty: &str, access: Access, indent: &str, step: &str) -> String {
    let a = access.keyword();
    let i = indent;
    let s = step;
    match m {
        NewMember::Method => format!("{a} {ty} {name}()\n{i}{{\n{i}{s}throw new NotImplementedException();\n{i}}}"),
        NewMember::Property => format!("{a} {ty} {name}\n{i}{{\n{i}{s}get => throw new NotImplementedException();\n{i}{s}set => throw new NotImplementedException();\n{i}}}"),
        NewMember::Field => format!("{a} {ty} {name};"),
        NewMember::Event => format!("{a} event {ty} {name};"),
        NewMember::Constant => format!("{a} const {ty} {name} = 0;"),
        NewMember::Constructor => format!("{a} {name}()\n{i}{{\n{i}}}"),
        NewMember::Destructor => format!("{name}()\n{i}{{\n{i}}}"),
    }
}

/// Adds a member after the last member of the same kind, or at the end of
/// the type; a blank line separates it from multi-line neighbours as VS
/// formats them.
pub fn add_member(text: &str, file: &FileStructure, type_name: &str, m: NewMember, name: &str, ty: &str, access: Access) -> Result<TextEdit, PlanError> {
    let t = find(file, type_name)?;
    let (open, close) = match (t.open_brace, t.close_brace) {
        (Some(o), Some(c)) => (o, c),
        _ => return Err(PlanError::Incomplete(type_name.to_owned())),
    };
    let st = step(text);
    let indent = t.members.first().map(|x| indent_at(text, x.range.start)).unwrap_or_else(|| format!("{}{st}", indent_at(text, t.range.start)));
    let body = member_text(m, name, ty, access, &indent, st);
    let multi = body.contains('\n');
    let same_kind = t.members.iter().rev().find(|x| x.kind == m.kind());
    if let Some(prev) = same_kind.or(t.members.last()) {
        let end = prev.range.end;
        let prev_multi = text[prev.range.clone()].contains('\n');
        let sep = if multi || prev_multi { "\n\n" } else { "\n" };
        return Ok(TextEdit::insert(end, format!("{sep}{indent}{body}")));
    }
    // Empty type: replace the inside of the braces (VS' empty body may hold
    // a blank line where the caret was).
    let close_indent = indent_at(text, close);
    let inner = &text[open + 1..close];
    if inner.trim().is_empty() {
        Ok(TextEdit::new(open + 1, close, format!("\n{indent}{body}\n{close_indent}")))
    } else {
        Ok(TextEdit::insert(line_start(text, close), format!("{indent}{body}\n")))
    }
}

/// Sets the base class (*Inheritance* tool): replaces the first base when
/// it is a class (not `I…`), otherwise puts it first.
pub fn set_base(text: &str, file: &FileStructure, type_name: &str, base: &str) -> Result<TextEdit, PlanError> {
    let t = find(file, type_name)?;
    match &t.base_list {
        Some(range) => {
            let list = &text[range.clone()];
            let first = t.bases.first().cloned().unwrap_or_default();
            let is_interface = |b: &str| b.starts_with('I') && b.chars().nth(1).is_some_and(char::is_uppercase);
            if !first.is_empty() && !is_interface(&first) {
                let off = range.start + list.find(&first).unwrap_or(1);
                Ok(TextEdit::new(off, off + first.len(), base))
            } else {
                let colon = range.start + list.find(':').unwrap_or(0);
                Ok(TextEdit::insert(colon + 1, format!(" {base},").replacen(", ", ",", 0)))
            }
        }
        None => Ok(TextEdit::insert(header_end(text, t), format!(" : {base}"))),
    }
}

/// Adds an interface to the base list (*Implement Interface*).
pub fn add_interface(text: &str, file: &FileStructure, type_name: &str, iface: &str) -> Result<TextEdit, PlanError> {
    let t = find(file, type_name)?;
    match &t.base_list {
        Some(range) => Ok(TextEdit::insert(range.end, format!(", {iface}"))),
        None => {
            let _ = text;
            Ok(TextEdit::insert(header_end(text, t), format!(" : {iface}")))
        }
    }
}

/// Where a base list goes: after the name and type parameters (and a
/// positional record's parameter list).
fn header_end(text: &str, t: &TypeDecl) -> usize {
    let mut end = t.name_range.end;
    let rest = &text[end..];
    let trimmed = rest.trim_start();
    let skip = rest.len() - trimmed.len();
    for (open, close) in [('<', '>'), ('(', ')')] {
        let r = &text[end..];
        let t2 = r.trim_start();
        if t2.starts_with(open) {
            let start = end + (r.len() - t2.len());
            let mut depth = 0;
            for (i, c) in text[start..].char_indices() {
                if c == open {
                    depth += 1;
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i + 1;
                        break;
                    }
                }
            }
        }
    }
    let _ = skip;
    end
}

/// Removes one base (deleting the inheritance or interface line); the
/// whole `: …` goes when it was the only one.
pub fn remove_base(text: &str, file: &FileStructure, type_name: &str, base: &str) -> Result<TextEdit, PlanError> {
    let t = find(file, type_name)?;
    let range = t.base_list.clone().ok_or_else(|| PlanError::MemberNotFound(base.to_owned()))?;
    if !t.bases.iter().any(|b| b == base) {
        return Err(PlanError::MemberNotFound(base.to_owned()));
    }
    if t.bases.len() == 1 {
        // Also the space before the colon.
        let start = text[..range.start].trim_end().len();
        return Ok(TextEdit::new(start, range.end, ""));
    }
    let list = &text[range.clone()];
    let pos = find_word(list, base).ok_or_else(|| PlanError::MemberNotFound(base.to_owned()))?;
    let abs = range.start + pos;
    let after = &text[abs + base.len()..range.end];
    if let Some(comma) = after.find(',').filter(|c| after[..*c].trim().is_empty()) {
        // `A, ` → remove through the comma and following spaces.
        let mut end = abs + base.len() + comma + 1;
        while text[end..].starts_with(' ') {
            end += 1;
        }
        Ok(TextEdit::new(abs, end, ""))
    } else {
        // Last one: remove the preceding `, `.
        let before = text[..abs].trim_end_matches(' ');
        let start = before.strip_suffix(',').map_or(abs, |b| b.len());
        Ok(TextEdit::new(start, abs + base.len(), ""))
    }
}

fn find_word(hay: &str, word: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = hay[from..].find(word) {
        let at = from + i;
        let before = hay[..at].chars().last().is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let after = hay[at + word.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '_' && c != '<');
        if before && after {
            return Some(at);
        }
        from = at + word.len();
    }
    None
}

fn member<'a>(t: &'a TypeDecl, name: &str, kind: Option<MemberKind>) -> Result<&'a Member, PlanError> {
    t.members.iter().find(|m| m.name == name && kind.is_none_or(|k| m.kind == k)).ok_or_else(|| PlanError::MemberNotFound(name.to_owned()))
}

/// Deletes a member's declaration with its line and the blank line before
/// it (*Delete Code*).
pub fn delete_member(text: &str, file: &FileStructure, type_name: &str, name: &str, kind: Option<MemberKind>) -> Result<TextEdit, PlanError> {
    let t = find(file, type_name)?;
    let m = member(t, name, kind)?;
    Ok(delete_lines(text, m.range.start, m.range.end))
}

/// Deletes a type's declaration (the file stays; VS also keeps it).
pub fn delete_type(text: &str, file: &FileStructure, type_name: &str) -> Result<TextEdit, PlanError> {
    let t = find(file, type_name)?;
    Ok(delete_lines(text, t.range.start, t.range.end))
}

fn delete_lines(text: &str, start: usize, end: usize) -> TextEdit {
    let mut s = line_start(text, start);
    let mut e = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
    // Leading doc comments and attributes on their own lines go too.
    loop {
        if s == 0 {
            break;
        }
        let prev_start = line_start(text, s - 1);
        let prev = text[prev_start..s].trim();
        if prev.starts_with("///") || (prev.starts_with('[') && prev.ends_with(']')) {
            s = prev_start;
        } else {
            break;
        }
    }
    // Blank lines around stay as they were, except that no double blank
    // line and no blank line right inside a brace are left behind.
    let prev_line = |at: usize| if at == 0 { None } else { Some(&text[line_start(text, at - 1)..at]) };
    let next_line = |at: usize| text.get(at..).map(|r| r.split('\n').next().unwrap_or(""));
    let before = prev_line(s);
    let after = next_line(e);
    let before_blank = before.is_some_and(|l| l.trim().is_empty());
    let after_blank = after.is_some_and(|l| l.trim().is_empty() && e < text.len());
    let after_close = after.is_some_and(|l| l.trim_start().starts_with('}'));
    let before_open = before.is_some_and(|l| l.trim_end().ends_with('{'));
    if before_blank && (after_blank || after_close) {
        s = line_start(text, s - 1);
    } else if before_open && after_blank {
        e += after.map_or(0, str::len) + 1;
    }
    TextEdit::new(s, e, "")
}

/// Changes a member's declared type (Class Details: Type column).
pub fn set_member_type(text: &str, file: &FileStructure, type_name: &str, name: &str, kind: MemberKind, new_ty: &str) -> Result<TextEdit, PlanError> {
    let t = find(file, type_name)?;
    let m = member(t, name, Some(kind))?;
    let old = m.ty.as_deref().ok_or_else(|| PlanError::MemberNotFound(name.to_owned()))?;
    // The type is written before the name inside the declaration.
    let decl = &text[m.range.start..m.name_range.start];
    let pos = decl.rfind(old).ok_or_else(|| PlanError::MemberNotFound(old.to_owned()))?;
    Ok(TextEdit::new(m.range.start + pos, m.range.start + pos + old.len(), new_ty))
}

/// Changes a member's or type's access modifier (Class Details: Modifier).
pub fn set_access(text: &str, decl_start: usize, decl_end: usize, modifiers: &[String], access: Access) -> TextEdit {
    let decl = &text[decl_start..decl_end];
    let access_words = ["public", "internal", "protected", "private"];
    let present: Vec<&String> = modifiers.iter().filter(|m| access_words.contains(&m.as_str())).collect();
    // Skip attributes: the modifiers start at the first modifier word.
    if let Some(first) = present.first() {
        let start = find_word(decl, first).unwrap_or(0);
        let mut end = start;
        for w in &present {
            if let Some(i) = find_word(&decl[end..], w) {
                end = end + i + w.len();
            }
        }
        TextEdit::new(decl_start + start, decl_start + end, access.keyword())
    } else {
        let first_mod = modifiers.first().and_then(|m| find_word(decl, m));
        let at = first_mod.unwrap_or_else(|| decl.len() - decl.trim_start_matches(|c: char| c == '[' || c.is_whitespace()).len());
        TextEdit::insert(decl_start + at, format!("{} ", access.keyword()))
    }
}

/// Renames a member's declaration only (references are Roslyn's job).
pub fn rename_member_decl(file: &FileStructure, type_name: &str, name: &str, kind: MemberKind, new_name: &str) -> Result<TextEdit, PlanError> {
    let t = find(file, type_name)?;
    let m = member(t, name, Some(kind))?;
    Ok(TextEdit::new(m.name_range.start, m.name_range.end, new_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csharp::parse;
    use crate::edit::apply;
    use crate::text::TextFile;

    fn course(file: &str) -> String {
        TextFile::read(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doublyLinkedList/doublyLinkedList").join(file)).unwrap().text
    }

    fn run(text: &str, e: Result<TextEdit, PlanError>) -> String {
        apply(text, &[e.unwrap()])
    }

    #[test]
    fn add_method_to_empty_vs_class() {
        let src = course("DoublyLinkedPersonList.cs");
        let f = parse(&src);
        let out = run(&src, add_member(&src, &f, "doublyLinkedList.DoublyLinkedPersonList", NewMember::Method, "AddLast", "void", Access::Public));
        assert_eq!(out, "namespace doublyLinkedList\n{\n    internal class DoublyLinkedPersonList\n    {\n        public void AddLast()\n        {\n            throw new NotImplementedException();\n        }\n    }\n}");
    }

    #[test]
    fn add_property_after_last_property() {
        let src = course("Person.cs");
        let f = parse(&src);
        let out = run(&src, add_member(&src, &f, "doublyLinkedList.Person", NewMember::Property, "Age", "int", Access::Public));
        assert!(out.ends_with("                this.sex = value;\n            }\n        }\n\n        public int Age\n        {\n            get => throw new NotImplementedException();\n            set => throw new NotImplementedException();\n        }\n    }\n}"), "{out}");
        assert_eq!(parse(&out).find("doublyLinkedList.Person").unwrap().members.last().unwrap().name, "Age");
    }

    #[test]
    fn add_field_after_last_field() {
        let src = course("Person.cs");
        let f = parse(&src);
        let out = run(&src, add_member(&src, &f, "doublyLinkedList.Person", NewMember::Field, "age", "int", Access::Private));
        assert!(out.contains("        private Sex sex;\n        private int age;\n\n        public Person("), "{out}");
    }

    #[test]
    fn add_method_after_last_method() {
        let src = course("Program.cs");
        let f = parse(&src);
        let out = run(&src, add_member(&src, &f, "doublyLinkedList.Program", NewMember::Method, "Method", "void", Access::Public));
        assert!(out.ends_with("            list.DebugPrint(); // \"Leonardo DiCaprio, 11.11.1974 <--> Shera Danese, 09.10.1949\"\n        }\n\n        public void Method()\n        {\n            throw new NotImplementedException();\n        }\n    }\n}\n"), "{out}");
    }

    #[test]
    fn set_base_writes_colon_list() {
        let src = course("Person.cs");
        let f = parse(&src);
        let out = run(&src, set_base(&src, &f, "doublyLinkedList.Person", "Human"));
        assert!(out.contains("    internal class Person : Human\n    {"));
        let f2 = parse(&out);
        let out2 = run(&out, set_base(&out, &f2, "doublyLinkedList.Person", "Being"));
        assert!(out2.contains("internal class Person : Being\n"));
        let out3 = run(&out2, add_interface(&out2, &parse(&out2), "doublyLinkedList.Person", "IComparable"));
        assert!(out3.contains("internal class Person : Being, IComparable\n"));
    }

    #[test]
    fn remove_base_keeps_formatting() {
        let src = "class A : B, IX, IY\n{\n}";
        let f = parse(src);
        assert_eq!(run(src, remove_base(src, &f, "A", "IX")), "class A : B, IY\n{\n}");
        assert_eq!(run(src, remove_base(src, &f, "A", "IY")), "class A : B, IX\n{\n}");
        assert_eq!(run(src, remove_base(src, &f, "A", "B")), "class A : IX, IY\n{\n}");
        let one = "class A : B\n{\n}";
        assert_eq!(run(one, remove_base(one, &parse(one), "A", "B")), "class A\n{\n}");
    }

    #[test]
    fn interface_first_then_base_class() {
        let src = "class A : IX\n{\n}";
        let out = run(src, set_base(src, &parse(src), "A", "B"));
        assert_eq!(out, "class A : B, IX\n{\n}");
        let generic = "class A<T> where T : new()\n{\n}";
        assert_eq!(run(generic, add_interface(generic, &parse(generic), "A", "IX")), "class A<T> : IX where T : new()\n{\n}");
    }

    #[test]
    fn delete_member_and_type() {
        let src = course("Person.cs");
        let f = parse(&src);
        let out = run(&src, delete_member(&src, &f, "doublyLinkedList.Person", "Sex", Some(MemberKind::Property)));
        assert!(out.ends_with("                this.birthDay = value;\n            }\n\n        }\n    }\n}"), "{out}");
        let out = run(&src, delete_member(&src, &f, "doublyLinkedList.Person", "sex", Some(MemberKind::Field)));
        assert!(out.contains("        private DateTime birthDay;\n\n        public Person("));
        let two = "namespace N\n{\n    class A\n    {\n    }\n\n    class B\n    {\n    }\n}";
        assert_eq!(run(two, delete_type(two, &parse(two), "N.B")), "namespace N\n{\n    class A\n    {\n    }\n}");
    }

    #[test]
    fn change_type_access_and_name() {
        let src = course("Person.cs");
        let f = parse(&src);
        let out = run(&src, set_member_type(&src, &f, "doublyLinkedList.Person", "BirthDay", MemberKind::Property, "DateOnly"));
        assert!(out.contains("        public DateOnly BirthDay\n"));
        let m = &f.find("doublyLinkedList.Person").unwrap().members[0];
        let out = apply(&src, &[set_access(&src, m.range.start, m.range.end, &m.modifiers, Access::Protected)]);
        assert!(out.contains("        protected string firstName;"));
        let out = run(&src, rename_member_decl(&f, "doublyLinkedList.Person", "firstName", MemberKind::Field, "first"));
        assert!(out.contains("private string first;"));
        let prog = course("Program.cs");
        let pf = parse(&prog);
        let main = &pf.types[0].members[0];
        assert!(apply(&prog, &[set_access(&prog, main.range.start, main.range.end, &main.modifiers, Access::Public)]).contains("        public static void Main(string[] args)"));
    }

    #[test]
    fn add_type_file_matches_class_designer() {
        let text = new_type_file(TypeKind::Class, "ListNode", "doublyLinkedList", Access::Public);
        let vs = course("ListNode.cs");
        assert!(vs.starts_with(&text[..text.find("    {").unwrap()]), "header as in VS' ListNode.cs");
        assert!(text.ends_with("    public class ListNode\n    {\n    }\n}"));
    }
}
