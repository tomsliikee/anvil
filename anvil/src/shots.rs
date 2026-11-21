//! Development screenshots without compositor rights, as Atlas does. With
//! `ANVIL_SCREENSHOT=<dir>` the window renders itself through its own GSK
//! renderer into PNGs at twice the logical size, walking through Anvil:
//! start page; with `ANVIL_OPEN=<solution>` the editor, the Solution
//! Explorer's menu, the class diagram, Add New Item, Error List after a
//! build, Settings → Keyboard, and dark. Then it quits and restores the
//! stored theme. Without the variable nothing here runs.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib, graphene};

use crate::ui::window::{self};

const SCALE: f32 = 2.0;

pub fn requested() -> Option<PathBuf> {
    std::env::var_os("ANVIL_SCREENSHOT").map(PathBuf::from)
}

/// Saves what `widget` (a window or a dialog's toplevel) shows now.
fn save(widget: &gtk::Widget, dir: &Path, name: &str) {
    let (w, h) = (widget.width() as f32, widget.height() as f32);
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    snapshot.scale(SCALE, SCALE);
    paintable.snapshot(&snapshot, w as f64, h as f64);
    let Some(node) = snapshot.to_node() else {
        eprintln!("anvil: {name}: nothing to render");
        return;
    };
    let Some(renderer) = widget.native().and_then(|n| n.renderer()) else {
        eprintln!("anvil: {name}: no renderer");
        return;
    };
    let bounds = graphene::Rect::new(0.0, 0.0, w * SCALE, h * SCALE);
    let texture: gdk::Texture = renderer.render_texture(&node, Some(&bounds));
    let path = dir.join(format!("{name}.png"));
    match texture.save_to_png(&path) {
        Ok(()) => println!("anvil: wrote {}", path.display()),
        Err(e) => eprintln!("anvil: {}: {e}", path.display()),
    }
}

