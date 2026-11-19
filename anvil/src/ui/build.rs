//! Build, Rebuild, Clean, Start Without Debugging, Hot Reload and Hand in
//! as ZIP, all through the host's `dotnet`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use adw::prelude::*;
use anvil_core::diagnostics;
use anvil_core::host::Command;
use gtk::{gio, glib};

use crate::host;
use crate::i18n::{tr, trf};
use crate::ui::window::{self};

#[derive(Clone, Debug)]
pub enum Target {
    Solution,
    Project(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Build,
    Rebuild,
    Clean,
}

#[derive(Default)]
pub struct Builder {
    running: RefCell<Option<host::Streaming>>,
}

impl Builder {
    pub fn new() -> Builder {
        Builder::default()
    }

    pub fn cancel(&self) {
        if let Some(s) = self.running.borrow_mut().as_mut() {
            s.kill();
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.borrow().is_some()
    }
}

pub fn current_project() -> Option<String> {
    let app = crate::ui::window::app();
    let path = app.tabs.current_path()?;
    let sol = app.solution.borrow();
    sol.as_ref()?.project_of(&path).map(|p| p.name.clone())
}

fn target_path(t: &Target) -> Option<PathBuf> {
    let app = crate::ui::window::app();
    let sol = app.solution.borrow();
    let sol = sol.as_ref()?;
    match t {
        Target::Solution => Some(sol.path.clone()),
        Target::Project(n) => sol.project_named(n).map(|p| p.path.clone()),
    }
}

/// Builds and calls `done(success)`; files are saved first, as VS does.
pub fn build_then(target: Target, verb: Verb, done: impl FnOnce(bool) + 'static) {
    let app = crate::ui::window::app();
    if app.builder.is_running() {
        app.status.flash(&tr("A build is already running."));
        done(false);
        return;
    }
    let Some(path) = target_path(&target) else {
        app.status.flash(&tr("Open a solution first."));
        done(false);
        return;
    };
    app.tabs.save_all();
    let dir = window::solution_dir().unwrap_or_default();
    let verb_arg = match verb {
        Verb::Build | Verb::Rebuild => "build",
        Verb::Clean => "clean",
    };
    let mut cmd = Command::dotnet().arg(verb_arg).arg(path.to_string_lossy()).args(["-nologo", "-consoleLoggerParameters:NoSummary"]).cwd(&dir);
    if verb == Verb::Rebuild {
        cmd = cmd.arg("--no-incremental");
    }
    app.output.clear("Build");
    app.output.show("Build");
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    app.output.append("Build", &format!("{} {}\n", trf("Build started: {}", &[&name]), glib::DateTime::now_local().map(|d| d.format("%H:%M:%S").map(|s| s.to_string()).unwrap_or_default()).unwrap_or_default()));
    app.status.set_message(&tr("Build started…"));
    let mut stream = host::stream(cmd);
    let mut rx = std::mem::replace(&mut stream.lines, tokio::sync::mpsc::unbounded_channel().1);
    *app.builder.running.borrow_mut() = Some(stream);
    let generation = app.generation.get();
    glib::spawn_future_local(async move {
        let mut log = String::new();
        let mut status = None;
        while let Some(line) = rx.recv().await {
            let app = crate::ui::window::app();
            match line {
                host::Line::Out(l) | host::Line::Err(l) => {
                    app.output.append("Build", &format!("{l}\n"));
                    log.push_str(&l);
                    log.push('\n');
                }
                host::Line::Done(s) => status = s,
            }
        }
        let app = crate::ui::window::app();
        app.builder.running.borrow_mut().take();
        if app.generation.get() != generation {
            done(false);
            return;
        }
        let diags = diagnostics::parse_log(&log);
        let c = diagnostics::counts(&diags);
        app.errors.set_build(&diags, window::solution_dir().as_deref());
        let ok = status == Some(0);
        let summary = if status.is_none() {
            tr("Build canceled")
        } else if ok {
            trf("Build succeeded. {} warnings.", &[&c.warnings.to_string()])
        } else {
            trf("Build failed. {} errors, {} warnings.", &[&c.errors.to_string(), &c.warnings.to_string()])
        };
        app.output.append("Build", &format!("========== {summary} ==========\n"));
        app.status.set_message(&summary);
        if !ok && c.errors > 0 {
            window::show_bottom("errors");
        } else if !ok && status.is_some() {
            // Failed without a parsed error (a missing SDK, a bad
            // project file): the log says why.
            window::show_bottom("output");
        }
        if ok && verb != Verb::Clean {
            app.tests.discover();
        }
        done(ok);
    });
}

pub fn build(target: Target, verb: Verb) {
    build_then(target, verb, |_| {});
}

fn startup() -> Option<anvil_core::solution::Project> {
    let app = crate::ui::window::app();
    let name = window::startup_project_name()?;
    app.solution.borrow().as_ref()?.project_named(&name).cloned()
}

/// Ctrl+F5: build the startup project, run it in the terminal panel with
/// the arguments from `launchSettings.json`, wait for a key at the end.
pub fn run_without_debugging() {
    let Some(p) = startup() else {
        crate::ui::window::app().status.flash(&tr("Open a solution first."));
        return;
    };
    build_then(Target::Project(p.name.clone()), Verb::Build, move |ok| {
        if !ok {
            return;
        }
        glib::spawn_future_local(async move {
            // The built program itself, in its output folder with the
            // launchSettings arguments and environment, as VS runs it.
            let dll = built_program(&p).await;
            let mut cmd = Command::new("dotnet").arg(dll.to_string_lossy()).args(launch_args(&p));
            for (k, v) in launch_env(&p) {
                cmd = cmd.env(k, v);
            }
            let cwd = dll.parent().map(std::path::Path::to_path_buf).unwrap_or_else(|| p.dir.clone());
            window::show_bottom("terminal");
            crate::ui::window::app().terminal.run_program(&cwd, &cmd, None);
        });
    });
}

/// The program MSBuild built (`TargetPath`), or VS' default location
/// `bin/Debug/<tfm>/<AssemblyName>.dll`.
pub async fn built_program(p: &anvil_core::solution::Project) -> std::path::PathBuf {
    let cmd = Command::dotnet().arg("msbuild").arg(p.path.to_string_lossy()).args(["-getProperty:TargetPath", "-nologo"]).cwd(&p.dir);
    if let Ok(out) = host::run(cmd, None).await
        && out.ok()
        && let Some(path) = anvil_dap::launch::parse_target_path(&out.stdout_text())
    {
        return path;
    }
    let tfm = p.csproj.as_ref().and_then(|c| c.target_framework()).unwrap_or_else(|| "net10.0".into());
    let assembly = p.csproj.as_ref().and_then(|c| c.property("AssemblyName")).unwrap_or_else(|| p.name.clone());
    anvil_dap::launch::output_dll(&p.dir, &assembly, &tfm)
}

/// The environment variables VS would set (launchSettings.json).
pub fn launch_env(p: &anvil_core::solution::Project) -> Vec<(String, String)> {
    std::fs::read_to_string(p.dir.join("Properties/launchSettings.json")).map(|t| anvil_core::launch::env(&t)).unwrap_or_default()
}

/// The command-line arguments VS would pass (launchSettings.json).
pub fn launch_args(p: &anvil_core::solution::Project) -> Vec<String> {
    let path = p.dir.join("Properties/launchSettings.json");
    std::fs::read_to_string(path).ok().and_then(|t| anvil_core::launch::args(&t)).map(|a| anvil_core::launch::split_args(&a)).unwrap_or_default()
}

/// Hot Reload without the debugger: `dotnet watch` in the terminal; saving
/// a file applies the change.
pub fn hot_reload() {
    let app = crate::ui::window::app();
    if app.debugger.is_active() {
        app.status.flash(&tr("Hot Reload is not available while debugging (netcoredbg has no Edit and Continue)."));
        return;
    }
    let Some(p) = startup() else { return };
    app.tabs.save_all();
    let cmd = Command::dotnet().args(["watch", "run", "--project"]).arg(p.path.to_string_lossy()).arg("--").args(launch_args(&p));
    window::show_bottom("terminal");
    app.terminal.run_program(&p.dir, &cmd, None);
    window::toast(&tr("Hot Reload is on: saving a file applies the change."));
}

/// Runs a `dotnet` command that rewrites `file` (`dotnet sln add`), then
/// puts back the file's BOM and line endings, which the SDK on Linux
/// writes differently from VS.
pub async fn dotnet_on_file(file: &Path, args: Vec<String>) -> Result<(), String> {
    let before = anvil_core::text::TextFile::read(file).map_err(|e| e.to_string())?;
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let out = host::run(Command::dotnet().args(args).cwd(&dir), None).await.map_err(|e| e.to_string())?;
    let app = crate::ui::window::app();
    app.output.append("Build", &out.stdout_text());
    if !out.ok() {
        return Err(format!("{}{}", out.stdout_text(), out.stderr_text()));
    }
    if let Ok(after) = std::fs::read(file) {
        let fixed = anvil_core::text::restyle(&after, before.encoding, before.eol);
        if fixed != after {
            anvil_core::text::write_atomic(file, &fixed).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Moves a project to the top of the solution, so VS starts it.
pub fn move_project_first(name: &str) {
    let app = crate::ui::window::app();
    let Some(sol) = app.solution.borrow().clone() else { return };
    let Some(project) = sol.project_named(name).cloned() else { return };
    let name = name.to_owned();
    glib::spawn_future_local(async move {
        let rel = |p: &Path| p.strip_prefix(&sol.dir).unwrap_or(p).to_string_lossy().into_owned();
        let others: Vec<String> = sol.projects.iter().filter(|p| p.name != name).map(|p| rel(&p.path)).collect();
        // Remove all, then add the chosen one first and the rest in order:
        // the SDK writes the file as VS would.
        let mut all = vec![rel(&project.path)];
        all.extend(others);
        let sln = sol.path.clone();
        let mut res = dotnet_on_file(&sln, [vec!["sln".into(), sln.to_string_lossy().into_owned(), "remove".into()], all.clone()].concat()).await;
        if res.is_ok() {
            res = dotnet_on_file(&sln, [vec!["sln".into(), sln.to_string_lossy().into_owned(), "add".into()], all].concat()).await;
        }
        match res {
            Ok(()) => {
                crate::ui::explorer::reload_solution_model();
                window::toast(&trf("{} is now the first project.", &[&name]));
            }
            Err(e) => window::show_error(&tr("The solution could not be changed"), &e),
        }
    });
}

/// File → Hand in as ZIP…: build, show the counts, then write the ZIP.
pub fn hand_in() {
    let app = crate::ui::window::app();
    let Some(sol) = app.solution.borrow().clone() else {
        app.status.flash(&tr("Open a solution first."));
        return;
    };
    build_then(Target::Solution, Verb::Build, move |ok| {
        let app = crate::ui::window::app();
        let (errors, warnings) = app.errors.counts();
        let files = anvil_core::handin::handin_files(&sol.dir);
        let detail = trf("Build: {} errors, {} warnings. The ZIP holds {} files, without bin, obj and .vs.", &[&errors.to_string(), &warnings.to_string(), &files.len().to_string()]);
        let heading = if ok && errors == 0 { tr("Hand in as ZIP") } else { tr("The solution does not build") };
        let d = adw::AlertDialog::new(Some(&heading), Some(&detail));
        d.add_responses(&[("cancel", &tr("Cancel")), ("zip", &if ok { tr("Save ZIP…") } else { tr("Hand In Anyway…") })]);
        if !ok {
            d.set_response_appearance("zip", adw::ResponseAppearance::Destructive);
        }
        d.set_default_response(Some(if ok { "zip" } else { "cancel" }));
        let sol2 = sol.clone();
        d.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
            if r != "zip" {
                return;
            }
            let default = anvil_core::handin::default_zip_path(&sol2.dir, &sol2.name);
            let fd = gtk::FileDialog::builder().title(tr("Hand in as ZIP")).initial_name(default.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).build();
            if let Some(parent) = default.parent() {
                fd.set_initial_folder(Some(&gio::File::for_path(parent)));
            }
            let dir = sol2.dir.clone();
            fd.save(Some(&crate::ui::window::app().window), gio::Cancellable::NONE, move |res| {
                let Ok(file) = res else { return };
                let Some(out) = file.path() else { return };
                match anvil_core::handin::write_zip(&dir, &out) {
                    Ok(n) => window::toast(&trf("{} written ({} files).", &[&out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), &n.to_string()])),
                    Err(e) => window::show_error(&tr("The ZIP could not be written"), &e.to_string()),
                }
            });
        });
    });
}
