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
use crate::ui::jobs;
use crate::ui::window::{self};

#[derive(Clone, Debug)]
pub enum Target {
    Solution,
    Project(String),
}

pub use anvil_core::build::Verb;

#[derive(Default)]
pub struct Builder {
    running: RefCell<Option<host::Streaming>>,
    /// What each project's last build wrote (`App -> …/App.dll`).
    outputs: RefCell<std::collections::HashMap<String, PathBuf>>,
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

/// The project files a build of `t` covers, for the restore check and
/// the progress count.
fn graph(t: &Target, path: &Path) -> Vec<PathBuf> {
    match t {
        Target::Solution => crate::ui::window::app().solution.borrow().as_ref().map(|s| s.projects.iter().map(|p| p.path.clone()).collect()).unwrap_or_default(),
        Target::Project(_) => anvil_core::build::project_graph(path),
    }
}

/// Builds and calls `done(success)`; files are saved first, as VS does.
/// The build shows as a job in the status line and ends in a toast; the
/// Output window is never opened by itself. Restore runs only when a
/// project's assets are older than its inputs.
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
    let projects = graph(&target, &path);
    let restore = verb != Verb::Clean && !projects.iter().all(|p| anvil_core::build::assets_current(p));
    let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let text = match verb {
        Verb::Build => trf("Building {}…", &[&name]),
        Verb::Rebuild => trf("Rebuilding {}…", &[&name]),
        Verb::Clean => trf("Cleaning {}…", &[&name]),
    };
    let job = jobs::start("build", &text).cancellable(|| crate::ui::window::app().builder.cancel());
    app.output.clear("Build");
    app.output.show("Build");
    app.errors.set_build(&[], None);
    let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    app.output.append("Build", &format!("{} {}\n", trf("Build started: {}", &[&file]), glib::DateTime::now_local().map(|d| d.format("%H:%M:%S").map(|s| s.to_string()).unwrap_or_default()).unwrap_or_default()));
    app.status.set_message("");
    run(Run { verb, path, restore, retried: false, projects, text, job }, Box::new(done));
}

struct Run {
    verb: Verb,
    path: PathBuf,
    restore: bool,
    /// Already run again with restore after `--no-restore` met stale assets.
    retried: bool,
    projects: Vec<PathBuf>,
    text: String,
    job: jobs::Job,
}

