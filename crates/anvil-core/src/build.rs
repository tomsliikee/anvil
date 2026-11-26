//! The `dotnet build` command line Anvil runs, and what it reads back while
//! the build streams: whether a restore is needed at all, the project
//! graph a project build covers, and which project just finished.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::csproj::Csproj;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Build,
    Rebuild,
    Clean,
}

/// The arguments after `dotnet`. `restore: false` passes `--no-restore`
/// (the assets are current, see [`assets_current`]); restoring a
/// solution whose packages did not change costs a second or more per build.
/// Full paths make every `file(line,col)` in the output a link; the
/// terminal logger is off because the output is not a terminal, and an
/// explicit `-tl:off` keeps a forced `MSBUILDTERMINALLOGGER` from writing
/// escape codes into the Output window. Minimal verbosity (the default) is
/// kept on purpose: `-v:q` saves little and hides the `Project -> dll`
/// lines that show the build moving.
pub fn dotnet_args(verb: Verb, target: &Path, restore: bool) -> Vec<String> {
    let mut a = vec![if verb == Verb::Clean { "clean" } else { "build" }.to_owned(), target.to_string_lossy().into_owned()];
    a.extend(["-nologo", "-consoleLoggerParameters:NoSummary", "-tl:off", "-p:GenerateFullPaths=true"].map(str::to_owned));
    if verb == Verb::Rebuild {
        a.push("--no-incremental".into());
    }
    if !restore && verb != Verb::Clean {
        a.push("--no-restore".into());
    }
    a
}

/// A build that failed only because `--no-restore` met missing or stale
/// assets (NETSDK1004 "assets file not found", NETSDK1005/1047 "no target
/// for", NETSDK1064 "package not found" (assets written on another
/// machine), NU1105 a project that cannot be read): run it again with
/// restore.
pub fn needs_restore_retry(log: &str) -> bool {
    ["NETSDK1004", "NETSDK1005", "NETSDK1047", "NETSDK1064", "NU1105"].iter().any(|c| log.contains(c))
}

/// The project an assets file was restored for (`restore.projectPath`),
/// read without parsing the whole file (it runs to megabytes).
fn assets_project_path(assets: &str) -> Option<String> {
    let restore = assets.find("\"restore\": {")?;
    let key = "\"projectPath\": ";
    let start = restore + assets[restore..].find(key)? + key.len();
    let rest = &assets[start..];
    let mut end = None;
    let mut escaped = false;
    for (i, c) in rest.char_indices().skip(1) {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => {
                end = Some(i);
                break;
            }
            _ => {}
        }
    }
    serde_json::from_str(&rest[..=end?]).ok()
}

/// `start` and every project it references, directly or not; a project
/// build covers exactly these. Unreadable project files are kept (MSBuild
/// will say what is wrong with them) but not followed.
pub fn project_graph(start: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut todo = vec![normalize(start)];
    while let Some(p) = todo.pop() {
        if out.contains(&p) {
            continue;
        }
        if let Some(text) = std::fs::read_to_string(&p).ok()
            && let Ok(c) = Csproj::parse(&text)
        {
            let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
            todo.extend(c.project_refs().iter().map(|r| normalize(&dir.join(r))));
        }
        out.push(p);
    }
    out
}

/// `a/b/../c` → `a/c` without touching the disk, so the same project
/// reached by two relative paths counts once.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Files above or beside a project that change what restore produces.
const RESTORE_INPUTS: &[&str] = &["Directory.Build.props", "Directory.Build.targets", "Directory.Packages.props", "NuGet.Config", "NuGet.config", "nuget.config", "global.json"];

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Whether restore would change nothing for `project`: its
/// `obj/project.assets.json` exists and is not older than the project file
/// or any `Directory.*.props`, NuGet.Config or global.json above it. When
/// restore ran but found nothing to do, NuGet leaves the assets file alone
/// and only touches `obj/project.nuget.cache`, so the newer of the two
/// counts. A project that moves its `obj` folder elsewhere is never
/// current, which only costs the restore it had before. Neither is an
/// assets file restored for another path: an `obj` folder copied from
/// Windows (or checked in) names `C:\…` package folders and fails with
/// NETSDK1064 under `--no-restore`.
pub fn assets_current(project: &Path) -> bool {
    let Some(dir) = project.parent() else { return false };
    let obj = dir.join("obj");
    let Some(assets) = mtime(&obj.join("project.assets.json")) else { return false };
    let restored_for = std::fs::read_to_string(obj.join("project.assets.json")).ok().and_then(|t| assets_project_path(&t));
    let same = |p: &str| Path::new(p) == project || std::fs::canonicalize(p).ok().zip(std::fs::canonicalize(project).ok()).is_some_and(|(a, b)| a == b);
    if !restored_for.as_deref().is_some_and(same) {
        return false;
    }
    let restored = mtime(&obj.join("project.nuget.cache")).map_or(assets, |c| c.max(assets));
    let Some(proj) = mtime(project) else { return false };
    if proj > restored {
        return false;
    }
    let mut d = Some(dir);
    while let Some(cur) = d {
        for name in RESTORE_INPUTS {
            if mtime(&cur.join(name)).is_some_and(|t| t > restored) {
                return false;
            }
        }
        d = cur.parent();
    }
    true
}

