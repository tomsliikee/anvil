//! One model over `.slnx` and `.sln`: the projects of a solution, their
//! folders and files, and which one Visual Studio will start.

use std::path::{Path, PathBuf};

use crate::csproj::Csproj;
use crate::slnx::{ProjectRef, Slnx};
use crate::text::TextFile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Slnx,
    Sln,
}

#[derive(Clone, Debug)]
pub struct Project {
    pub name: String,
    /// Absolute path of the `.csproj`.
    pub path: PathBuf,
    pub dir: PathBuf,
    pub folder: Option<String>,
    /// `None` when the file is missing or broken; the explorer shows it
    /// unloaded, as VS does.
    pub csproj: Option<Csproj>,
}

impl Project {
    /// The namespace new items start from: `RootNamespace` or the project
    /// name with characters C# does not allow replaced, as VS does.
    pub fn root_namespace(&self) -> String {
        self.csproj
            .as_ref()
            .and_then(|c| c.property("RootNamespace"))
            .unwrap_or_else(|| crate::templates::safe_identifier_path(&self.name))
    }

    /// The `.cs` files that belong to the project: SDK-style globbing of
    /// `**/*.cs`, minus `bin/`, `obj/` and `<Compile Remove>`.
    pub fn source_files(&self) -> Vec<PathBuf> {
        let removes: Vec<String> = self.csproj.as_ref().map(Csproj::compile_removes).unwrap_or_default();
        let mut out = Vec::new();
        walk(&self.dir, &self.dir, &mut |rel, abs| {
            if rel.ends_with(".cs") && !removes.iter().any(|r| r.eq_ignore_ascii_case(rel)) {
                out.push(abs.to_owned());
            }
        });
        out.sort();
        out
    }
}

/// Walks a project folder the way SDK globbing sees it: `bin`, `obj` and
/// dot-folders are skipped at every level.
pub fn walk(root: &Path, dir: &Path, f: &mut dyn FnMut(&str, &Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name.starts_with('.') || name.eq_ignore_ascii_case("bin") || name.eq_ignore_ascii_case("obj") {
                continue;
            }
            walk(root, &path, f);
        } else if let Ok(rel) = path.strip_prefix(root) {
            f(&rel.to_string_lossy().replace('\\', "/"), &path);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Solution {
    pub path: PathBuf,
    pub dir: PathBuf,
    pub name: String,
    pub format: Format,
    pub projects: Vec<Project>,
    pub folders: Vec<String>,
}

#[derive(Debug)]
pub struct OpenError(pub String);

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OpenError {}

impl Solution {
    pub fn open(path: &Path) -> Result<Solution, OpenError> {
        let file = TextFile::read(path).map_err(|e| OpenError(format!("{}: {e}", path.display())))?;
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let (format, refs, folders) = match ext.as_str() {
            "slnx" => {
                let s = Slnx::parse(&file.text).map_err(|e| OpenError(format!("{}: {e}", path.display())))?;
                (Format::Slnx, s.projects(), s.folders())
            }
            "sln" => (Format::Sln, crate::sln::projects(&file.text), Vec::new()),
            _ => return Err(OpenError(format!("{} is not a solution", path.display()))),
        };
        let projects = refs.into_iter().map(|r| load_project(&dir, r)).collect();
        Ok(Solution {
            name: crate::slnx::stem(&path.to_string_lossy()),
            path: path.to_owned(),
            dir,
            format,
            projects,
            folders,
        })
    }

    /// Visual Studio starts the first project of the solution unless the
    /// user picked another in its own `.suo`, which Anvil cannot write.
    pub fn first_project(&self) -> Option<&Project> {
        self.projects.first()
    }

    pub fn project_named(&self, name: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.name == name)
    }

    /// The project that owns a file (the deepest project folder above it).
    pub fn project_of(&self, file: &Path) -> Option<&Project> {
        self.projects.iter().filter(|p| file.starts_with(&p.dir)).max_by_key(|p| p.dir.components().count())
    }

    /// Finds the one solution file in a folder (Open Folder).
    pub fn find_in(dir: &Path) -> Option<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("slnx" | "sln")))
            .collect();
        // Prefer .slnx when both exist (SDK 10 migration leaves both).
        found.sort_by_key(|p| (p.extension().and_then(|e| e.to_str()) != Some("slnx"), p.clone()));
        found.into_iter().next()
    }
}

fn load_project(dir: &Path, r: ProjectRef) -> Project {
    let path = dir.join(&r.path);
    let csproj = TextFile::read(&path).ok().and_then(|f| Csproj::parse(&f.text).ok());
    Project { name: r.name, dir: path.parent().map(Path::to_path_buf).unwrap_or_default(), path, folder: r.folder, csproj }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    #[test]
    fn opens_course_solution() {
        let s = Solution::open(&fixtures().join("doublyLinkedList/doublyLinkedList.slnx")).unwrap();
        assert_eq!(s.name, "doublyLinkedList");
        assert_eq!(s.format, Format::Slnx);
        let p = &s.projects[0];
        assert!(p.csproj.is_some());
        assert_eq!(p.root_namespace(), "doublyLinkedList");
        let files: Vec<String> = p.source_files().iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(files, ["DoublyLinkedPersonList.cs", "ListNode.cs", "Person.cs", "Program.cs", "Sex.cs"]);
    }

    #[test]
    fn first_project_is_vs_startup() {
        let s = Solution::open(&fixtures().join("sdk-samples/Sample2.slnx")).unwrap();
        assert_eq!(s.first_project().unwrap().name, "Lib");
        let s = Solution::open(&fixtures().join("sdk-samples/Sample.sln")).unwrap();
        assert_eq!(s.first_project().unwrap().name, "App");
        assert_eq!(s.project_named("Lib").unwrap().root_namespace(), "Lib");
    }

    #[test]
    fn project_of_file() {
        let s = Solution::open(&fixtures().join("sdk-samples/Sample.sln")).unwrap();
        let f = fixtures().join("sdk-samples/My Lib/Class1.cs");
        assert_eq!(s.project_of(&f).unwrap().name, "Lib");
        assert_eq!(Solution::find_in(&fixtures().join("sdk-samples")).unwrap().file_name().unwrap(), "Sample2.slnx");
    }
}