fn run(r: Run, done: Box<dyn FnOnce(bool)>) {
    let app = crate::ui::window::app();
    let dir = window::solution_dir().unwrap_or_default();
    let args = anvil_core::build::dotnet_args(r.verb, &r.path, r.restore);
    app.output.append("Build", &format!("dotnet {}\n", args.join(" ")));
    let cmd = Command::dotnet().args(args).cwd(&dir);
    let mut stream = host::stream(cmd);
    let mut rx = std::mem::replace(&mut stream.lines, tokio::sync::mpsc::unbounded_channel().1);
    *app.builder.running.borrow_mut() = Some(stream);
    let generation = app.generation.get();
    let started = std::time::Instant::now();
    glib::spawn_future_local(async move {
        let mut log = String::new();
        let mut status = None;
        let mut diags: Vec<diagnostics::Diagnostic> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut finished = 0usize;
        let mut first_line = true;
        let mut shown_progress = 0usize;
        // The Error List fills while the build runs, a few times a second.
        let mut shown = 0usize;
        let mut last_shown = std::time::Instant::now();
        while let Some(line) = rx.recv().await {
            let app = crate::ui::window::app();
            match line {
                host::Line::Out(l) | host::Line::Err(l) => {
                    if first_line {
                        first_line = false;
                        jobs::trace(&format!("build first output after {} ms", started.elapsed().as_millis()));
                    }
                    if let Some(d) = diagnostics::parse_line(&l)
                        && seen.insert(d.clone())
                    {
                        diags.push(d);
                    }
                    if let Some((project, out)) = anvil_core::build::finished_project(&l) {
                        app.builder.outputs.borrow_mut().insert(project.to_owned(), PathBuf::from(out));
                        finished += 1;
                    }
                    if finished > 0 && r.projects.len() > 1 && shown_progress != finished {
                        shown_progress = finished;
                        r.job.set_text(&format!("{} {}", r.text, trf("{} of {}", &[&finished.min(r.projects.len()).to_string(), &r.projects.len().to_string()])));
                    }
                    if diags.len() != shown && last_shown.elapsed() > std::time::Duration::from_millis(300) {
                        shown = diags.len();
                        last_shown = std::time::Instant::now();
                        app.errors.set_build(&diags, window::solution_dir().as_deref());
                    }
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
            r.job.finish();
            done(false);
            return;
        }
        let ok = status == Some(0);
        // Assets that looked current but were not: once more, restoring.
        if !ok && status.is_some() && !r.restore && !r.retried && anvil_core::build::needs_restore_retry(&log) {
            app.output.append("Build", &format!("{}\n", tr("The package assets are out of date; building again with restore.")));
            run(Run { restore: true, retried: true, ..r }, done);
            return;
        }
        if ok && r.restore {
            anvil_core::build::mark_restored(&r.projects);
        }
        app.errors.set_build(&diags, window::solution_dir().as_deref());
        let c = diagnostics::counts(&diags);
        let elapsed = anvil_core::build::format_elapsed(r.job.elapsed());
        r.job.finish();
        jobs::trace(&format!("build finished after {} ms", started.elapsed().as_millis()));
        // The Error List's own counters, so both read the same.
        let counts = format!(
            "{}, {}",
            crate::i18n::ntr("{} Error", "{} Errors", c.errors as u32).replace("{}", &c.errors.to_string()),
            crate::i18n::ntr("{} Warning", "{} Warnings", c.warnings as u32).replace("{}", &c.warnings.to_string())
        );
        let summary = if status.is_none() {
            tr("Build canceled")
        } else if r.verb == Verb::Clean {
            if ok { tr("Clean succeeded") } else { tr("Clean failed") }
        } else if ok {
            trf("Build succeeded – {}", &[&counts])
        } else {
            trf("Build failed – {}", &[&counts])
        };
        app.output.append("Build", &format!("========== {summary} · {elapsed} ==========\n"));
        app.status.set_message(&summary);
        // The toast leads to the details: the Error List when there is
        // something in it, the log otherwise (a failure without a parsed
        // error: a missing SDK, a broken project file).
        let open = if status.is_none() {
            jobs::Open::Nothing
        } else if c.errors + c.warnings > 0 {
            jobs::Open::ErrorList
        } else {
            jobs::Open::Output("Build")
        };
        jobs::result("build", &format!("{summary} · {elapsed}"), open);
        if !ok && c.errors > 0 {
            // VS' default: a build with errors shows the Error List.
            window::show_bottom("errors");
        }
        if ok && r.verb != Verb::Clean {
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
            let dll = match built_program(&p).await {
                Ok(dll) => dll,
                Err(e) => {
                    crate::ui::window::app().status.set_message(&e);
                    window::toast(&e);
                    return;
                }
            };
            let mut cmd = Command::new("dotnet").arg(dll.to_string_lossy()).args(launch_args(&p));
            for (k, v) in launch_env(&p) {
                cmd = cmd.env(k, v);
            }
            let cwd = dll.parent().map(std::path::Path::to_path_buf).unwrap_or_else(|| p.dir.clone());
            run_in_terminal(&p.name, &cwd, &cmd);
        });
    });
}

/// Runs the program in the terminal as a "run" job: the status line shows
/// it until the program ends (not until the key press after it), the
/// result toast gives its exit code, and the Run pane keeps a line per run.
fn run_in_terminal(name: &str, cwd: &Path, cmd: &Command) {
    let app = crate::ui::window::app();
    let exit_file = anvil_core::session::dir(&window::solution_dir().unwrap_or_else(|| cwd.to_path_buf())).join("run-exit");
    let _ = std::fs::create_dir_all(exit_file.parent().unwrap_or(cwd));
    let _ = std::fs::remove_file(&exit_file);
    let job = jobs::start("run", &trf("Running {}…", &[name])).cancellable(|| crate::ui::window::app().terminal.kill());
    app.output.append("Run", &format!("{} {}\n", trf("Started {}:", &[name]), anvil_core::host::sh_line(&Command { env: Vec::new(), ..cmd.clone() })));
    window::show_bottom("terminal");
    let name = name.to_owned();
    let report = {
        let job = job.clone();
        let name = name.clone();
        move |code: Option<i32>| {
            if !job.is_current() {
                return;
            }
            let elapsed = anvil_core::build::format_elapsed(job.elapsed());
            job.finish();
            let text = match code {
                Some(c) => trf("{} exited with code {}", &[&name, &c.to_string()]),
                None => trf("{} was stopped", &[&name]),
            };
            crate::ui::window::app().output.append("Run", &format!("{text} · {elapsed}\n"));
            jobs::result("run", &format!("{text} · {elapsed}"), jobs::Open::Pane("terminal"));
        }
    };
    let report = std::rc::Rc::new(report);
    let on_exit = report.clone();
    let poll_file = exit_file.clone();
    app.terminal.run_program_noting_exit(cwd, cmd, Some(&exit_file), Some(Box::new(move |status| on_exit(Some(status)))));
    // The shell writes the code before it waits for the key press.
    glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        if !job.is_current() {
            return glib::ControlFlow::Break;
        }
        match std::fs::read_to_string(&poll_file).ok().and_then(|t| t.trim().parse::<i32>().ok()) {
            Some(code) => {
                report(Some(code));
                glib::ControlFlow::Break
            }
            None => glib::ControlFlow::Continue,
        }
    });
}

