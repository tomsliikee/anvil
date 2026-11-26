//! SDK-style `.csproj` files, read and changed without rewriting them.

use crate::edit::TextEdit;
use crate::xml::{Doc, Id, XmlError};

/// The `PackageReference` block of the course assignment.
pub const STYLECOP_BLOCK: &str = "<ItemGroup>\n  <PackageReference Include=\"StyleCop.Analyzers\" Version=\"1.1.118\">\n    <PrivateAssets>all</PrivateAssets>\n    <IncludeAssets>runtime; build; native; contentfiles; analyzers; buildtransitive</IncludeAssets>\n  </PackageReference>\n</ItemGroup>";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageRef {
    pub id: String,
    pub version: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Csproj {
    pub doc: Doc,
}

impl Csproj {
    pub fn parse(text: &str) -> Result<Csproj, XmlError> {
        let doc = Doc::parse(text)?;
        if doc.el(0).name != "Project" {
            return Err(XmlError("not an MSBuild project".into()));
        }
        Ok(Csproj { doc })
    }

    pub fn sdk(&self) -> Option<&str> {
        self.doc.attr(0, "Sdk")
    }

    fn property_groups(&self) -> impl Iterator<Item = Id> + '_ {
        self.doc.children_named(0, "PropertyGroup")
    }

    /// The last unconditional definition wins, as in MSBuild.
    pub fn property(&self, name: &str) -> Option<String> {
        self.property_groups()
            .filter(|&g| self.doc.attr(g, "Condition").is_none())
            .filter_map(|g| self.doc.child(g, name))
            .filter(|&p| self.doc.attr(p, "Condition").is_none())
            .last()
            .map(|p| self.doc.inner_text(p))
    }

    /// Sets a property: replaces its value where it is defined, otherwise
    /// adds it to the first unconditional `PropertyGroup` (or a new one).
    pub fn set_property(&self, name: &str, value: &str) -> TextEdit {
        let groups: Vec<Id> = self.property_groups().filter(|&g| self.doc.attr(g, "Condition").is_none()).collect();
        if let Some(p) = groups.iter().rev().find_map(|&g| self.doc.child(g, name)) {
            return self.doc.set_text(p, value);
        }
        let line = format!("<{name}>{}</{name}>", crate::xml::escape_text(value));
        match groups.first() {
            Some(&g) => {
                let last = self.doc.children(g).last();
                self.doc.insert_child(g, last, &line)
            }
            None => self.doc.insert_child(0, None, &format!("<PropertyGroup>\n  {line}\n</PropertyGroup>")),
        }
    }

    /// Removes a property everywhere it is defined unconditionally.
    pub fn remove_property(&self, name: &str) -> Vec<TextEdit> {
        self.property_groups()
            .filter(|&g| self.doc.attr(g, "Condition").is_none())
            .filter_map(|g| self.doc.child(g, name))
            .map(|p| self.doc.remove(p))
            .collect()
    }

    fn items<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = Id> + 'a {
        self.doc.children_named(0, "ItemGroup").flat_map(move |g| self.doc.children_named(g, kind).collect::<Vec<_>>())
    }

    pub fn package_refs(&self) -> Vec<PackageRef> {
        self.items("PackageReference")
            .filter_map(|i| {
                let id = self.doc.attr(i, "Include")?.to_owned();
                let version = self.doc.attr(i, "Version").map(str::to_owned).or_else(|| self.doc.child(i, "Version").map(|v| self.doc.inner_text(v)));
                Some(PackageRef { id, version })
            })
            .collect()
    }

    pub fn project_refs(&self) -> Vec<String> {
        self.items("ProjectReference").filter_map(|i| self.doc.attr(i, "Include").map(|p| p.replace('\\', "/"))).collect()
    }

    /// `<Compile Remove="…">` entries: files the user excluded.
    pub fn compile_removes(&self) -> Vec<String> {
        self.items("Compile").filter_map(|i| self.doc.attr(i, "Remove").map(|p| p.replace('\\', "/"))).collect()
    }

    /// Excludes a file the way VS does: `<Compile Remove="path" />` in an
    /// item group (VS writes Windows separators).
    pub fn exclude_compile(&self, rel_path: &str) -> TextEdit {
        let item = format!("<Compile Remove=\"{}\" />", crate::xml::escape_attr(&rel_path.replace('/', "\\")));
        match self.doc.children_named(0, "ItemGroup").find(|&g| self.doc.children(g).all(|c| self.doc.el(c).name == "Compile")) {
            Some(g) => {
                let last = self.doc.children(g).last();
                self.doc.insert_child(g, last, &item)
            }
            None => {
                // VS puts a new ItemGroup after the last PropertyGroup.
                let after = self.doc.children(0).last();
                self.insert_group_after(after, &item)
            }
        }
    }

    fn insert_group_after(&self, after: Option<Id>, item: &str) -> TextEdit {
        let fragment = format!("\n<ItemGroup>\n  {item}\n</ItemGroup>");
        self.doc.insert_child(0, after, &fragment)
    }

    /// Adds the course's StyleCop.Analyzers reference after the last
    /// `PropertyGroup`, with an empty line before it, exactly as the
    /// course's project has it.
    pub fn add_stylecop(&self) -> TextEdit {
        let after = self.doc.children_named(0, "PropertyGroup").last();
        self.doc.insert_child(0, after, &format!("\n{STYLECOP_BLOCK}"))
    }

    /// `<AdditionalFiles Include>` paths (StyleCop reads `stylecop.json`
    /// only when it is one).
    pub fn additional_files(&self) -> Vec<String> {
        self.doc.all("AdditionalFiles").filter_map(|i| self.doc.attr(i, "Include").map(str::to_owned)).collect()
    }

    /// Whether `rel_path` (`/` or `\\` separated) is an additional file.
    pub fn has_additional_file(&self, rel_path: &str) -> bool {
        let want = rel_path.replace('\\', "/");
        self.additional_files().iter().any(|f| f.replace('\\', "/").eq_ignore_ascii_case(&want))
    }

    /// Makes `rel_path` an analyzer additional file as Visual Studio does
    /// when StyleCop's "Add StyleCop settings file" fix adds
    /// `stylecop.json` to an SDK project: the file leaves the `None` glob
    /// and becomes `AdditionalFiles`, each in an ItemGroup of its own at
    /// the end, an empty line before each.
    pub fn add_additional_file(&self, rel_path: &str) -> TextEdit {
        let p = rel_path.replace('/', "\\");
        let after = self.doc.children(0).last();
        self.doc.insert_child(0, after, &format!("\n<ItemGroup>\n  <None Remove=\"{p}\" />\n</ItemGroup>\n\n<ItemGroup>\n  <AdditionalFiles Include=\"{p}\" />\n</ItemGroup>"))
    }

    pub fn target_framework(&self) -> Option<String> {
        self.property("TargetFramework").or_else(|| self.property("TargetFrameworks").and_then(|t| t.split(';').next().map(str::to_owned)))
    }

    pub fn is_test_project(&self) -> bool {
        self.property("IsTestProject").is_some_and(|v| v.eq_ignore_ascii_case("true"))
            || self.package_refs().iter().any(|p| matches!(p.id.as_str(), "Microsoft.NET.Test.Sdk" | "MSTest" | "MSTest.Sdk" | "xunit" | "xunit.v3" | "NUnit"))
            || self.sdk().is_some_and(|s| s.starts_with("MSTest.Sdk"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::apply;
    use crate::text::TextFile;

    fn course() -> TextFile {
        TextFile::read(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doublyLinkedList/doublyLinkedList/doublyLinkedList.csproj")).unwrap()
    }

    #[test]
    fn reads_course_properties_and_stylecop() {
        let c = Csproj::parse(&course().text).unwrap();
        assert_eq!(c.sdk(), Some("Microsoft.NET.Sdk"));
        assert_eq!(c.property("OutputType").as_deref(), Some("Exe"));
        assert_eq!(c.target_framework().as_deref(), Some("net10.0"));
        assert_eq!(c.property("Nullable").as_deref(), Some("enable"));
        assert_eq!(c.package_refs(), vec![PackageRef { id: "StyleCop.Analyzers".into(), version: Some("1.1.118".into()) }]);
        assert!(!c.is_test_project());
    }

    #[test]
    fn csproj_set_nullable_changes_one_element() {
        let mut f = course();
        let c = Csproj::parse(&f.text).unwrap();
        let before = f.encode();
        f.text = apply(&f.text, &[c.set_property("Nullable", "disable")]);
        let after = f.encode();
        let expected = String::from_utf8(before).unwrap().replace("<Nullable>enable</Nullable>", "<Nullable>disable</Nullable>");
        assert_eq!(String::from_utf8(after).unwrap(), expected);
    }

    #[test]
    fn new_property_goes_into_first_group_with_crlf() {
        let mut f = course();
        let c = Csproj::parse(&f.text).unwrap();
        f.text = apply(&f.text, &[c.set_property("RootNamespace", "Course")]);
        let s = String::from_utf8(f.encode()).unwrap();
        assert!(s.contains("    <Nullable>enable</Nullable>\r\n    <RootNamespace>Course</RootNamespace>\r\n  </PropertyGroup>"));
    }

    #[test]
    fn exclude_adds_item_group() {
        let mut f = course();
        let c = Csproj::parse(&f.text).unwrap();
        f.text = apply(&f.text, &[c.exclude_compile("Sub/Old.cs")]);
        let c2 = Csproj::parse(&f.text).unwrap();
        assert_eq!(c2.compile_removes(), vec!["Sub/Old.cs".to_owned()]);
        assert!(f.text.contains("    <Compile Remove=\"Sub\\Old.cs\" />"));
    }

    /// `dotnet new console` plus Anvil's StyleCop block, made CRLF, is the
    /// course's project byte for byte.
    #[test]
    fn console_with_stylecop_matches_course() {
        let sdk = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sdk-samples/App/App.csproj")).unwrap();
        let mut f = TextFile::decode(&sdk);
        let c = Csproj::parse(&f.text).unwrap();
        f.text = apply(&f.text, &[c.add_stylecop()]);
        f.eol = crate::text::Eol::Crlf;
        let course = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doublyLinkedList/doublyLinkedList/doublyLinkedList.csproj")).unwrap();
        assert_eq!(String::from_utf8(f.encode()).unwrap(), String::from_utf8(course).unwrap());
    }

    #[test]
    fn stylecop_settings_become_an_additional_file() {
        let mut f = course();
        let c = Csproj::parse(&f.text).unwrap();
        assert!(!c.has_additional_file("stylecop.json"));
        f.text = apply(&f.text, &[c.add_additional_file("stylecop.json")]);
        assert!(f.text.ends_with("    </PackageReference>\n  </ItemGroup>\n\n  <ItemGroup>\n    <None Remove=\"stylecop.json\" />\n  </ItemGroup>\n\n  <ItemGroup>\n    <AdditionalFiles Include=\"stylecop.json\" />\n  </ItemGroup>\n\n</Project>\n"), "{}", f.text);
        let c = Csproj::parse(&f.text).unwrap();
        assert!(c.has_additional_file("stylecop.json") && c.has_additional_file("STYLECOP.json"));
        assert_eq!(c.additional_files(), ["stylecop.json"]);
    }

    #[test]
    fn csproj_round_trip_identical() {
        let f = course();
        let bytes = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doublyLinkedList/doublyLinkedList/doublyLinkedList.csproj")).unwrap();
        Csproj::parse(&f.text).unwrap();
        assert_eq!(f.encode(), bytes);
    }
}
