//! The C# structure model: types and members of a file with their ranges,
//! from `tree-sitter-c-sharp`. It is fast and tolerant of broken code, so
//! the class diagram and Go To All work while the user types; Roslyn does
//! everything that needs semantics.

use std::ops::Range;

use tree_sitter::{Node, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeKind {
    Class,
    Struct,
    Interface,
    Enum,
    Record,
    RecordStruct,
    Delegate,
}

impl TypeKind {
    pub fn keyword(self) -> &'static str {
        match self {
            TypeKind::Class => "class",
            TypeKind::Struct => "struct",
            TypeKind::Interface => "interface",
            TypeKind::Enum => "enum",
            TypeKind::Record => "record",
            TypeKind::RecordStruct => "record struct",
            TypeKind::Delegate => "delegate",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemberKind {
    Field,
    Constant,
    Property,
    Indexer,
    Method,
    Constructor,
    Destructor,
    Operator,
    Event,
    EnumValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    Public,
    Internal,
    Protected,
    ProtectedInternal,
    PrivateProtected,
    Private,
}

impl Access {
    pub fn keyword(self) -> &'static str {
        match self {
            Access::Public => "public",
            Access::Internal => "internal",
            Access::Protected => "protected",
            Access::ProtectedInternal => "protected internal",
            Access::PrivateProtected => "private protected",
            Access::Private => "private",
        }
    }

    fn from_modifiers(mods: &[String], default: Access) -> Access {
        let has = |m: &str| mods.iter().any(|x| x == m);
        match (has("public"), has("internal"), has("protected"), has("private")) {
            (true, ..) => Access::Public,
            (_, true, true, _) => Access::ProtectedInternal,
            (_, _, true, true) => Access::PrivateProtected,
            (_, true, ..) => Access::Internal,
            (_, _, true, _) => Access::Protected,
            (_, _, _, true) => Access::Private,
            _ => default,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub ty: String,
    pub name: String,
    /// `ref`, `out`, `in`, `params`, `this`.
    pub modifier: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub kind: MemberKind,
    pub name: String,
    /// Field, property, event or return type; `None` for constructors,
    /// destructors and enum values.
    pub ty: Option<String>,
    pub params: Vec<Param>,
    pub modifiers: Vec<String>,
    pub access: Access,
    /// The whole declaration including attributes, comments not included.
    pub range: Range<usize>,
    pub name_range: Range<usize>,
    /// Attribute names without `Attribute` suffix (`TestMethod`, `Fact`).
    pub attributes: Vec<String>,
}

impl Member {
    pub fn is_static(&self) -> bool {
        self.modifiers.iter().any(|m| m == "static" || m == "const")
    }
}

/// How VS' Class Designer shows members (`MembersFormat` in the `.cd`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MembersFormat {
    Name,
    #[default]
    NameAndType,
    FullSignature,
}

impl MembersFormat {
    pub fn parse(s: &str) -> MembersFormat {
        match s {
            "Name" => MembersFormat::Name,
            "FullSignature" => MembersFormat::FullSignature,
            _ => MembersFormat::NameAndType,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MembersFormat::Name => "Name",
            MembersFormat::NameAndType => "NameAndType",
            MembersFormat::FullSignature => "FullSignature",
        }
    }
}

impl Member {
    /// The member's line on a diagram card.
    pub fn display(&self, format: MembersFormat) -> String {
        let callable = matches!(self.kind, MemberKind::Method | MemberKind::Constructor | MemberKind::Destructor | MemberKind::Operator | MemberKind::Indexer);
        let mut s = self.name.clone();
        if callable && format != MembersFormat::Name {
            let params: Vec<String> = self
                .params
                .iter()
                .map(|p| {
                    let ty = match &p.modifier {
                        Some(m) => format!("{m} {}", p.ty),
                        None => p.ty.clone(),
                    };
                    if format == MembersFormat::FullSignature { format!("{ty} {}", p.name) } else { ty }
                })
                .collect();
            let (open, close) = if self.kind == MemberKind::Indexer { ('[', ']') } else { ('(', ')') };
            s = format!("{s}{open}{}{close}", params.join(", "));
        }
        if format != MembersFormat::Name
            && let Some(ty) = &self.ty
        {
            s = format!("{s} : {ty}");
        }
        s
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDecl {
    pub kind: TypeKind,
    pub name: String,
    /// `<T, U>` as written, empty when not generic.
    pub type_params: String,
    pub namespace: String,
    pub modifiers: Vec<String>,
    pub access: Access,
    pub bases: Vec<String>,
    pub members: Vec<Member>,
    pub nested: Vec<TypeDecl>,
    pub range: Range<usize>,
    pub name_range: Range<usize>,
    /// Byte offsets of `{` and `}` of the body (`None` while still typed or
    /// for a delegate / positional record without body).
    pub open_brace: Option<usize>,
    pub close_brace: Option<usize>,
    /// Range of the base list including its `:`.
    pub base_list: Option<Range<usize>>,
    pub is_partial: bool,
}

impl TypeDecl {
    /// `Namespace.Outer+Inner` without generic arity, as the `.cd` names it
    /// (`doublyLinkedList.Person`).
    pub fn full_name(&self, outer: &[&str]) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if !self.namespace.is_empty() {
            parts.push(&self.namespace);
        }
        let mut name = outer.join(".");
        if !name.is_empty() {
            name.push('.');
        }
        name.push_str(&self.name);
        if self.namespace.is_empty() { name } else { format!("{}.{name}", parts[0]) }
    }

    pub fn is_abstract(&self) -> bool {
        self.modifiers.iter().any(|m| m == "abstract")
    }

    pub fn is_static(&self) -> bool {
        self.modifiers.iter().any(|m| m == "static")
    }

    pub fn members_of(&self, kind: MemberKind) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(move |m| m.kind == kind)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileStructure {
    pub types: Vec<TypeDecl>,
    pub usings: Vec<String>,
    /// Whether the file uses `namespace X;`.
    pub file_scoped: bool,
    pub has_errors: bool,
}

impl FileStructure {
    /// Every type including nested ones, with their outer type names.
    pub fn all_types(&self) -> Vec<(Vec<&str>, &TypeDecl)> {
        fn walk<'a>(t: &'a TypeDecl, outer: Vec<&'a str>, out: &mut Vec<(Vec<&'a str>, &'a TypeDecl)>) {
            let mut inner = outer.clone();
            inner.push(&t.name);
            out.push((outer, t));
            for n in &t.nested {
                walk(n, inner.clone(), out);
            }
        }
        let mut out = Vec::new();
        for t in &self.types {
            walk(t, Vec::new(), &mut out);
        }
        out
    }

    pub fn find(&self, full_name: &str) -> Option<&TypeDecl> {
        self.all_types().into_iter().find(|(outer, t)| t.full_name(outer) == full_name).map(|(_, t)| t)
    }
}

pub fn language() -> tree_sitter::Language {
    tree_sitter_c_sharp::LANGUAGE.into()
}

pub fn parse(text: &str) -> FileStructure {
    let mut parser = Parser::new();
    parser.set_language(&language()).expect("the bundled C# grammar matches tree-sitter");
    let Some(tree) = parser.parse(text, None) else { return FileStructure::default() };
    let root = tree.root_node();
    let mut out = FileStructure { has_errors: root.has_error(), ..Default::default() };
    let mut ns = String::new();
    let mut types = Vec::new();
    collect(root, text, &mut ns, &mut types, &mut out);
    out.types = types;
    out
}

fn txt<'a>(n: Node, text: &'a str) -> &'a str {
    &text[n.byte_range()]
}

fn compact(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn modifiers(n: Node, text: &str) -> Vec<String> {
    let mut c = n.walk();
    n.children(&mut c).filter(|ch| ch.kind() == "modifier").map(|m| txt(m, text).to_owned()).collect()
}

fn collect(node: Node, text: &str, ns: &mut String, types: &mut Vec<TypeDecl>, file: &mut FileStructure) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    for child in children {
        match child.kind() {
            "using_directive" => file.usings.push(compact(txt(child, text).trim_start_matches("global ").trim_start_matches("using").trim_end_matches(';').trim())),
            "namespace_declaration" => {
                let name = child.child_by_field_name("name").map(|n| txt(n, text).to_owned()).unwrap_or_default();
                let saved = ns.clone();
                *ns = if ns.is_empty() { name } else { format!("{ns}.{name}") };
                if let Some(body) = child.child_by_field_name("body") {
                    collect(body, text, ns, types, file);
                }
                *ns = saved;
            }
            "file_scoped_namespace_declaration" => {
                file.file_scoped = true;
                *ns = child.child_by_field_name("name").map(|n| txt(n, text).to_owned()).unwrap_or_default();
                // Declarations after `namespace X;` are its children in
                // newer grammars and siblings in older ones.
                collect(child, text, ns, types, file);
            }
            "class_declaration" | "struct_declaration" | "interface_declaration" | "enum_declaration" | "record_declaration" | "record_struct_declaration" | "delegate_declaration" => {
                if let Some(t) = type_decl(child, text, ns) {
                    types.push(t);
                }
            }
            "ERROR" => recover(child, text, ns, types, file),
            "declaration_list" | "global_statement" | "preproc_if" | "preproc_else" | "preproc_elif" | "preproc_region" => collect(child, text, ns, types, file),
            _ => {}
        }
    }
}

/// Error recovery for code being typed: tree-sitter wraps the broken part
/// in an `ERROR` node whose children are the loose tokens. A `namespace`
/// keyword followed by a name opens that namespace; a type keyword followed
/// by a name starts a type that collects the member declarations after it,
/// so the diagram keeps its cards while a brace is missing.
fn recover(node: Node, text: &str, ns: &mut String, types: &mut Vec<TypeDecl>, file: &mut FileStructure) {
    let mut cursor = node.walk();
    let kids: Vec<Node> = node.children(&mut cursor).collect();
    let mut open: Option<TypeDecl> = None;
    let mut i = 0;
    while i < kids.len() {
        let k = kids[i];
        let next_name = kids.get(i + 1).filter(|n| matches!(n.kind(), "identifier" | "qualified_name"));
        match k.kind() {
            "namespace" if next_name.is_some() => {
                *ns = txt(*next_name.unwrap(), text).to_owned();
                i += 2;
                continue;
            }
            "class" | "struct" | "interface" | "enum" | "record" if next_name.is_some() => {
                if let Some(t) = open.take() {
                    types.push(t);
                }
                let name = *next_name.unwrap();
                let kind = match k.kind() {
                    "class" => TypeKind::Class,
                    "struct" => TypeKind::Struct,
                    "interface" => TypeKind::Interface,
                    "enum" => TypeKind::Enum,
                    _ => TypeKind::Record,
                };
                open = Some(TypeDecl {
                    kind,
                    name: txt(name, text).to_owned(),
                    type_params: String::new(),
                    namespace: ns.clone(),
                    modifiers: Vec::new(),
                    access: Access::Internal,
                    bases: Vec::new(),
                    members: Vec::new(),
                    nested: Vec::new(),
                    range: k.start_byte()..node.end_byte(),
                    name_range: name.byte_range(),
                    open_brace: None,
                    close_brace: None,
                    base_list: None,
                    is_partial: false,
                });
                i += 2;
                continue;
            }
            "field_declaration" | "property_declaration" | "method_declaration" | "constructor_declaration" | "event_field_declaration" if open.is_some() => {
                let t = open.as_mut().unwrap();
                let default = if t.kind == TypeKind::Interface { Access::Public } else { Access::Private };
                members_from(&[k], text, t, default);
            }
            "ERROR" => recover(k, text, ns, types, file),
            _ => {
                let mut sub = Vec::new();
                collect_one(k, text, ns, &mut sub, file);
                types.extend(sub);
            }
        }
        i += 1;
    }
    if let Some(t) = open {
        types.push(t);
    }
}

fn collect_one(k: Node, text: &str, ns: &mut String, types: &mut Vec<TypeDecl>, file: &mut FileStructure) {
    match k.kind() {
        "class_declaration" | "struct_declaration" | "interface_declaration" | "enum_declaration" | "record_declaration" | "record_struct_declaration" | "delegate_declaration" => {
            if let Some(t) = type_decl(k, text, ns) {
                types.push(t);
            }
        }
        "namespace_declaration" | "declaration_list" => collect(k, text, ns, types, file),
        _ => {}
    }
}

fn type_decl(n: Node, text: &str, ns: &str) -> Option<TypeDecl> {
    let name_node = n.child_by_field_name("name")?;
    let mods = modifiers(n, text);
    let kind = match n.kind() {
        "class_declaration" => TypeKind::Class,
        "struct_declaration" => TypeKind::Struct,
        "interface_declaration" => TypeKind::Interface,
        "enum_declaration" => TypeKind::Enum,
        "record_struct_declaration" => TypeKind::RecordStruct,
        "record_declaration" => {
            // `record struct` parses as a record_declaration with a `struct` token.
            let mut c = n.walk();
            if n.children(&mut c).any(|ch| ch.kind() == "struct") { TypeKind::RecordStruct } else { TypeKind::Record }
        }
        _ => TypeKind::Delegate,
    };
    let mut c = n.walk();
    let kids: Vec<Node> = n.children(&mut c).collect();
    let type_params = kids.iter().find(|k| k.kind() == "type_parameter_list").map(|k| compact(txt(*k, text))).unwrap_or_default();
    let base_node = kids.iter().find(|k| k.kind() == "base_list").copied();
    let bases = base_node
        .map(|b| {
            let mut bc = b.walk();
            b.named_children(&mut bc).filter(|x| x.kind() != "argument_list").map(|x| compact(txt(x, text))).collect()
        })
        .unwrap_or_default();
    let default_access = Access::Internal;
    let mut t = TypeDecl {
        kind,
        name: txt(name_node, text).to_owned(),
        type_params,
        namespace: ns.to_owned(),
        access: Access::from_modifiers(&mods, default_access),
        is_partial: mods.iter().any(|m| m == "partial"),
        modifiers: mods,
        bases,
        members: Vec::new(),
        nested: Vec::new(),
        range: n.byte_range(),
        name_range: name_node.byte_range(),
        open_brace: None,
        close_brace: None,
        base_list: base_node.map(|b| b.byte_range()),
    };
    if kind == TypeKind::Delegate {
        t.members.push(Member {
            kind: MemberKind::Method,
            name: "Invoke".into(),
            ty: n.child_by_field_name("type").map(|x| compact(txt(x, text))),
            params: n.child_by_field_name("parameters").map(|p| params(p, text)).unwrap_or_default(),
            modifiers: Vec::new(),
            access: Access::Public,
            range: n.byte_range(),
            name_range: name_node.byte_range(),
            attributes: Vec::new(),
        });
        return Some(t);
    }
    // Positional record parameters are properties.
    if let Some(pl) = kids.iter().find(|k| k.kind() == "parameter_list") {
        for p in params(*pl, text) {
            t.members.push(Member { kind: MemberKind::Property, name: p.name.clone(), ty: Some(p.ty.clone()), params: Vec::new(), modifiers: vec!["public".into()], access: Access::Public, range: pl.byte_range(), name_range: pl.byte_range(), attributes: Vec::new() });
        }
    }
    let Some(body) = n.child_by_field_name("body") else { return Some(t) };
    let mut bc = body.walk();
    for k in body.children(&mut bc) {
        match k.kind() {
            "{" => t.open_brace = Some(k.start_byte()),
            "}" if !k.is_missing() => t.close_brace = Some(k.start_byte()),
            _ => {}
        }
    }
    let member_default = if kind == TypeKind::Interface { Access::Public } else { Access::Private };
    members(body, text, &mut t, member_default);
    Some(t)
}

fn members(body: Node, text: &str, t: &mut TypeDecl, default: Access) {
    let mut bc = body.walk();
    let kids: Vec<Node> = body.children(&mut bc).collect();
    members_from(&kids, text, t, default);
}

fn members_from(kids: &[Node], text: &str, t: &mut TypeDecl, default: Access) {
    for &m in kids {
        let mods = modifiers(m, text);
        let attrs = attributes(m, text);
        let access = Access::from_modifiers(&mods, default);
        let ty = m.child_by_field_name("type").or_else(|| m.child_by_field_name("returns")).map(|x| compact(txt(x, text)));
        let mut push = |kind: MemberKind, name: Node, ty: Option<String>, ps: Vec<Param>| {
            t.members.push(Member { kind, name: txt(name, text).to_owned(), ty, params: ps, modifiers: mods.clone(), access, range: m.byte_range(), name_range: name.byte_range(), attributes: attrs.clone() });
        };
        match m.kind() {
            "field_declaration" | "event_field_declaration" => {
                let mut c = m.walk();
                let Some(decl) = m.children(&mut c).find(|x| x.kind() == "variable_declaration") else { continue };
                let vty = decl.child_by_field_name("type").map(|x| compact(txt(x, text)));
                let kind = if m.kind() == "event_field_declaration" {
                    MemberKind::Event
                } else if mods.iter().any(|x| x == "const") {
                    MemberKind::Constant
                } else {
                    MemberKind::Field
                };
                let mut dc = decl.walk();
                for v in decl.children(&mut dc).filter(|x| x.kind() == "variable_declarator") {
                    if let Some(n) = v.child_by_field_name("name") {
                        push(kind, n, vty.clone(), Vec::new());
                    }
                }
            }
            "property_declaration" => {
                if let Some(n) = m.child_by_field_name("name") {
                    push(MemberKind::Property, n, ty, Vec::new());
                }
            }
            "event_declaration" => {
                if let Some(n) = m.child_by_field_name("name") {
                    push(MemberKind::Event, n, ty, Vec::new());
                }
            }
            "method_declaration" => {
                if let Some(n) = m.child_by_field_name("name") {
                    let ps = m.child_by_field_name("parameters").map(|p| params(p, text)).unwrap_or_default();
                    push(MemberKind::Method, n, ty, ps);
                }
            }
            "constructor_declaration" | "destructor_declaration" => {
                if let Some(n) = m.child_by_field_name("name") {
                    let ps = m.child_by_field_name("parameters").map(|p| params(p, text)).unwrap_or_default();
                    let kind = if m.kind() == "constructor_declaration" { MemberKind::Constructor } else { MemberKind::Destructor };
                    push(kind, n, None, ps);
                }
            }
            "indexer_declaration" => {
                let mut c = m.walk();
                let this = m.children(&mut c).find(|x| x.kind() == "this");
                if let Some(this) = this {
                    let ps = m.child_by_field_name("parameters").map(|p| params(p, text)).unwrap_or_default();
                    push(MemberKind::Indexer, this, ty, ps);
                }
            }
            "operator_declaration" | "conversion_operator_declaration" => {
                let ps = m.child_by_field_name("parameters").map(|p| params(p, text)).unwrap_or_default();
                let op = m.child_by_field_name("operator").unwrap_or(m);
                t.members.push(Member {
                    kind: MemberKind::Operator,
                    name: format!("operator {}", txt(op, text)),
                    ty,
                    params: ps,
                    modifiers: mods.clone(),
                    access,
                    range: m.byte_range(),
                    name_range: op.byte_range(),
                    attributes: attrs.clone(),
                });
            }
            "enum_member_declaration" => {
                if let Some(n) = m.child_by_field_name("name") {
                    t.members.push(Member { kind: MemberKind::EnumValue, name: txt(n, text).to_owned(), ty: None, params: Vec::new(), modifiers: Vec::new(), access: Access::Public, range: m.byte_range(), name_range: n.byte_range(), attributes: attrs.clone() });
                }
            }
            "class_declaration" | "struct_declaration" | "interface_declaration" | "enum_declaration" | "record_declaration" | "record_struct_declaration" | "delegate_declaration" => {
                if let Some(nested) = type_decl(m, text, &t.namespace) {
                    t.nested.push(nested);
                }
            }
            "ERROR" | "preproc_if" | "preproc_region" => members(m, text, t, default),
            _ => {}
        }
    }
}

fn attributes(n: Node, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut c = n.walk();
    for list in n.children(&mut c).filter(|k| k.kind() == "attribute_list") {
        let mut lc = list.walk();
        for a in list.children(&mut lc).filter(|k| k.kind() == "attribute") {
            if let Some(name) = a.child_by_field_name("name") {
                let full = txt(name, text);
                let last = full.rsplit('.').next().unwrap_or(full);
                out.push(last.strip_suffix("Attribute").unwrap_or(last).to_owned());
            }
        }
    }
    out
}

fn params(list: Node, text: &str) -> Vec<Param> {
    let mut c = list.walk();
    list.children(&mut c)
        .filter(|p| p.kind() == "parameter")
        .map(|p| {
            let mut pc = p.walk();
            let modifier = p.children(&mut pc).find(|x| x.kind() == "modifier" || matches!(x.kind(), "ref" | "out" | "in" | "params" | "this")).map(|x| txt(x, text).to_owned());
            Param {
                ty: p.child_by_field_name("type").map(|x| compact(txt(x, text))).unwrap_or_default(),
                name: p.child_by_field_name("name").map(|x| txt(x, text).to_owned()).unwrap_or_default(),
                modifier,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextFile;

    fn course(file: &str) -> FileStructure {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doublyLinkedList/doublyLinkedList").join(file);
        parse(&TextFile::read(&p).unwrap().text)
    }

    #[test]
    fn course_person_members() {
        let f = course("Person.cs");
        let p = &f.types[0];
        assert_eq!(p.full_name(&[]), "doublyLinkedList.Person");
        assert_eq!(p.access, Access::Internal);
        let names: Vec<(&str, MemberKind)> = p.members.iter().map(|m| (m.name.as_str(), m.kind)).collect();
        assert_eq!(names[0], ("firstName", MemberKind::Field));
        assert_eq!(names[4], ("Person", MemberKind::Constructor));
        assert_eq!(names[5], ("FirstName", MemberKind::Property));
        assert_eq!(names.len(), 9);
        assert_eq!(p.members[0].access, Access::Private);
        assert_eq!(p.members[4].display(MembersFormat::FullSignature), "Person(string firstName, string lastName, DateTime birthDay, Sex sex)");
        assert_eq!(p.members[5].display(MembersFormat::FullSignature), "FirstName : string");
        assert!(p.open_brace.is_some() && p.close_brace.is_some());
    }

    #[test]
    fn course_program_and_sex() {
        let f = course("Program.cs");
        let m = &f.types[0].members[0];
        assert_eq!(m.display(MembersFormat::FullSignature), "Main(string[] args) : void");
        assert_eq!(m.display(MembersFormat::NameAndType), "Main(string[]) : void");
        assert!(m.is_static());
        let s = course("Sex.cs");
        assert_eq!(s.types[0].kind, TypeKind::Enum);
        let values: Vec<&str> = s.types[0].members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(values, ["Male", "Female"]);
    }

    #[test]
    fn course_list_node_usings() {
        let f = course("ListNode.cs");
        assert_eq!(f.usings, ["System", "System.Collections.Generic", "System.Linq", "System.Text"]);
        assert_eq!(f.types[0].access, Access::Public);
        assert_eq!(f.types[0].members.len(), 7);
    }

    #[test]
    fn nested_partial_generic_and_bases() {
        let f = parse("namespace A.B {\n public partial class Box<T> : Base, IDisposable where T : new() {\n  class Inner { int x; }\n  public event EventHandler Changed;\n  public T this[int i] => default;\n  ~Box() {}\n }\n}");
        let t = &f.types[0];
        assert_eq!(t.type_params, "<T>");
        assert_eq!(t.bases, ["Base", "IDisposable"]);
        assert!(t.is_partial);
        assert_eq!(t.nested[0].name, "Inner");
        let all: Vec<String> = f.all_types().iter().map(|(o, t)| t.full_name(o)).collect();
        assert_eq!(all, ["A.B.Box", "A.B.Box.Inner"]);
        let kinds: Vec<MemberKind> = t.members.iter().map(|m| m.kind).collect();
        assert_eq!(kinds, [MemberKind::Event, MemberKind::Indexer, MemberKind::Destructor]);
    }

    #[test]
    fn file_scoped_namespace() {
        let f = parse("namespace App;\n\ninternal record Point(int X, int Y);\npublic interface IShape { double Area(); }\n");
        assert!(f.file_scoped);
        assert_eq!(f.types[0].full_name(&[]), "App.Point");
        assert_eq!(f.types[0].members.len(), 2);
        assert_eq!(f.types[1].members[0].access, Access::Public);
    }

    #[test]
    fn tolerates_unclosed_class() {
        let f = parse("namespace N\n{\n    class A\n    {\n        void M() {}\n    }\n\n    class B\n    {\n        int x;\n        void Broken( {\n");
        assert!(f.has_errors);
        let names: Vec<&str> = f.all_types().iter().map(|(_, t)| t.name.as_str()).collect();
        assert!(names.contains(&"A"), "{names:?}");
        assert_eq!(f.find("N.A").unwrap().members[0].name, "M");
        let b = f.find("N.B").expect("the half-typed class keeps its card");
        assert_eq!(b.members[0].name, "x");
    }
}