enum Step {
    Shot(&'static str),
    Act(Box<dyn Fn()>),
    Wait(u64),
    /// Polls every 200 ms until the condition holds, at most this long.
    Until(Box<dyn Fn() -> bool>, u64),
    Theme(adw::ColorScheme),
}

fn until(f: impl Fn() -> bool + 'static, max_ms: u64) -> Step {
    Step::Until(Box::new(f), max_ms)
}

fn act(f: impl Fn() + 'static) -> Step {
    Step::Act(Box::new(f))
}

fn command(id: &'static str) -> Step {
    act(move || crate::ui::actions::dispatch(id))
}

fn close_dialogs() {
    let app = crate::ui::window::app();
    if let Some(d) = app.window.visible_dialog() {
        d.force_close();
    }
}

pub fn run(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceLight);
    let mut steps = vec![Step::Wait(800), Step::Shot("start-light")];
    // ANVIL_SHOTS=ui: a quick pass over the layout without build and AI.
    if std::env::var("ANVIL_SHOTS").as_deref() == Ok("ui")
        && let Ok(open) = std::env::var("ANVIL_OPEN")
    {
        steps.extend(ui_steps(PathBuf::from(open)));
        next(Rc::new(dir), Rc::new(steps), 0);
        return;
    }
    // ANVIL_SHOTS=typing: types into ANVIL_TYPE_FILE (a large file of the
    // solution, at its `// TYPE HERE` line) for the frame log.
    if std::env::var("ANVIL_SHOTS").as_deref() == Ok("typing")
        && let (Ok(open), Ok(file)) = (std::env::var("ANVIL_OPEN"), std::env::var("ANVIL_TYPE_FILE"))
    {
        steps.extend(typing_steps(PathBuf::from(open), PathBuf::from(file)));
        next(Rc::new(dir), Rc::new(steps), 0);
        return;
    }
    // ANVIL_SHOTS=debug: F5 for real with netcoredbg on a sample console
    // app (Program.cs with a loop at line 11 and a Console.ReadLine).
    if std::env::var("ANVIL_SHOTS").as_deref() == Ok("debug")
        && let Ok(open) = std::env::var("ANVIL_OPEN")
    {
        steps.extend(debug_steps(PathBuf::from(open)));
        next(Rc::new(dir), Rc::new(steps), 0);
        return;
    }
    if let Ok(open) = std::env::var("ANVIL_OPEN") {
        let sol = PathBuf::from(&open);
        let project_dir = sol.parent().map(|d| d.join(sol.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())).unwrap_or_default();
        let person = project_dir.join("Person.cs");
        let diagram = project_dir.join("ClassDiagram1.cd");
        let list_file = project_dir.join("DoublyLinkedPersonList.cs");
        steps.extend([
            act(move || window::open_path(&sol)),
            Step::Wait(1500),
            act(move || {
                crate::ui::window::app().tabs.open(&person, false);
            }),
            Step::Wait(900),
            Step::Shot("editor-light"),
            command("build.solution"),
            // A cold `dotnet build` with Roslyn loading next to it takes
            // longer than 14 s on this PC.
            Step::Wait(30000),
            act(|| window::show_bottom("errors")),
            Step::Wait(500),
            Step::Shot("errors-light"),
            act(move || {
                crate::ui::window::app().tabs.open(&diagram, false);
            }),
            Step::Wait(1200),
            Step::Shot("diagram-light"),
            act(|| {
                let app = crate::ui::window::app();
                if let Some(crate::ui::editor::Doc::Diagram(d)) = app.tabs.current().as_deref() {
                    let c = d.canvas.clone();
                    crate::ui::menus::popup(c.upcast_ref(), 520.0, 260.0, &[crate::ui::menus::Item::Command("designer.layout")]);
                }
            }),
            Step::Wait(300),
            act(|| {
                // Select the project so New Item has a target.
                window::show_side("explorer");
            }),
            command("project.add-new-item"),
            Step::Wait(1500),
            Step::Shot("newitem-light"),
            act(close_dialogs),
            Step::Wait(300),
            act(|| crate::ui::settings::show(Some("keyboard"))),
            Step::Wait(900),
            Step::Shot("keyboard-light"),
            act(close_dialogs),
            Step::Wait(300),
            // AI: ghost text from the local Ollama, then the chat.
            act(move || {
                let app = crate::ui::window::app();
                let _ = app.settings.set_boolean("ai-enabled", true);
                let _ = app.settings.set_boolean("ai-inline", true);
                let _ = app.settings.set_boolean("ai-chat", true);
                app.tabs.open(&list_file, false);
                if let Some(d) = app.tabs.current_code() {
                    let text = d.text();
                    if let Some(i) = text.find("{\n\n") {
                        let mut it = d.iter_at_byte(i + 2);
                        d.buffer.insert(&mut it, "        public int Count { get; private set; }\n\n        public void Clear()\n        {\n            ");
                        d.buffer.place_cursor(&it);
                    }
                }
            }),
            Step::Wait(45000),
            Step::Shot("ghost-light"),
            act(|| {
                let app = crate::ui::window::app();
                window::show_side("chat");
                app.chat.ask_for_shots("Why does Program.cs not compile? Answer in two sentences.");
            }),
            Step::Wait(60000),
            Step::Shot("chat-light"),
            Step::Theme(adw::ColorScheme::ForceDark),
            Step::Wait(800),
            Step::Shot("chat-dark"),
            act(|| {
                let app = crate::ui::window::app();
                window::show_side("explorer");
                if app.tabs.view.n_pages() > 1 {
                    app.tabs.view.set_selected_page(&app.tabs.view.nth_page(1));
                }
            }),
            Step::Wait(800),
            Step::Shot("diagram-dark"),
            act(|| {
                let app = crate::ui::window::app();
                let n = app.tabs.view.n_pages();
                if n > 0 {
                    app.tabs.view.set_selected_page(&app.tabs.view.nth_page(0));
                }
            }),
            Step::Wait(600),
            Step::Shot("editor-dark"),
        ]);
    } else {
        steps.extend([Step::Theme(adw::ColorScheme::ForceDark), Step::Wait(600), Step::Shot("start-dark")]);
    }
    // ANVIL_NEW_PROJECT=<folder>: File → New Project → Console App with
    // StyleCop, through the host's dotnet, then a build.
    if let Ok(dir) = std::env::var("ANVIL_NEW_PROJECT") {
        steps.extend([
            Step::Theme(adw::ColorScheme::ForceLight),
            act(move || {
                let dir = PathBuf::from(&dir);
                glib::spawn_future_local(async move {
                    let t = crate::ui::dialogs::project_templates().remove(1);
                    if let Err(e) = crate::ui::dialogs::create_project(t, "Uebung1", &dir, false, None).await {
                        eprintln!("anvil: new project: {e}");
                    }
                });
            }),
            Step::Wait(25000),
            command("build.solution"),
            Step::Wait(20000),
            Step::Shot("newproject-light"),
        ]);
    }
    next(Rc::new(dir), Rc::new(steps), 0);
}

