//! Changes on disk that Anvil did not make (a git switch or pull, NuGet,
//! another editor): Roslyn hears of them through
//! `workspace/didChangeWatchedFiles`, open tabs without edits reload, and a
//! tab with edits asks first, as Visual Studio does.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use adw::prelude::*;
use anvil_core::text::TextFile;
use gtk::{gio, glib};

use crate::i18n::{tr, trf};

/// Folders never watched: build output, tool state, history.
const SKIP: [&str; 6] = ["bin", "obj", ".vs", ".git", "node_modules", "TestResults"];

/// Files whose change matters to Roslyn or to an open tab.
fn relevant(p: &Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "cs" | "csproj" | "sln" | "slnx" | "props" | "targets" | "editorconfig" | "json" | "cd" | "resx" | "xaml")
}

/// LSP's FileChangeType: 1 created, 2 changed, 3 deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Created = 1,
    Changed = 2,
    Deleted = 3,
}

#[derive(Default)]
pub struct Watcher {
    monitors: RefCell<Vec<gio::FileMonitor>>,
    pending: RefCell<BTreeMap<PathBuf, Change>>,
    timer: RefCell<Option<glib::SourceId>>,
}

impl Watcher {
    pub fn new() -> Watcher {
        Watcher::default()
    }

    /// Watches the solution folder and its subfolders (GIO monitors are
    /// not recursive); stops watching the previous solution.
    pub fn watch(&self, root: Option<&Path>) {
        for m in self.monitors.borrow_mut().drain(..) {
            m.cancel();
        }
        self.pending.borrow_mut().clear();
        let Some(root) = root else { return };
        let mut dirs = vec![root.to_path_buf()];
        let mut count = 0;
        while let Some(d) = dirs.pop() {
            // A solution of a course is small; a huge folder is not
            // watched past this.
            count += 1;
            if count > 400 {
                break;
            }
            self.watch_dir(&d);
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if e.file_type().is_ok_and(|t| t.is_dir()) && !SKIP.contains(&name.as_str()) && !name.starts_with('.') {
                    dirs.push(p);
                }
            }
        }
    }

    fn watch_dir(&self, dir: &Path) {
        let Ok(m) = gio::File::for_path(dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else { return };
        m.connect_changed(|_, file, other, event| {
            let app = crate::ui::window::app();
            let Some(path) = file.path() else { return };
            let w = &app.watcher;
            match event {
                gio::FileMonitorEvent::Created | gio::FileMonitorEvent::MovedIn => {
                    if path.is_dir() {
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if !SKIP.contains(&name.as_str()) {
                            w.watch_dir(&path);
                        }
                    } else {
                        w.note(path, Change::Created);
                    }
                }
                gio::FileMonitorEvent::ChangesDoneHint | gio::FileMonitorEvent::Changed => w.note(path, Change::Changed),
                gio::FileMonitorEvent::Deleted | gio::FileMonitorEvent::MovedOut => w.note(path, Change::Deleted),
                gio::FileMonitorEvent::Renamed => {
                    w.note(path, Change::Deleted);
                    if let Some(to) = other.and_then(|o| o.path()) {
                        w.note(to, Change::Created);
                    }
                }
                _ => {}
            }
        });
        self.monitors.borrow_mut().push(m);
    }

    fn note(&self, path: PathBuf, change: Change) {
        if !relevant(&path) {
            return;
        }
        {
            let mut p = self.pending.borrow_mut();
            // Created then changed is still created.
            let c = match (p.get(&path), change) {
                (Some(Change::Created), Change::Changed) => Change::Created,
                (_, c) => c,
            };
            p.insert(path, c);
        }
        if let Some(id) = self.timer.borrow_mut().take() {
            id.remove();
        }
        let id = glib::timeout_add_local_once(Duration::from_millis(300), || {
            let app = crate::ui::window::app();
            app.watcher.timer.borrow_mut().take();
            let changes: Vec<(PathBuf, Change)> = std::mem::take(&mut *app.watcher.pending.borrow_mut()).into_iter().collect();
            apply(&changes);
        });
        *self.timer.borrow_mut() = Some(id);
    }
}

fn apply(changes: &[(PathBuf, Change)]) {
    let app = crate::ui::window::app();
    app.lsp.files_changed(changes);
    let mut structure = false;
    for (path, change) in changes {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if *change != Change::Changed || matches!(ext, "csproj" | "sln" | "slnx") {
            structure = true;
        }
        if *change == Change::Changed
            && let Some(doc) = app.tabs.code_doc(path)
        {
            reload_if_changed(&doc);
        }
    }
    if structure {
        crate::ui::explorer::reload_solution_model();
    }
    // Saves and outside changes show in Git Changes.
    app.git.refresh();
}

/// Takes the file's new text into an open tab: at once when the tab has no
/// edits, after asking when it has.
fn reload_if_changed(doc: &std::rc::Rc<crate::ui::editor::CodeDoc>) {
    let Ok(file) = TextFile::read(&doc.path()) else { return };
    if file.text == doc.text() {
        return;
    }
    if !doc.buffer.is_modified() {
        doc.replace_text(&file.text);
        doc.buffer.set_modified(false);
        return;
    }
    let app = crate::ui::window::app();
    let name = doc.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let d = adw::AlertDialog::new(Some(&trf("{} was changed outside Anvil", &[&name])), Some(&tr("This file has unsaved changes in Anvil too. Reload it and lose them, or keep Anvil's version?")));
    d.add_responses(&[("keep", &tr("Keep Mine")), ("reload", &tr("Reload"))]);
    d.set_response_appearance("reload", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("keep"));
    let doc = doc.clone();
    d.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
        if r == "reload"
            && let Ok(file) = TextFile::read(&doc.path())
        {
            doc.replace_text(&file.text);
            doc.buffer.set_modified(false);
        }
    });
}
