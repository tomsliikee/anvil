//! Visual Studio's C# item templates, rendered as VS 2026 writes them:
//! UTF-8 with BOM, CRLF, block namespace from the folder path, not
//! StyleCop-cleaned.
//!
//! Evidence: the class template is the byte-exact shape of
//! `DoublyLinkedPersonList.cs` in the course fixture, which VS 2026 created
//! with Add → Class in a project with `ImplicitUsings` (no `using` lines,
//! an empty line in the body where VS puts the caret, no final line
//! break). Interface, struct, enum and record follow that shape with their
//! keyword (*not verified against VS*). The test classes are the SDK
//! template engine's `mstest-class` / `nunit-test`, which VS also uses.

use std::path::Path;

use crate::text::{Encoding, Eol, TextFile};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemTemplate {
    Class,
    Interface,
    Enum,
    Struct,
    Record,
    ClassDiagram,
    MsTestClass,
    NUnitTest,
    Json,
    Text,
    EditorConfig,
}

impl ItemTemplate {
    pub const ALL: [ItemTemplate; 11] = [
        ItemTemplate::Class,
        ItemTemplate::Interface,
        ItemTemplate::Enum,
        ItemTemplate::Struct,
        ItemTemplate::Record,
        ItemTemplate::ClassDiagram,
        ItemTemplate::MsTestClass,
        ItemTemplate::NUnitTest,
        ItemTemplate::Json,
        ItemTemplate::Text,
        ItemTemplate::EditorConfig,
    ];

    /// VS' name in the Add New Item dialog (English; translated in the UI).
    pub fn title(self) -> &'static str {
        match self {
            ItemTemplate::Class => "Class",
            ItemTemplate::Interface => "Interface",
            ItemTemplate::Enum => "Enum",
            ItemTemplate::Struct => "Struct",
            ItemTemplate::Record => "Record",
            ItemTemplate::ClassDiagram => "Class Diagram",
            ItemTemplate::MsTestClass => "MSTest Test Class",
            ItemTemplate::NUnitTest => "NUnit Test Class",
            ItemTemplate::Json => "JSON File",
            ItemTemplate::Text => "Text File",
            ItemTemplate::EditorConfig => "editorconfig File (.NET)",
        }
    }

    /// The default name stem and extension VS proposes (`Class1.cs`).
    pub fn default_name(self) -> (&'static str, &'static str) {
        match self {
            ItemTemplate::Class => ("Class", ".cs"),
            ItemTemplate::Interface => ("Interface", ".cs"),
            ItemTemplate::Enum => ("Enum", ".cs"),
            ItemTemplate::Struct => ("Struct", ".cs"),
            ItemTemplate::Record => ("Record", ".cs"),
            ItemTemplate::ClassDiagram => ("ClassDiagram", ".cd"),
            ItemTemplate::MsTestClass => ("UnitTest", ".cs"),
            ItemTemplate::NUnitTest => ("UnitTest", ".cs"),
            ItemTemplate::Json => ("JSON", ".json"),
            ItemTemplate::Text => ("TextFile", ".txt"),
            ItemTemplate::EditorConfig => (".editorconfig", ""),
        }
    }
}

pub struct Ctx<'a> {
    /// Namespace of the folder the item goes into (see [`namespace_for`]).
    pub namespace: &'a str,
    /// The file name without extension; made a safe identifier here.
    pub name: &'a str,
}

/// The new file's bytes.
pub fn render(t: ItemTemplate, ctx: &Ctx) -> Vec<u8> {
    let name = safe_identifier(ctx.name);
    let ns = ctx.namespace;
    let keyword_type = |kw: &str| format!("namespace {ns}\n{{\n    internal {kw} {name}\n    {{\n\n    }}\n}}");
    let text = match t {
        ItemTemplate::Class => keyword_type("class"),
        ItemTemplate::Interface => keyword_type("interface"),
        ItemTemplate::Struct => keyword_type("struct"),
        ItemTemplate::Record => keyword_type("record"),
        ItemTemplate::Enum => keyword_type("enum"),
        ItemTemplate::ClassDiagram => crate::cd::EMPTY_DIAGRAM.to_owned(),
        ItemTemplate::MsTestClass => format!(
            "using Microsoft.VisualStudio.TestTools.UnitTesting;\n\nnamespace {ns};\n\n[TestClass]\npublic class {name}\n{{\n    [TestMethod]\n    public void TestMethod1()\n    {{\n    }}\n}}\n"
        ),
        ItemTemplate::NUnitTest => format!(
            "namespace {ns};\n\npublic class {name}\n{{\n    [SetUp]\n    public void Setup()\n    {{\n    }}\n\n    [Test]\n    public void Test1()\n    {{\n        Assert.Pass();\n    }}\n}}\n"
        ),
        ItemTemplate::Json => "{\n}\n".to_owned(),
        ItemTemplate::Text => String::new(),
        ItemTemplate::EditorConfig => include_str!("../templates/editorconfig.txt").to_owned(),
    };
    TextFile { encoding: Encoding::Utf8 { bom: true }, eol: Eol::Crlf, text }.encode()
}

