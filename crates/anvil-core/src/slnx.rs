//! `.slnx`, the XML solution format Visual Studio 2026 and SDK 10 write.

use crate::xml::{Doc, Id, XmlError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRef {
    /// Display name: the file stem of the project path.
    pub name: String,
    /// Path relative to the solution, with `/` separators.
    pub path: String,
    /// Solution folder (`/My Lib/`) the project sits in, if any.
    pub folder: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Slnx {
    pub doc: Doc,
}

impl Slnx {
    pub fn parse(text: &str) -> Result<Slnx, XmlError> {
        let doc = Doc::parse(text)?;
        if doc.el(0).name != "Solution" {
            return Err(XmlError(format!("expected <Solution>, found <{}>", doc.el(0).name)));
        }
        Ok(Slnx { doc })
    }

    /// Projects in file order, which is the order VS uses to pick its
    /// default startup project.
    pub fn projects(&self) -> Vec<ProjectRef> {
        let mut out = Vec::new();
        self.collect(0, None, &mut out);
        out
    }

    fn collect(&self, id: Id, folder: Option<&str>, out: &mut Vec<ProjectRef>) {
        for c in self.doc.children(id) {
            match self.doc.el(c).name.as_str() {
                "Project" => {
                    if let Some(path) = self.doc.attr(c, "Path") {
                        let path = path.replace('\\', "/");
                        out.push(ProjectRef { name: stem(&path), path, folder: folder.map(str::to_owned) });
                    }
                }
                "Folder" => {
                    let name = self.doc.attr(c, "Name").map(str::to_owned);
                    self.collect(c, name.as_deref(), out);
                }
                _ => {}
            }
        }
    }

    pub fn folders(&self) -> Vec<String> {
        self.doc.all("Folder").filter_map(|f| self.doc.attr(f, "Name").map(str::to_owned)).collect()
    }
}

pub fn stem(path: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    file.rsplit_once('.').map_or(file, |(s, _)| s).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextFile;

    fn fixture(rel: &str) -> TextFile {
        TextFile::read(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)).unwrap()
    }

    #[test]
    fn course_slnx() {
        let f = fixture("doublyLinkedList/doublyLinkedList.slnx");
        let s = Slnx::parse(&f.text).unwrap();
        let p = s.projects();
        assert_eq!(p, vec![ProjectRef { name: "doublyLinkedList".into(), path: "doublyLinkedList/doublyLinkedList.csproj".into(), folder: None }]);
    }

    #[test]
    fn sdk_slnx_with_folder() {
        let s = Slnx::parse(&fixture("sdk-samples/Sample2.slnx").text).unwrap();
        let p = s.projects();
        assert_eq!(p[0].path, "My Lib/Lib.csproj");
        assert_eq!(p[0].folder.as_deref(), Some("/My Lib/"));
        assert_eq!(p[1].name, "App");
        assert_eq!(s.folders(), vec!["/My Lib/".to_owned()]);
    }
}