/// Four files opened as preview tabs one after another (the order that
/// crashed), the diagram, and the dialogs, light and dark.
fn ui_steps(sol: PathBuf) -> Vec<Step> {
    let project_dir = sol.parent().map(|d| d.join(sol.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())).unwrap_or_default();
    let files = ["Program.cs", "Person.cs", "ListNode.cs", "DoublyLinkedPersonList.cs"].map(|f| project_dir.join(f));
    let diagram = project_dir.join("ClassDiagram1.cd");
    let mut steps = vec![act(move || window::open_path(&sol)), Step::Wait(1500)];
    for f in files.clone() {
        steps.push(act(move || {
            crate::ui::window::app().tabs.open(&f, true);
        }));
        steps.push(Step::Wait(300));
    }
    let keep = files[3].clone();
    steps.extend([
        act(move || {
            crate::ui::window::app().tabs.open(&keep, false);
        }),
        Step::Wait(300),
        act(move || {
            crate::ui::window::app().tabs.open(&diagram, false);
        }),
        Step::Wait(1200),
        Step::Shot("ui-diagram-light"),
        act(|| {
            let app = crate::ui::window::app();
            app.tabs.view.set_selected_page(&app.tabs.view.nth_page(0));
        }),
        Step::Wait(400),
        Step::Shot("ui-editor-light"),
        act(|| crate::ui::dialogs::new_project(None)),
        Step::Wait(800),
        Step::Shot("ui-newproject-light"),
        act(close_dialogs),
        Step::Theme(adw::ColorScheme::ForceDark),
        Step::Wait(600),
        Step::Shot("ui-editor-dark"),
        act(|| crate::ui::dialogs::new_project(None)),
        Step::Wait(800),
        Step::Shot("ui-newproject-dark"),
        act(close_dialogs),
        Step::Wait(300),
        act(|| {
            window::show_side("explorer");
        }),
        command("project.add-new-item"),
        Step::Wait(1000),
        Step::Shot("ui-newitem-dark"),
        act(close_dialogs),
        Step::Wait(300),
        act(|| {
            let app = crate::ui::window::app();
            app.tabs.view.set_selected_page(&app.tabs.view.nth_page(1));
        }),
        Step::Wait(800),
        Step::Shot("ui-diagram-dark"),
        act(|| crate::ui::settings::show(Some("keyboard"))),
        Step::Wait(800),
        Step::Shot("ui-settings-dark"),
        act(close_dialogs),
    ]);
    steps
}

/// Waits for Roslyn, then types C# at about 14 keys a second, with a
/// pause after each line (where highlighting, diagnostics and the
/// designer catch up), and scrolls; `ANVIL_TYPE_DIAGRAM` opens a class
/// diagram beside it. The frame log's summary covers the typing only.
fn typing_steps(sol: PathBuf, file: PathBuf) -> Vec<Step> {
    let diagram = std::env::var("ANVIL_TYPE_DIAGRAM").ok().map(PathBuf::from);
    let lines: usize = std::env::var("ANVIL_TYPE_LINES").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    let ready = || crate::ui::window::app().lsp.client().is_some();
    let mut steps = vec![act(move || window::open_path(&sol)), Step::Wait(1500)];
    if let Some(d) = diagram {
        steps.push(act(move || {
            crate::ui::window::app().tabs.open(&d, false);
        }));
        steps.push(Step::Wait(800));
    }
    steps.extend([
        act(move || {
            let app = crate::ui::window::app();
            app.tabs.open(&file, false);
            if let Some(d) = app.tabs.current_code()
                && let Some(i) = d.text().find("// TYPE HERE")
            {
                d.goto_line_col(d.iter_at_byte(i).line() + 1, 0);
            }
        }),
        until(ready, 120_000),
        // Roslyn loads the projects after it is ready.
        Step::Wait(25_000),
        Step::Shot("typing-before"),
        act(move || {
            crate::frame_log::reset();
            type_script(SCRIPT.repeat(lines.div_ceil(3)).chars().collect(), 0);
        }),
        until(|| TYPING_DONE.with(std::cell::Cell::get), 600_000),
        Step::Shot("typing-after"),
        Step::Wait(2000),
        act(|| {
            if let Some(d) = crate::ui::window::app().tabs.current_code() {
                d.buffer.set_modified(false);
            }
        }),
    ]);
    steps
}

const SCRIPT: &str = "            var list = new System.Collections.Generic.List<int>();\n            list.Add(list.Count + 42);\n            Console.WriteLine(list.Count);\n";