/// The program MSBuild built (`TargetPath`, of the first framework when
/// the project targets several, as VS starts), or VS' default location
/// `bin/Debug/<tfm>/<AssemblyName>.dll`. A class library is refused with
/// VS' reason instead of letting `dotnet` fail on it.
pub async fn built_program(p: &anvil_core::solution::Project) -> Result<PathBuf, String> {
    // The build just said where it wrote the program; asking MSBuild
    // again costs an evaluation of the project (a second or more).
    let reported = crate::ui::window::app().builder.outputs.borrow().get(&p.name).cloned();
    if p.is_startable()
        && let Some(dll) = reported.filter(|d| d.extension().is_some_and(|e| e == "dll") && d.is_file())
    {
        return Ok(dll);
    }
    let query = |framework: Option<&str>| {
        let mut cmd = Command::dotnet().arg("msbuild").arg(p.path.to_string_lossy()).args(anvil_dap::launch::PROGRAM_PROPERTIES.iter().map(|k| format!("-getProperty:{k}"))).arg("-nologo").cwd(&p.dir);
        if let Some(f) = framework {
            cmd = cmd.arg(format!("-property:TargetFramework={f}"));
        }
        async move { host::run(cmd, None).await.ok().filter(host::Output::ok).and_then(|o| anvil_dap::launch::parse_program_info(&o.stdout_text())) }
    };
    let mut info = query(None).await;
    if let Some(i) = &info
        && i.target_path.is_none()
        && let Some(first) = i.frameworks.first().cloned()
    {
        info = query(Some(&first)).await.or(info);
    }
    if let Some(i) = &info
        && !i.output_type.is_empty()
        && !i.is_startable()
    {
        return Err(trf("{} is a class library ({}) and cannot be started directly. Choose a console app as startup project (the list next to Start, or Set as Startup Project in the Solution Explorer).", &[&p.name, &i.output_type]));
    }
    if let Some(path) = info.and_then(|i| i.target_path) {
        return Ok(path);
    }
    let tfm = p.csproj.as_ref().and_then(|c| c.target_framework()).unwrap_or_else(|| "net10.0".into());
    let assembly = p.csproj.as_ref().and_then(|c| c.property("AssemblyName")).unwrap_or_else(|| p.name.clone());
    Ok(anvil_dap::launch::output_dll(&p.dir, &assembly, &tfm))
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
    jobs::info(&tr("Hot Reload is on: saving a file applies the change."));
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
                jobs::info(&trf("{} is now the first project.", &[&name]));
            }
            Err(e) => jobs::error("Build", &tr("The solution could not be changed"), &e),
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
                    Ok(n) => jobs::info(&trf("{} written ({} files).", &[&out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), &n.to_string()])),
                    Err(e) => jobs::error("General", &tr("The ZIP could not be written"), &e.to_string()),
                }
            });
        });
    });
}
