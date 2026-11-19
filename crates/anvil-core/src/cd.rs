//! Visual Studio Class Designer files (`.cd`). Read losslessly: every
//! change is a text edit on the original, so unknown elements, attribute
//! order and VS' formatting survive and an unchanged diagram is written
//! back byte for byte (UTF-8 with BOM, CRLF, as VS writes it).

use crate::csharp::{MembersFormat, TypeKind};
use crate::edit::{apply, TextEdit};
use crate::text::{Encoding, Eol, TextFile};
use crate::xml::{Doc, Id, XmlError};

/// What VS writes for Add → Class Diagram: UTF-8 with BOM, CRLF and no
/// line break after the last line (as in the course's `ClassDiagram1.cd`).
pub const EMPTY_DIAGRAM: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ClassDiagram MajorVersion=\"1\" MinorVersion=\"1\">\n  <Font Name=\"Segoe UI\" Size=\"9\" />\n</ClassDiagram>";

/// VS' default width of a new shape, in inches.
pub const DEFAULT_WIDTH: f64 = 1.5;
pub const DPI: f64 = 96.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Class,
    Interface,
    Enum,
    Struct,
    Delegate,
    Comment,
}

impl ShapeKind {
    pub fn element(self) -> &'static str {
        match self {
            ShapeKind::Class => "Class",
            ShapeKind::Interface => "Interface",
            ShapeKind::Enum => "Enum",
            ShapeKind::Struct => "Struct",
            ShapeKind::Delegate => "Delegate",
            ShapeKind::Comment => "Comment",
        }
    }

    fn from_element(name: &str) -> Option<ShapeKind> {
        Some(match name {
            "Class" => ShapeKind::Class,
            "Interface" => ShapeKind::Interface,
            "Enum" => ShapeKind::Enum,
            "Struct" => ShapeKind::Struct,
            "Delegate" => ShapeKind::Delegate,
            "Comment" => ShapeKind::Comment,
            _ => return None,
        })
    }

    /// The shape VS uses for a C# type (records are classes, record
    /// structs are structs in the Class Designer).
    pub fn for_type(kind: TypeKind) -> ShapeKind {
        match kind {
            TypeKind::Class | TypeKind::Record => ShapeKind::Class,
            TypeKind::Struct | TypeKind::RecordStruct => ShapeKind::Struct,
            TypeKind::Interface => ShapeKind::Interface,
            TypeKind::Enum => ShapeKind::Enum,
            TypeKind::Delegate => ShapeKind::Delegate,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub id: Id,
    pub kind: ShapeKind,
    /// Full type name (`doublyLinkedList.Person`); the text for a comment.
    pub name: String,
    /// Inches.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: Option<f64>,
    pub hash: Option<String>,
    pub file: Option<String>,
    pub collapsed: bool,
    /// Compartments VS shows folded (`Fields`, `Methods`, …).
    pub collapsed_compartments: Vec<String>,
    pub show_as_association: Vec<String>,
    pub show_as_collection: Vec<String>,
    pub has_lollipop: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// `AssociationLine` (with the member name) or `InheritanceLine`.
    pub from: String,
    pub member: Option<String>,
    pub to: String,
    pub points: Vec<(f64, f64)>,
}

#[derive(Clone, Debug)]
pub struct Diagram {
    pub file: TextFile,
    pub doc: Doc,
}

/// Formats inches the way VS does: shortest decimal, no trailing zeros
/// (`3.5`, `11`, `8.25`, `-0.5`).
pub fn fmt_inches(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    let r = if r == 0.0 { 0.0 } else { r };
    format!("{r}")
}

pub fn inches_to_px(v: f64) -> f64 {
    v * DPI
}

pub fn px_to_inches(v: f64) -> f64 {
    v / DPI
}

/// Child order inside a shape, as VS writes it.
const CHILD_ORDER: &[&str] = &["Position", "Compartments", "NestedTypes", "AssociationLine", "InheritanceLine", "TypeIdentifier", "ShowAsAssociation", "ShowAsCollectionAssociation", "Lollipop"];

impl Diagram {
    pub fn parse(bytes: &[u8]) -> Result<Diagram, XmlError> {
        let file = TextFile::decode(bytes);
        let doc = Doc::parse(&file.text)?;
        if doc.el(0).name != "ClassDiagram" {
            return Err(XmlError("not a class diagram".into()));
        }
        Ok(Diagram { file, doc })
    }

    pub fn new_empty() -> Diagram {
        let file = TextFile { encoding: Encoding::Utf8 { bom: true }, eol: Eol::Crlf, text: EMPTY_DIAGRAM.to_owned() };
        let doc = Doc::parse(&file.text).expect("the empty diagram parses");
        Diagram { file, doc }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.file.encode()
    }

    /// Applies edits and re-reads the document.
    pub fn apply(&mut self, edits: &[TextEdit]) {
        if edits.is_empty() {
            return;
        }
        self.file.text = apply(&self.file.text, edits);
        self.doc = Doc::parse(&self.file.text).expect("edits keep the diagram well-formed");
    }

    pub fn members_format(&self) -> MembersFormat {
        self.doc.attr(0, "MembersFormat").map(MembersFormat::parse).unwrap_or_default()
    }

    pub fn set_members_format(&mut self, f: MembersFormat) {
        let e = self.doc.set_attr(0, "MembersFormat", f.as_str());
        self.apply(&[e]);
    }

    pub fn shapes(&self) -> Vec<Shape> {
        self.doc.children(0).filter_map(|id| self.shape(id)).collect()
    }

    fn shape(&self, id: Id) -> Option<Shape> {
        let d = &self.doc;
        let kind = ShapeKind::from_element(&d.el(id).name)?;
        let pos = d.child(id, "Position");
        let num = |name: &str| pos.and_then(|p| d.attr(p, name)).and_then(|v| v.parse::<f64>().ok());
        let ti = d.child(id, "TypeIdentifier");
        let names_in = |el: &str| -> Vec<String> {
            d.child(id, el).map(|s| d.children(s).filter_map(|p| d.attr(p, "Name").map(str::to_owned)).collect()).unwrap_or_default()
        };
        let name = if kind == ShapeKind::Comment { d.attr(id, "CommentText").unwrap_or_default().to_owned() } else { d.attr(id, "Name")?.to_owned() };
        Some(Shape {
            id,
            kind,
            name,
            x: num("X").unwrap_or(0.0),
            y: num("Y").unwrap_or(0.0),
            width: num("Width").unwrap_or(DEFAULT_WIDTH),
            height: num("Height"),
            hash: ti.and_then(|t| d.child(t, "HashCode")).map(|h| d.inner_text(h)),
            file: ti.and_then(|t| d.child(t, "FileName")).map(|h| d.inner_text(h)),
            collapsed: d.attr(id, "Collapsed") == Some("true"),
            collapsed_compartments: d
                .child(id, "Compartments")
                .map(|c| d.children(c).filter(|&x| d.attr(x, "Collapsed") == Some("true")).filter_map(|x| d.attr(x, "Name").map(str::to_owned)).collect())
                .unwrap_or_default(),
            show_as_association: names_in("ShowAsAssociation"),
            show_as_collection: names_in("ShowAsCollectionAssociation"),
            has_lollipop: d.child(id, "Lollipop").is_some(),
        })
    }

    pub fn find(&self, name: &str) -> Option<Shape> {
        self.shapes().into_iter().find(|s| s.name == name && s.kind != ShapeKind::Comment)
    }

    pub fn lines(&self) -> Vec<Line> {
        let d = &self.doc;
        let mut out = Vec::new();
        for s in self.shapes() {
            for l in d.children(s.id).filter(|&c| matches!(d.el(c).name.as_str(), "AssociationLine" | "InheritanceLine")) {
                let points = d
                    .child(l, "Path")
                    .map(|p| {
                        d.children(p)
                            .filter_map(|pt| Some((d.attr(pt, "X")?.parse().ok()?, d.attr(pt, "Y")?.parse().ok()?)))
                            .collect()
                    })
                    .unwrap_or_default();
                out.push(Line { from: s.name.clone(), member: d.attr(l, "Name").map(str::to_owned), to: d.attr(l, "Type").unwrap_or_default().to_owned(), points });
            }
        }
        out
    }

    fn shape_id(&self, name: &str) -> Option<Id> {
        self.doc.children(0).find(|&c| ShapeKind::from_element(&self.doc.el(c).name).is_some() && self.doc.attr(c, "Name") == Some(name))
    }

    /// Moves a shape; only its `Position` attributes change.
    pub fn move_shape(&mut self, name: &str, x: f64, y: f64) {
        let Some(id) = self.shape_id(name) else { return };
        let Some(pos) = self.doc.child(id, "Position") else { return };
        let edits = [self.doc.set_attr(pos, "X", &fmt_inches(x)), self.doc.set_attr(pos, "Y", &fmt_inches(y))];
        self.apply(&edits);
    }

    pub fn set_width(&mut self, name: &str, width: f64) {
        let Some(id) = self.shape_id(name) else { return };
        let Some(pos) = self.doc.child(id, "Position") else { return };
        let e = self.doc.set_attr(pos, "Width", &fmt_inches(width));
        self.apply(&[e]);
    }

    pub fn set_collapsed(&mut self, name: &str, collapsed: bool) {
        let Some(id) = self.shape_id(name) else { return };
        let e = match (self.doc.el(id).attrs.iter().find(|a| a.name == "Collapsed").is_some(), collapsed) {
            (_, true) => self.doc.set_attr(id, "Collapsed", "true"),
            (true, false) => {
                // Remove ` Collapsed="true"` from the start tag.
                let a = self.doc.el(id).attrs.iter().find(|a| a.name == "Collapsed").expect("checked");
                let start = self.doc.text[..a.value_span.start].rfind(" Collapsed=").unwrap_or(a.value_span.start);
                TextEdit::new(start, a.value_span.end + 1, "")
            }
            (false, false) => return,
        };
        self.apply(&[e]);
    }

    pub fn set_hash(&mut self, name: &str, hash: &str) {
        let Some(id) = self.shape_id(name) else { return };
        let Some(h) = self.doc.child(id, "TypeIdentifier").and_then(|t| self.doc.child(t, "HashCode")) else { return };
        if self.doc.inner_text(h) == hash {
            return;
        }
        let e = self.doc.set_text(h, hash);
        self.apply(&[e]);
    }

    pub fn set_file(&mut self, name: &str, file: &str) {
        let Some(id) = self.shape_id(name) else { return };
        let Some(f) = self.doc.child(id, "TypeIdentifier").and_then(|t| self.doc.child(t, "FileName")) else { return };
        let e = self.doc.set_text(f, file);
        self.apply(&[e]);
    }

    /// Renames a shape (after a type rename in code).
    pub fn rename(&mut self, old: &str, new: &str) {
        let Some(id) = self.shape_id(old) else { return };
        let e = self.doc.set_attr(id, "Name", new);
        self.apply(&[e]);
    }

    /// Adds a type shape before `<Font>`, as VS writes it.
    pub fn add_shape(&mut self, kind: ShapeKind, name: &str, x: f64, y: f64, hash: &str, file: &str) {
        let fragment = format!(
            "<{el} Name=\"{name}\">\n  <Position X=\"{x}\" Y=\"{y}\" Width=\"{w}\" />\n  <TypeIdentifier>\n    <HashCode>{hash}</HashCode>\n    <FileName>{file}</FileName>\n  </TypeIdentifier>\n</{el}>",
            el = kind.element(),
            name = crate::xml::escape_attr(name),
            x = fmt_inches(x),
            y = fmt_inches(y),
            w = fmt_inches(DEFAULT_WIDTH),
            file = crate::xml::escape_text(file),
        );
        self.insert_top_level(&fragment);
    }

    /// Adds a comment shape (`<Comment CommentText="…">`).
    pub fn add_comment(&mut self, text: &str, x: f64, y: f64) {
        let fragment = format!(
            "<Comment CommentText=\"{}\">\n  <Position X=\"{}\" Y=\"{}\" Height=\"0.75\" Width=\"1.7\" />\n</Comment>",
            crate::xml::escape_attr(text),
            fmt_inches(x),
            fmt_inches(y)
        );
        self.insert_top_level(&fragment);
    }

    fn insert_top_level(&mut self, fragment: &str) {
        // Shapes go after the last shape, i.e. before <Font>.
        let last_shape = self.doc.children(0).filter(|&c| self.doc.el(c).name != "Font").last();
        let e = match last_shape {
            Some(id) => self.doc.insert_child(0, Some(id), fragment),
            None => {
                let font = self.doc.child(0, "Font");
                match font {
                    // Insert before Font's line.
                    Some(f) => {
                        let start = self.doc.el(f).span.start;
                        let line_start = self.doc.text[..start].rfind('\n').map_or(0, |i| i + 1);
                        let indent = self.doc.indent_of(f).to_owned();
                        let body: String = fragment.lines().map(|l| format!("{indent}{l}\n")).collect();
                        TextEdit::insert(line_start, body)
                    }
                    None => self.doc.insert_child(0, None, fragment),
                }
            }
        };
        self.apply(&[e]);
    }

    pub fn remove_shape(&mut self, name: &str) {
        let Some(id) = self.shape_id(name) else { return };
        // Lines from other shapes to this one go too, as in VS.
        let mut edits = vec![self.doc.remove(id)];
        for other in self.doc.children(0) {
            if other == id {
                continue;
            }
            for l in self.doc.children(other) {
                if matches!(self.doc.el(l).name.as_str(), "AssociationLine" | "InheritanceLine") && self.doc.attr(l, "Type") == Some(name) {
                    edits.push(self.doc.remove(l));
                }
            }
        }
        self.apply(&edits);
    }

    /// Inserts a child element into a shape at VS' position for its kind.
    fn insert_in_shape(&mut self, id: Id, element: &str, fragment: &str) {
        let rank = |n: &str| CHILD_ORDER.iter().position(|o| *o == n).unwrap_or(CHILD_ORDER.len());
        let mine = rank(element);
        let after = self.doc.children(id).filter(|&c| rank(&self.doc.el(c).name) <= mine).last();
        let e = match after {
            Some(a) => self.doc.insert_child(id, Some(a), fragment),
            None => {
                // Before the first child.
                let first = self.doc.children(id).next();
                match first {
                    Some(f) => {
                        let start = self.doc.el(f).span.start;
                        let line_start = self.doc.text[..start].rfind('\n').map_or(0, |i| i + 1);
                        let indent = self.doc.indent_of(f).to_owned();
                        TextEdit::insert(line_start, fragment.lines().map(|l| format!("{indent}{l}\n")).collect::<String>())
                    }
                    None => self.doc.insert_child(id, None, fragment),
                }
            }
        };
        self.apply(&[e]);
    }

    /// *Show as Association* / *Show as Collection Association* on a
    /// property or field of `shape`.
    pub fn show_as_association(&mut self, shape: &str, member: &str, collection: bool) {
        let Some(id) = self.shape_id(shape) else { return };
        let el = if collection { "ShowAsCollectionAssociation" } else { "ShowAsAssociation" };
        let item = format!("<Property Name=\"{}\" />", crate::xml::escape_attr(member));
        match self.doc.child(id, el) {
            Some(list) => {
                let last = self.doc.children(list).last();
                let e = self.doc.insert_child(list, last, &item);
                self.apply(&[e]);
            }
            None => self.insert_in_shape(id, el, &format!("<{el}>\n  {item}\n</{el}>")),
        }
    }

    /// Back from an association line to a member row.
    pub fn show_as_member(&mut self, shape: &str, member: &str) {
        let Some(id) = self.shape_id(shape) else { return };
        let mut edits = Vec::new();
        for el in ["ShowAsAssociation", "ShowAsCollectionAssociation"] {
            if let Some(list) = self.doc.child(id, el) {
                let props: Vec<Id> = self.doc.children(list).collect();
                let hit: Vec<Id> = props.iter().copied().filter(|&p| self.doc.attr(p, "Name") == Some(member)).collect();
                if hit.is_empty() {
                    continue;
                }
                if hit.len() == props.len() {
                    edits.push(self.doc.remove(list));
                } else {
                    edits.extend(hit.into_iter().map(|p| self.doc.remove(p)));
                }
            }
        }
        for l in self.doc.children_named(id, "AssociationLine") {
            if self.doc.attr(l, "Name") == Some(member) {
                edits.push(self.doc.remove(l));
            }
        }
        self.apply(&edits);
    }

    pub fn set_compartment_collapsed(&mut self, shape: &str, compartment: &str, collapsed: bool) {
        let Some(id) = self.shape_id(shape) else { return };
        let existing = self.doc.child(id, "Compartments").and_then(|c| self.doc.children(c).find(|&x| self.doc.attr(x, "Name") == Some(compartment)));
        match (existing, collapsed) {
            (Some(x), true) => {
                let e = self.doc.set_attr(x, "Collapsed", "true");
                self.apply(&[e]);
            }
            (Some(x), false) => {
                let list = self.doc.el(x).parent.expect("compartment has a parent");
                let e = if self.doc.children(list).count() == 1 { self.doc.remove(list) } else { self.doc.remove(x) };
                self.apply(&[e]);
            }
            (None, true) => {
                let item = format!("<Compartment Name=\"{compartment}\" Collapsed=\"true\" />");
                match self.doc.child(id, "Compartments") {
                    Some(list) => {
                        let last = self.doc.children(list).last();
                        let e = self.doc.insert_child(list, last, &item);
                        self.apply(&[e]);
                    }
                    None => self.insert_in_shape(id, "Compartments", &format!("<Compartments>\n  {item}\n</Compartments>")),
                }
            }
            (None, false) => {}
        }
    }

    pub fn font(&self) -> (String, f64) {
        let f = self.doc.child(0, "Font");
        let name = f.and_then(|f| self.doc.attr(f, "Name")).unwrap_or("Segoe UI").to_owned();
        let size = f.and_then(|f| self.doc.attr(f, "Size")).and_then(|s| s.parse().ok()).unwrap_or(9.0);
        (name, size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn course_bytes() -> Vec<u8> {
        std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doublyLinkedList/doublyLinkedList/ClassDiagram1.cd")).unwrap()
    }

    #[test]
    fn unchanged_diagram_identical_bytes() {
        let d = Diagram::parse(&course_bytes()).unwrap();
        assert_eq!(d.to_bytes(), course_bytes());
    }

    #[test]
    fn reads_course_shapes() {
        let d = Diagram::parse(&course_bytes()).unwrap();
        let s = d.shapes();
        assert_eq!(s.len(), 5);
        assert_eq!(s[0].name, "doublyLinkedList.Person");
        assert_eq!((s[0].x, s[0].y, s[0].width), (3.5, 8.25, 4.5));
        assert_eq!(s[4].kind, ShapeKind::Enum);
        assert_eq!(s[1].hash.as_deref(), Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAAAAA="));
        assert_eq!(s[1].file.as_deref(), Some("Program.cs"));
        assert_eq!(d.members_format(), MembersFormat::FullSignature);
        assert_eq!(d.font(), ("Segoe UI".to_owned(), 9.0));
    }

    #[test]
    fn moving_one_card_changes_only_its_position() {
        let mut d = Diagram::parse(&course_bytes()).unwrap();
        d.move_shape("doublyLinkedList.Program", 10.75, 3.25);
        let expected = String::from_utf8(course_bytes()).unwrap().replace("<Position X=\"11\" Y=\"3.5\" Width=\"1.5\" />", "<Position X=\"10.75\" Y=\"3.25\" Width=\"1.5\" />");
        assert_eq!(String::from_utf8(d.to_bytes()).unwrap(), expected);
    }

    #[test]
    fn new_empty_matches_vs() {
        let bytes = Diagram::new_empty().to_bytes();
        assert_eq!(bytes, b"\xEF\xBB\xBF<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<ClassDiagram MajorVersion=\"1\" MinorVersion=\"1\">\r\n  <Font Name=\"Segoe UI\" Size=\"9\" />\r\n</ClassDiagram>");
    }

    #[test]
    fn added_class_shape_matches_vs_layout() {
        let mut d = Diagram::new_empty();
        d.add_shape(ShapeKind::Class, "N.Car", 0.5, 0.75, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", "Car.cs");
        let s = String::from_utf8(d.to_bytes()).unwrap();
        assert!(s.contains("<ClassDiagram MajorVersion=\"1\" MinorVersion=\"1\">\r\n  <Class Name=\"N.Car\">\r\n    <Position X=\"0.5\" Y=\"0.75\" Width=\"1.5\" />\r\n    <TypeIdentifier>\r\n      <HashCode>AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=</HashCode>\r\n      <FileName>Car.cs</FileName>\r\n    </TypeIdentifier>\r\n  </Class>\r\n  <Font Name=\"Segoe UI\" Size=\"9\" />\r\n</ClassDiagram>"), "{s}");
        d.add_shape(ShapeKind::Enum, "N.Color", 3.0, 0.75, "x", "Color.cs");
        assert_eq!(d.shapes().len(), 2);
        assert!(d.to_bytes().ends_with(b"  </Enum>\r\n  <Font Name=\"Segoe UI\" Size=\"9\" />\r\n</ClassDiagram>"));
    }

    #[test]
    fn unknown_elements_and_associations_survive() {
        let src = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<ClassDiagram MajorVersion=\"1\" MinorVersion=\"1\">\r\n  <Comment CommentText=\"Hi\">\r\n    <Position X=\"1\" Y=\"1\" Height=\"0.75\" Width=\"1.7\" />\r\n  </Comment>\r\n  <Class Name=\"A.B\">\r\n    <Position X=\"2\" Y=\"2\" Width=\"1.5\" />\r\n    <AssociationLine Name=\"Next\" Type=\"A.B\" FixedFromPoint=\"true\">\r\n      <Path>\r\n        <Point X=\"3.5\" Y=\"2.5\" />\r\n        <Point X=\"4\" Y=\"2.5\" />\r\n      </Path>\r\n    </AssociationLine>\r\n    <TypeIdentifier>\r\n      <HashCode>AAA=</HashCode>\r\n      <FileName>B.cs</FileName>\r\n    </TypeIdentifier>\r\n    <ShowAsAssociation>\r\n      <Property Name=\"Next\" />\r\n    </ShowAsAssociation>\r\n    <Lollipop Position=\"0.2\" />\r\n    <Future Thing=\"1\" />\r\n  </Class>\r\n  <Font Name=\"Segoe UI\" Size=\"9\" />\r\n</ClassDiagram>";
        let mut d = Diagram::parse(src.as_bytes()).unwrap();
        assert_eq!(d.to_bytes(), src.as_bytes());
        let b = d.find("A.B").unwrap();
        assert_eq!(b.show_as_association, ["Next"]);
        assert!(b.has_lollipop);
        assert_eq!(d.lines()[0].points, [(3.5, 2.5), (4.0, 2.5)]);
        assert_eq!(d.shapes()[0].kind, ShapeKind::Comment);
        d.move_shape("A.B", 2.25, 2.0);
        assert!(String::from_utf8(d.to_bytes()).unwrap().contains("<Future Thing=\"1\" />"));
        d.show_as_member("A.B", "Next");
        let s = String::from_utf8(d.to_bytes()).unwrap();
        assert!(!s.contains("ShowAsAssociation") && !s.contains("AssociationLine"), "{s}");
        assert!(s.contains("    <Lollipop Position=\"0.2\" />\r\n"));
    }

    #[test]
    fn compartments_and_collapse() {
        let mut d = Diagram::parse(&course_bytes()).unwrap();
        d.set_compartment_collapsed("doublyLinkedList.Person", "Fields", true);
        let s = String::from_utf8(d.to_bytes()).unwrap();
        assert!(s.contains("    <Position X=\"3.5\" Y=\"8.25\" Width=\"4.5\" />\r\n    <Compartments>\r\n      <Compartment Name=\"Fields\" Collapsed=\"true\" />\r\n    </Compartments>\r\n    <TypeIdentifier>"), "{s}");
        assert_eq!(d.find("doublyLinkedList.Person").unwrap().collapsed_compartments, ["Fields"]);
        d.set_compartment_collapsed("doublyLinkedList.Person", "Fields", false);
        assert_eq!(d.to_bytes(), course_bytes());
        d.set_collapsed("doublyLinkedList.Sex", true);
        assert!(d.find("doublyLinkedList.Sex").unwrap().collapsed);
        d.set_collapsed("doublyLinkedList.Sex", false);
        assert_eq!(d.to_bytes(), course_bytes());
    }

    #[test]
    fn inches_format() {
        assert_eq!(fmt_inches(3.5), "3.5");
        assert_eq!(fmt_inches(11.0), "11");
        assert_eq!(fmt_inches(8.25), "8.25");
        assert_eq!(fmt_inches(-0.0), "0");
        assert_eq!(fmt_inches(px_to_inches(144.0)), "1.5");
    }
}