/// `Program.cs` of VS 2026's Console App with "Do not use top-level
/// statements": block namespace, `internal class Program`, as the course's
/// VS-written `Program.cs` begins. (`dotnet new console --use-program-main`
/// writes a file-scoped namespace and no access modifier instead.)
pub fn console_program(namespace: &str) -> Vec<u8> {
    let text = format!("namespace {namespace}\n{{\n    internal class Program\n    {{\n        static void Main(string[] args)\n        {{\n            Console.WriteLine(\"Hello, World!\");\n        }}\n    }}\n}}\n");
    TextFile { encoding: Encoding::Utf8 { bom: true }, eol: Eol::Crlf, text }.encode()
}

/// VS' `$safeitemrootname$`: characters that cannot be in an identifier
/// become `_`, a leading digit gets a `_` in front.
pub fn safe_identifier(name: &str) -> String {
    let mut out: String = name.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// A dotted namespace with each part made safe (`My App.Core` →
/// `My_App.Core`).
pub fn safe_identifier_path(name: &str) -> String {
    name.split('.').filter(|p| !p.is_empty()).map(safe_identifier).collect::<Vec<_>>().join(".")
}

/// The namespace VS gives an item in `rel_folder` (relative to the project
/// folder, `/`-separated): the root namespace plus each folder name.
pub fn namespace_for(root_namespace: &str, rel_folder: &str) -> String {
    let mut ns = root_namespace.to_owned();
    for part in rel_folder.split(['/', '\\']).filter(|p| !p.is_empty()) {
        ns.push('.');
        ns.push_str(&safe_identifier_path(part));
    }
    ns
}

/// The first `<stem><n><ext>` (n from 1) that does not exist in `dir`,
/// compared case-insensitively as on Windows.
pub fn next_free_name(dir: &Path, stem: &str, ext: &str) -> String {
    let existing: Vec<String> = std::fs::read_dir(dir)
        .map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().to_lowercase()).collect())
        .unwrap_or_default();
    (1..).map(|n| format!("{stem}{n}{ext}")).find(|n| !existing.contains(&n.to_lowercase())).expect("an unused name exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doublyLinkedList/doublyLinkedList")
    }

    #[test]
    fn class_matches_vs2026() {
        let vs = std::fs::read(fixtures().join("DoublyLinkedPersonList.cs")).unwrap();
        let ours = render(ItemTemplate::Class, &Ctx { namespace: "doublyLinkedList", name: "DoublyLinkedPersonList" });
        assert_eq!(String::from_utf8_lossy(&ours), String::from_utf8_lossy(&vs));
        assert_eq!(ours, vs);
    }

    #[test]
    fn enum_and_interface_follow_the_class_shape() {
        let e = render(ItemTemplate::Enum, &Ctx { namespace: "N", name: "Sex" });
        assert_eq!(e, b"\xEF\xBB\xBFnamespace N\r\n{\r\n    internal enum Sex\r\n    {\r\n\r\n    }\r\n}");
    }

    #[test]
    fn console_program_matches_vs_shape() {
        let vs = std::fs::read(fixtures().join("Program.cs")).unwrap();
        let ours = console_program("doublyLinkedList");
        let head = b"\xEF\xBB\xBFnamespace doublyLinkedList\r\n{\r\n    internal class Program\r\n    {\r\n        static void Main(string[] args)\r\n        {\r\n";
        assert!(vs.starts_with(head));
        assert!(ours.starts_with(head));
        assert!(ours.ends_with(b"        }\r\n    }\r\n}\r\n") && vs.ends_with(b"        }\r\n    }\r\n}\r\n"));
    }

    #[test]
    fn namespace_from_folder_path() {
        assert_eq!(namespace_for("doublyLinkedList", ""), "doublyLinkedList");
        assert_eq!(namespace_for("App", "Models/My Folder"), "App.Models.My_Folder");
        assert_eq!(namespace_for("App", "2D"), "App._2D");
        assert_eq!(safe_identifier("Übung-1"), "Übung_1");
    }

    #[test]
    fn next_names() {
        assert_eq!(next_free_name(&fixtures(), "Class", ".cs"), "Class1.cs");
        assert_eq!(next_free_name(&fixtures(), "ClassDiagram", ".cd"), "ClassDiagram2.cd");
    }

    #[test]
    fn test_classes_are_bom_crlf() {
        let t = render(ItemTemplate::MsTestClass, &Ctx { namespace: "Tests", name: "UnitTest1" });
        assert!(t.starts_with(b"\xEF\xBB\xBFusing Microsoft.VisualStudio.TestTools.UnitTesting;\r\n\r\nnamespace Tests;\r\n"));
    }
}
