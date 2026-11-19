//! *File → Hand in as ZIP…*: the solution folder without `bin/`, `obj/`,
//! `.vs/` and without what `.gitignore` excludes. Visual Studio restores
//! the left-out folders when the lecturer opens the solution.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::ignore::Ignore;

const ALWAYS_SKIPPED: &[&str] = &["bin", "obj", ".vs", ".git", ".idea", ".vscode"];

/// Files of the hand-in, relative to `solution_dir`, sorted.
pub fn handin_files(solution_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // Each folder's own .gitignore applies below it, like git.
    walk(solution_dir, Path::new(""), &mut Vec::new(), &mut out);
    out.sort();
    out
}

fn walk(root: &Path, rel: &Path, ignores: &mut Vec<(PathBuf, Ignore)>, out: &mut Vec<PathBuf>) {
    let dir = root.join(rel);
    let local = std::fs::read_to_string(dir.join(".gitignore")).ok().map(|t| (rel.to_path_buf(), Ignore::from_gitignore(&t)));
    let pushed = local.is_some();
    if let Some(l) = local {
        ignores.push(l);
    }
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let child = rel.join(&name);
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let is_dir = ft.is_dir();
        if is_dir && ALWAYS_SKIPPED.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
            continue;
        }
        let ignored = ignores.iter().any(|(base, ig)| {
            child.strip_prefix(base).is_ok_and(|r| ig.is_ignored(&r.to_string_lossy().replace('\\', "/"), is_dir))
        });
        if ignored {
            continue;
        }
        if is_dir {
            walk(root, &child, ignores, out);
        } else if !name.ends_with(".anvil-tmp") {
            out.push(child);
        }
    }
    if pushed {
        ignores.pop();
    }
}

/// Writes the ZIP with every entry under `<folder name>/`, deflated, with
/// `/` separators and UTF-8 names. Returns the number of files.
pub fn write_zip(solution_dir: &Path, out: &Path) -> std::io::Result<usize> {
    let folder = solution_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Solution".into());
    let files = handin_files(solution_dir);
    let tmp = out.with_extension("zip.anvil-tmp");
    {
        let file = std::fs::File::create(&tmp)?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).unix_permissions(0o644);
        for rel in &files {
            let name = format!("{folder}/{}", rel.to_string_lossy().replace('\\', "/"));
            zip.start_file(name, options).map_err(std::io::Error::other)?;
            zip.write_all(&std::fs::read(solution_dir.join(rel))?)?;
        }
        zip.finish().map_err(std::io::Error::other)?;
    }
    std::fs::rename(&tmp, out)?;
    Ok(files.len())
}

/// Extracts the entries under `prefix` of a ZIP (a NuGet package) into
/// `dest`, without the prefix. Paths that would leave `dest` are skipped.
pub fn extract_prefix(zip_bytes: &[u8], prefix: &str, dest: &Path) -> std::io::Result<usize> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(std::io::Error::other)?;
    let mut n = 0;
    for i in 0..z.len() {
        let mut entry = z.by_index(i).map_err(std::io::Error::other)?;
        let name = entry.name().map_err(std::io::Error::other)?.into_owned();
        let Some(rel) = name.strip_prefix(prefix) else { continue };
        if rel.is_empty() || rel.ends_with('/') || rel.split('/').any(|p| p == ".." || p.is_empty()) {
            continue;
        }
        let out = dest.join(rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
        n += 1;
    }
    Ok(n)
}

/// Where the ZIP goes by default: next to the solution folder, named after
/// the solution (`doublyLinkedList.zip`).
pub fn default_zip_path(solution_dir: &Path, solution_name: &str) -> PathBuf {
    solution_dir.parent().unwrap_or(solution_dir).join(format!("{solution_name}.zip"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("anvil-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() { copy_dir(&p, &to.join(e.file_name())) } else { std::fs::copy(&p, to.join(e.file_name())).unwrap(); }
        }
    }

    #[test]
    fn skips_bin_obj_vs_and_ignored() {
        let d = temp_dir("handin");
        let sol = d.join("Course");
        copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doublyLinkedList"), &sol);
        for junk in ["doublyLinkedList/bin/Debug/x.dll", "doublyLinkedList/obj/project.assets.json", ".vs/doublyLinkedList/v18/.suo", "notes.tmp"] {
            let p = sol.join(junk);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        std::fs::write(sol.join(".gitignore"), "*.tmp\n").unwrap();
        let files: Vec<String> = handin_files(&sol).iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert_eq!(
            files,
            [".gitignore", "doublyLinkedList/ClassDiagram1.cd", "doublyLinkedList/DoublyLinkedPersonList.cs", "doublyLinkedList/ListNode.cs", "doublyLinkedList/Person.cs", "doublyLinkedList/Program.cs", "doublyLinkedList/Sex.cs", "doublyLinkedList/doublyLinkedList.csproj", "doublyLinkedList.slnx"]
        );
        let zip_path = d.join("Course.zip");
        assert_eq!(write_zip(&sol, &zip_path).unwrap(), 9);
        let mut z = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let mut names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().unwrap().into_owned()).collect();
        names.sort();
        assert!(names.contains(&"Course/doublyLinkedList/Person.cs".to_owned()));
        let mut person = z.by_name("Course/doublyLinkedList/Person.cs").unwrap();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut person, &mut bytes).unwrap();
        assert_eq!(bytes, std::fs::read(sol.join("doublyLinkedList/Person.cs")).unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn zip_keeps_unicode_names() {
        let d = temp_dir("unicode");
        let sol = d.join("Übung 1");
        std::fs::create_dir_all(sol.join("Prüfung")).unwrap();
        std::fs::write(sol.join("Prüfung/Größe.cs"), "x").unwrap();
        let zip_path = d.join("Übung.zip");
        write_zip(&sol, &zip_path).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        assert_eq!(z.by_index(0).unwrap().name().unwrap(), "Übung 1/Prüfung/Größe.cs");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn extracts_a_prefix_and_refuses_escapes() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            for (name, body) in [("content/LS/a.dll", "A"), ("content/LS/sub/b.json", "B"), ("lib/x", "X"), ("content/LS/../evil", "E")] {
                z.start_file(name, o).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        let d = temp_dir("extract");
        assert_eq!(extract_prefix(buf.get_ref(), "content/LS/", &d).unwrap(), 2);
        assert_eq!(std::fs::read_to_string(d.join("sub/b.json")).unwrap(), "B");
        assert!(!d.join("x").exists() && !d.parent().unwrap().join("evil").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn default_path_is_next_to_the_folder() {
        assert_eq!(default_zip_path(Path::new("/home/t/Uni/Course"), "doublyLinkedList"), Path::new("/home/t/Uni/doublyLinkedList.zip"));
    }
}