thread_local! {
    static TYPING_DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// One key per call, as the text view's own `insert-at-cursor` (the path
/// typed text takes), 70 ms apart and 700 ms after a line.
fn type_script(chars: Vec<char>, i: usize) {
    let Some(&c) = chars.get(i) else {
        TYPING_DONE.with(|d| d.set(true));
        return;
    };
    if let Some(d) = crate::ui::window::app().tabs.current_code() {
        crate::frame_log::keystroke();
        d.view.emit_insert_at_cursor(&c.to_string());
    }
    let delay = if c == '\n' { 700 } else { 70 };
    glib::timeout_add_local_once(Duration::from_millis(delay), move || type_script(chars, i + 1));
}

/// Breakpoint with hit count 3 in the loop, F5, Locals, a step (changed
/// values), Watch, Call Stack, Breakpoints, then input in the terminal and
/// the end of the program; light and dark.
fn debug_steps(sol: PathBuf) -> Vec<Step> {
    let p2 = sol.parent().map(|d| d.join("DebugDemo/Program.cs")).unwrap_or_default();
    let stopped = || crate::ui::window::app().debugger.is_stopped();
    vec![
        act(move || window::open_path(&sol)),
        Step::Wait(1500),
        act(move || {
            let app = crate::ui::window::app();
            app.tabs.open(&p2, false);
            app.debugger.clear_breakpoints();
            app.debugger.toggle_breakpoint(&p2, 11);
            app.debugger.set_hit_count(&p2, 11, 3, anvil_core::session::HitMode::Equal);
            app.debugger.toggle_breakpoint(&p2, 15);
            app.session.borrow_mut().watches = vec!["names".into(), "total * 2".into()];
        }),
        Step::Wait(500),
        Step::Shot("debug-breakpoints-set-light"),
        command("debug.start"),
        until(stopped, 120_000),
        Step::Wait(1500),
        Step::Shot("debug-stopped-light"),
        command("debug.step-over"),
        until(stopped, 10_000),
        command("debug.step-over"),
        until(stopped, 10_000),
        Step::Wait(1200),
        Step::Shot("debug-step-light"),
        act(|| window::show_bottom("watch")),
        Step::Wait(1200),
        Step::Shot("debug-watch-light"),
        act(|| window::show_bottom("breakpoints")),
        Step::Wait(500),
        Step::Shot("debug-breakpoints-light"),
        Step::Theme(adw::ColorScheme::ForceDark),
        act(|| window::show_bottom("locals")),
        Step::Wait(800),
        Step::Shot("debug-step-dark"),
        act(|| window::show_bottom("call-stack")),
        Step::Wait(400),
        Step::Shot("debug-callstack-dark"),
        // On to the second breakpoint, then past it to Console.ReadLine.
        command("debug.start"),
        until(stopped, 10_000),
        Step::Wait(800),
        Step::Shot("debug-second-dark"),
        command("debug.start"),
        Step::Wait(1500),
        act(|| window::show_bottom("terminal")),
        act(|| crate::ui::window::app().terminal.send_input("Ada\r")),
        until(|| !crate::ui::window::app().debugger.is_active(), 15_000),
        Step::Wait(800),
        Step::Shot("debug-ended-dark"),
        act(move || {
            let app = crate::ui::window::app();
            app.debugger.clear_breakpoints();
            app.session.borrow_mut().watches.clear();
            window::save_session();
        }),
    ]
}

fn next(dir: Rc<PathBuf>, steps: Rc<Vec<Step>>, i: usize) {
    let app = crate::ui::window::app();
    let Some(step) = steps.get(i) else {
        let settings = gio::Settings::new(crate::APP_ID);
        for key in [crate::theme::KEY, "ai-enabled", "ai-inline", "ai-chat", "ai-chat-model", "recent-solutions"] {
            settings.reset(key);
        }
        app.tabs.mark_all_clean();
        if let Some(a) = GtkWindowExt::application(&app.window) {
            a.quit();
        }
        return;
    };
    let delay = match step {
        Step::Shot(name) => {
            // A dialog is its own toplevel in libadwaita's window; the
            // window's snapshot includes it.
            save(app.window.upcast_ref(), &dir, name);
            0
        }
        Step::Act(f) => {
            f();
            0
        }
        Step::Wait(ms) => *ms,
        Step::Until(cond, max) => {
            if !cond() && *max >= 200 {
                let left = max - 200;
                let steps2 = steps.clone();
                glib::timeout_add_local_once(Duration::from_millis(200), move || until_then(dir, steps2, i, left));
                return;
            }
            if !cond() {
                eprintln!("anvil: step {i}: condition not met in time");
            }
            0
        }
        Step::Theme(s) => {
            adw::StyleManager::default().set_color_scheme(*s);
            0
        }
    };
    glib::timeout_add_local_once(Duration::from_millis(delay.max(16)), move || next(dir, steps, i + 1));
}

/// Re-checks an `Until` step until it holds or its time is up.
fn until_then(dir: Rc<PathBuf>, steps: Rc<Vec<Step>>, i: usize, left: u64) {
    let Some(Step::Until(cond, _)) = steps.get(i) else { return };
    if cond() || left < 200 {
        if !cond() {
            eprintln!("anvil: step {i}: condition not met in time");
        }
        next(dir, steps, i + 1);
        return;
    }
    glib::timeout_add_local_once(Duration::from_millis(200), move || until_then(dir, steps, i, left - 200));
}