/// After a build that restored and succeeded: marks each project's
/// assets as current. NuGet leaves its files untouched when a restore
/// changes nothing (an edited property in the project file), so without
/// this every later build would restore again.
pub fn mark_restored(projects: &[PathBuf]) {
    let now = SystemTime::now();
    for p in projects {
        let cache = p.parent().map(|d| d.join("obj/project.nuget.cache"));
        if let Some(f) = cache.filter(|c| c.is_file()).and_then(|c| std::fs::File::options().write(true).open(c).ok()) {
            let _ = f.set_modified(now);
        }
    }
}

/// What the last builds reported, kept in `.vs/anvil/build.json`.
///
/// An incremental build whose inputs did not change skips the compiler,
/// and with it every analyzer: the second Build in a row reports no
/// StyleCop warning at all, although none was fixed. A project whose
/// output file still has the time stamp it had after the build that
/// reported its diagnostics was not compiled again, so those diagnostics
/// still hold and are shown again.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BuildMemory {
    /// Per project name (`App`): its output file and that file's time
    /// stamp (nanoseconds since the epoch) after the build.
    pub outputs: std::collections::BTreeMap<String, (PathBuf, u128)>,
    /// Per project name: what its last compile reported.
    pub diagnostics: std::collections::BTreeMap<String, Vec<crate::diagnostics::Diagnostic>>,
}

fn stamp(p: &Path) -> Option<u128> {
    mtime(p)?.duration_since(SystemTime::UNIX_EPOCH).ok().map(|d| d.as_nanos())
}

/// The project a diagnostic belongs to, by name (`[/w/App/App.csproj]`
/// or a multi-targeting `[…/App.csproj::TargetFramework=net8.0]`).
pub fn project_name(d: &crate::diagnostics::Diagnostic) -> Option<String> {
    let p = d.project.as_deref()?;
    let p = p.split_once("::").map_or(p, |(a, _)| a);
    Some(crate::slnx::stem(p)).filter(|s| !s.is_empty())
}

impl BuildMemory {
    fn path(solution_dir: &Path) -> PathBuf {
        crate::session::dir(solution_dir).join("build.json")
    }

    pub fn load(solution_dir: &Path) -> BuildMemory {
        std::fs::read_to_string(Self::path(solution_dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, solution_dir: &Path) -> std::io::Result<()> {
        let p = Self::path(solution_dir);
        std::fs::create_dir_all(p.parent().expect("has a parent"))?;
        crate::text::write_atomic(&p, serde_json::to_string(self).expect("serialises").as_bytes())
    }

    /// After a build: `fresh` is what it reported, `finished` each project
    /// it finished with its output file. Returns the diagnostics to show:
    /// `fresh`, plus the remembered ones of every finished project whose
    /// output was not written again. The memory then holds this build.
    pub fn merge(&mut self, fresh: &[crate::diagnostics::Diagnostic], finished: &[(String, PathBuf)]) -> Vec<crate::diagnostics::Diagnostic> {
        let mut out = fresh.to_vec();
        let mut by_project: std::collections::BTreeMap<String, Vec<crate::diagnostics::Diagnostic>> = Default::default();
        for d in fresh {
            if let Some(n) = project_name(d) {
                by_project.entry(n).or_default().push(d.clone());
            }
        }
        for (name, output) in finished {
            let now = stamp(output);
            let unchanged = now.is_some() && self.outputs.get(name).is_some_and(|(p, s)| p == output && Some(*s) == now);
            if unchanged
                && !by_project.contains_key(name)
                && let Some(old) = self.diagnostics.get(name)
            {
                out.extend(old.iter().filter(|d| !fresh.contains(d)).cloned());
                by_project.insert(name.clone(), old.clone());
            }
            match now {
                Some(s) => {
                    self.outputs.insert(name.clone(), (output.clone(), s));
                }
                None => {
                    self.outputs.remove(name);
                }
            }
            self.diagnostics.remove(name);
        }
        // Projects that failed (no output line) keep only what they reported now.
        for (name, list) in by_project {
            self.diagnostics.insert(name, list);
        }
        out
    }

    /// After Clean: nothing is remembered.
    pub fn clear(&mut self) {
        *self = BuildMemory::default();
    }
}

/// The project MSBuild just finished at minimal verbosity and what it
/// wrote (`  Core -> /work/Core/bin/Debug/net10.0/Core.dll`). Ctrl+F5 and
/// F5 run that file, which spares a second MSBuild evaluation for
/// `-getProperty:TargetPath` after every build.
pub fn finished_project(line: &str) -> Option<(&str, &str)> {
    let (name, out) = line.trim().split_once(" -> ")?;
    (!name.is_empty() && !name.contains(char::is_whitespace) && (out.starts_with('/') || out.get(1..3) == Some(":\\"))).then_some((name, out))
}

/// Elapsed time as the status line and the result toast show it:
/// `0.4 s`, `12.3 s`, `1:05`.
pub fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 60.0 { format!("{s:.1} s") } else { format!("{}:{:02}", d.as_secs() / 60, d.as_secs() % 60) }
}

/// The running clock in the status line, in whole seconds (`3 s`, `1:05`),
/// so it changes once a second and not on every frame.
pub fn format_running(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 { format!("{s} s") } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("anvil-build-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn csproj(refs: &[&str]) -> String {
        let items: String = refs.iter().map(|r| format!("<ProjectReference Include=\"{r}\" />")).collect();
        format!("<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>{items}</ItemGroup></Project>")
    }

    #[test]
    fn args_for_each_verb() {
        let t = Path::new("/w/App/App.csproj");
        let b = dotnet_args(Verb::Build, t, false);
        assert_eq!(b[..2], ["build", "/w/App/App.csproj"]);
        assert!(b.contains(&"--no-restore".into()) && b.contains(&"-p:GenerateFullPaths=true".into()) && b.contains(&"-tl:off".into()));
        assert!(!b.iter().any(|a| a.starts_with("-v")), "minimal verbosity keeps the progress lines");
        assert!(!dotnet_args(Verb::Build, t, true).contains(&"--no-restore".into()));
        assert!(dotnet_args(Verb::Rebuild, t, false).contains(&"--no-incremental".into()));
        let c = dotnet_args(Verb::Clean, t, false);
        assert_eq!(c[0], "clean");
        assert!(!c.contains(&"--no-restore".into()), "clean has no such option");
    }

    #[test]
    fn graph_follows_references_once() {
        let d = temp("graph");
        for (p, refs) in [("App", vec!["..\\Lib\\Lib.csproj", "../Util/Util.csproj"]), ("Lib", vec!["../Util/Util.csproj"]), ("Util", vec![]), ("Other", vec![])] {
            std::fs::create_dir_all(d.join(p)).unwrap();
            std::fs::write(d.join(p).join(format!("{p}.csproj")), csproj(&refs)).unwrap();
        }
        let mut g = project_graph(&d.join("App/App.csproj"));
        g.sort();
        assert_eq!(g, vec![d.join("App/App.csproj"), d.join("Lib/Lib.csproj"), d.join("Util/Util.csproj")]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The part of `project.assets.json` that names the project, as NuGet
    /// writes it (`path` already JSON-escaped).
    fn assets_for(path: &str) -> String {
        format!("{{\n  \"version\": 3,\n  \"project\": {{\n    \"version\": \"1.0.0\",\n    \"restore\": {{\n      \"projectUniqueName\": \"{path}\",\n      \"projectName\": \"App\",\n      \"projectPath\": \"{path}\",\n      \"packagesPath\": \"/x\"\n    }}\n  }}\n}}")
    }

    fn warn(project: &str, code: &str) -> crate::diagnostics::Diagnostic {
        crate::diagnostics::Diagnostic { file: format!("/w/{project}/Program.cs"), line: 1, col: 1, severity: crate::diagnostics::Severity::Warning, code: code.into(), message: "m".into(), project: Some(format!("/w/{project}/{project}.csproj")) }
    }

    #[test]
    fn an_up_to_date_project_keeps_its_analyzer_warnings() {
        let d = temp("memory");
        let (app, lib) = (d.join("App.dll"), d.join("Lib.dll"));
        std::fs::write(&app, "1").unwrap();
        std::fs::write(&lib, "1").unwrap();
        let finished = vec![("App".to_owned(), app.clone()), ("Lib".to_owned(), lib.clone())];
        let mut m = BuildMemory::default();
        let first = vec![warn("App", "SA1633"), warn("Lib", "SA1400")];
        assert_eq!(m.merge(&first, &finished), first);
        // Second build, nothing compiled: MSBuild reports nothing.
        assert_eq!(m.merge(&[], &finished), first);
        // Lib compiled again with its warning fixed; App still up to date.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&lib, "2").unwrap();
        assert_eq!(m.merge(&[], &finished), vec![warn("App", "SA1633")]);
        // Kept across sessions.
        m.save(&d).unwrap();
        let mut again = BuildMemory::load(&d);
        assert_eq!(again, m);
        assert_eq!(again.merge(&[], &finished), vec![warn("App", "SA1633")]);
        // App fails now: no output line, only its fresh errors count.
        let err = crate::diagnostics::Diagnostic { severity: crate::diagnostics::Severity::Error, code: "CS1002".into(), ..warn("App", "") };
        assert_eq!(again.merge(std::slice::from_ref(&err), &finished[1..]), vec![err.clone()]);
        assert_eq!(again.diagnostics.get("App"), Some(&vec![err]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn project_names_from_diagnostics() {
        let mut d = warn("App", "SA1633");
        assert_eq!(project_name(&d).as_deref(), Some("App"));
        d.project = Some("/w/My Lib/My Lib.csproj::TargetFramework=net8.0".into());
        assert_eq!(project_name(&d).as_deref(), Some("My Lib"));
        d.project = None;
        assert_eq!(project_name(&d), None);
    }

    #[test]
    fn reads_the_restored_project_path() {
        assert_eq!(assets_project_path(&assets_for("/home/t/\\u00dcbung 1/App.csproj")).as_deref(), Some("/home/t/Übung 1/App.csproj"));
        assert_eq!(assets_project_path(&assets_for("C:\\\\Users\\\\s\\\\App.csproj")).as_deref(), Some("C:\\Users\\s\\App.csproj"));
        assert_eq!(assets_project_path("{}"), None);
        assert!(needs_restore_retry("error NETSDK1064: Package StyleCop.Analyzers, version 1.1.118 was not found."));
    }

    #[test]
    fn assets_current_by_mtime() {
        let d = temp("assets");
        let p = d.join("App.csproj");
        std::fs::write(&p, csproj(&[])).unwrap();
        assert!(!assets_current(&p), "never restored");
        std::fs::create_dir_all(d.join("obj")).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(d.join("obj/project.assets.json"), assets_for("C:\\\\Users\\\\s\\\\App\\\\App.csproj")).unwrap();
        assert!(!assets_current(&p), "restored on Windows: its package folders are not here");
        std::fs::write(d.join("obj/project.assets.json"), assets_for(&p.to_string_lossy())).unwrap();
        assert!(assets_current(&p));
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(d.join("Directory.Packages.props"), "<Project/>").unwrap();
        assert!(!assets_current(&p), "central package versions changed");
        std::thread::sleep(Duration::from_millis(20));
        // A restore that found nothing to do touches only the cache file.
        std::fs::write(d.join("obj/project.nuget.cache"), "{}").unwrap();
        assert!(assets_current(&p));
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&p, csproj(&["x.csproj"])).unwrap();
        assert!(!assets_current(&p), "project file edited");
        std::thread::sleep(Duration::from_millis(20));
        mark_restored(std::slice::from_ref(&p));
        assert!(assets_current(&p), "a no-op restore counts once the build marked it");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn progress_and_retry_lines() {
        assert_eq!(finished_project("  Core -> /w/Core/bin/Debug/net10.0/Core.dll"), Some(("Core", "/w/Core/bin/Debug/net10.0/Core.dll")));
        assert_eq!(finished_project("  App.Tests -> C:\\w\\App.Tests.dll").map(|f| f.0), Some("App.Tests"));
        assert_eq!(finished_project("  Determining projects to restore..."), None);
        assert_eq!(finished_project("x => y -> z"), None);
        assert!(needs_restore_retry("/w/App/App.csproj : error NETSDK1004: Assets file '/w/App/obj/project.assets.json' not found."));
        assert!(!needs_restore_retry("Program.cs(3,1): error CS1002: ; expected"));
    }

    #[test]
    fn elapsed_formats() {
        assert_eq!(format_elapsed(Duration::from_millis(3240)), "3.2 s");
        assert_eq!(format_elapsed(Duration::from_secs(65)), "1:05");
        assert_eq!(format_running(Duration::from_millis(3900)), "3 s");
        assert_eq!(format_running(Duration::from_secs(125)), "2:05");
    }
}
