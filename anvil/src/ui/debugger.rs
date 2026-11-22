//! F5 with netcoredbg, attach-with-hold (see `anvil_dap::launch`): the
//! program runs in the terminal panel with a startup hook that waits; the
//! debugger attaches over stdin/stdout, sets the breakpoints and lets it
//! run. Hit counts are counted here (netcoredbg ignores them), Locals,
//! Autos and Watch show Name, Value and Type as in Visual Studio.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use anvil_core::host::Command;
use anvil_core::session::{Breakpoint, HitMode};
use anvil_core::solution::normalize;
use anvil_dap::client::{Client, Event, SourceBreakpoint, StackFrame};
use anvil_dap::display::Snapshot;
use anvil_dap::launch;
use gtk::{gio, glib};

use crate::host;
use crate::i18n::{tr, trf};
use crate::ui::editor::{self, CodeDoc};
use crate::ui::vartree::{self, Item, VarTree};
use crate::ui::window::{self};

struct Session {
    id: u64,
    client: Client,
    /// Ends the netcoredbg process if it does not leave by itself.
    stop_adapter: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<i64>,
    frame: Option<i64>,
    stopped: bool,
    temp_line: Option<(PathBuf, u32)>,
    current: Option<(PathBuf, u32)>,
    /// F10/F11 started the session: the entry function breakpoints are
    /// set and removed at the first stop.
    entry_stop: bool,
}

pub struct Debugger {
    breakpoints: RefCell<BTreeMap<PathBuf, BTreeMap<u32, Breakpoint>>>,
    /// Times each breakpoint was reached in the current (or last) session.
    hits: RefCell<HashMap<(PathBuf, u32), u32>>,
    /// The adapter's verdict per breakpoint; `false` = not bound (yet).
    verified: RefCell<HashMap<(PathBuf, u32), bool>>,
    bp_ids: RefCell<HashMap<i64, (PathBuf, u32)>>,
    session: RefCell<Option<Session>>,
    next_id: Cell<u64>,
    /// Between F5 and the attached session (build, hook, attach).
    starting: Cell<bool>,
    /// Stop (Shift+F5) while starting: the start gives up at its next step.
    cancel_start: Cell<bool>,
    downloading: Cell<bool>,
    pending_temp: RefCell<Option<(PathBuf, u32)>>,
    break_on_thrown: Cell<bool>,
    snapshot: Rc<RefCell<Snapshot>>,
    watch_snapshot: Rc<RefCell<Snapshot>>,
    locals: VarTree,
    autos: VarTree,
    watch: VarTree,
    watch_entry: gtk::Entry,
    stack: gtk::ListBox,
    immediate: gtk::TextView,
    immediate_entry: gtk::Entry,
    bp_list: gtk::ListBox,
    pages: RefCell<Vec<(gtk::Widget, &'static str, String)>>,
    stack_frames: RefCell<Vec<StackFrame>>,
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder().child(child).vexpand(true).build()
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl Debugger {
    pub fn new() -> Debugger {
        let immediate = gtk::TextView::builder().editable(false).monospace(true).left_margin(12).build();
        immediate.add_css_class("output");
        let stack = gtk::ListBox::new();
        stack.add_css_class("results");
        let bp_list = gtk::ListBox::new();
        bp_list.add_css_class("results");
        bp_list.add_css_class("breakpoints");
        Debugger {
            breakpoints: RefCell::default(),
            hits: RefCell::default(),
            verified: RefCell::default(),
            bp_ids: RefCell::default(),
            session: RefCell::default(),
            next_id: Cell::new(1),
            starting: Cell::new(false),
            cancel_start: Cell::new(false),
            downloading: Cell::new(false),
            pending_temp: RefCell::default(),
            break_on_thrown: Cell::new(false),
            snapshot: Rc::default(),
            watch_snapshot: Rc::default(),
            locals: VarTree::new(),
            autos: VarTree::new(),
            watch: VarTree::new(),
            watch_entry: gtk::Entry::builder().placeholder_text(tr("Add item to watch")).build(),
            stack,
            immediate,
            immediate_entry: gtk::Entry::builder().placeholder_text(tr("Evaluate an expression")).build(),
            bp_list,
            pages: RefCell::default(),
            stack_frames: RefCell::default(),
        }
    }

    /// Builds the panel pages. Breakpoints is always there; the others
    /// join the bottom panel while debugging or when opened from the
    /// Debug menu.
    pub fn add_pages(&self, bottom: &adw::ViewStack) {
        let watch_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        watch_box.append(&self.watch.widget);
        watch_box.append(&self.watch_entry);
        let imm = gtk::Box::new(gtk::Orientation::Vertical, 4);
        imm.append(&scrolled(&self.immediate));
        imm.append(&self.immediate_entry);
        let mut pages = vec![
            (self.locals.widget.clone().upcast::<gtk::Widget>(), "locals", tr("Locals")),
            (self.autos.widget.clone().upcast(), "autos", tr("Autos")),
            (watch_box.upcast(), "watch", tr("Watch 1")),
            (scrolled(&self.stack).upcast(), "call-stack", tr("Call Stack")),
            (imm.upcast(), "immediate", tr("Immediate Window")),
        ];
        let bps = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bps.append(&breakpoint_header());
        bps.append(&scrolled(&self.bp_list));
        bottom.add_titled(&bps, Some("breakpoints"), &tr("Breakpoints"));
        self.pages.borrow_mut().append(&mut pages);
    }

    fn show_pages(&self, on: bool) {
        let app = crate::ui::window::app();
        for (w, name, title) in self.pages.borrow().iter() {
            let present = app.bottom.child_by_name(name).is_some();
            if on && !present {
                app.bottom.add_titled(w, Some(name), title);
            } else if !on && present {
                app.bottom.remove(w);
            }
        }
    }

    /// Debug > Windows: the page exists even without a session.
    pub fn show_window(&self, name: &str) {
        if name != "breakpoints" {
            self.show_pages(true);
        }
        window::show_bottom(name);
    }

    pub fn connect(&self) {
        self.watch_entry.connect_activate(|e| {
            let app = crate::ui::window::app();
            let t = e.text().trim().to_owned();
            if !t.is_empty() {
                app.session.borrow_mut().watches.push(t);
                e.set_text("");
                glib::spawn_future_local(refresh_watch());
            }
        });
        // Delete removes the selected watch, as in VS.
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(|_, key, _, _| {
            if key != gtk::gdk::Key::Delete {
                return glib::Propagation::Proceed;
            }
            let app = crate::ui::window::app();
            let Some(name) = app.debugger.watch.selected_root() else { return glib::Propagation::Proceed };
            app.session.borrow_mut().watches.retain(|w| *w != name);
            glib::spawn_future_local(refresh_watch());
            glib::Propagation::Stop
        });
        self.watch.view.add_controller(keys);
        self.immediate_entry.connect_activate(|e| {
            let expr = e.text().to_string();
            e.set_text("");
            glib::spawn_future_local(async move {
                let app = crate::ui::window::app();
                let me = &app.debugger;
                let stopped = me.session.borrow().as_ref().filter(|s| s.stopped).map(|s| (s.client.clone(), s.frame));
                let Some((client, frame)) = stopped else {
                    append(&me.immediate, &format!("> {expr}\n{}\n", tr("The debugger is not stopped.")));
                    return;
                };
                let line = match client.evaluate(&expr, frame, "repl").await {
                    Ok(v) => v.value,
                    Err(e) => e.to_string(),
                };
                append(&app.debugger.immediate, &format!("> {expr}\n{line}\n"));
            });
        });
        self.stack.connect_row_activated(|list, row| {
            let app = crate::ui::window::app();
            let frames = app.debugger.stack_frames.borrow().clone();
            let Some(f) = frames.get(row.index() as usize) else { return };
            if let Some(s) = app.debugger.session.borrow_mut().as_mut() {
                s.frame = Some(f.id);
            }
            mark_current_frame(list, row.index());
            if let Some(p) = f.source.as_ref().and_then(|s| s.path.clone()) {
                app.tabs.goto(Path::new(&p), f.line as u32, f.column.max(1) as u32);
            }
            glib::spawn_future_local(async {
                refresh_variables().await;
                refresh_watch().await;
            });
        });
        self.bp_list.connect_row_activated(|_, row| {
            let app = crate::ui::window::app();
            let Some(b) = app.debugger.breakpoints().get(row.index() as usize).cloned() else { return };
            app.tabs.goto(&b.file, b.line, 1);
        });
        self.refresh_breakpoint_list();
        self.update_controls();
    }

    pub fn is_active(&self) -> bool {
        self.session.borrow().is_some()
    }

    pub fn is_stopped(&self) -> bool {
        self.session.borrow().as_ref().is_some_and(|s| s.stopped)
    }

    /// Sets a breakpoint's hit count (the screenshot run uses it; the
    /// Breakpoints window edits the same fields).
    pub fn set_hit_count(&self, file: &Path, line: u32, n: u32, mode: HitMode) {
        self.update(file, line, |b| {
            b.hit_count = Some(n);
            b.hit_mode = mode;
        });
        self.refresh_breakpoint_list();
    }

    pub fn is_starting(&self) -> bool {
        self.starting.get() || self.downloading.get()
    }

    /// The session in one block of text (the screenshot runs' log).
    pub fn describe(&self) -> String {
        let s = self.session.borrow();
        let state = match s.as_ref() {
            None if self.is_starting() => "starting".to_owned(),
            None => "no session".to_owned(),
            Some(s) if s.stopped => format!("stopped at {:?}", s.current),
            Some(_) => "running".to_owned(),
        };
        let marks: Vec<String> = self.breakpoints().iter().map(|b| format!("{}:{} verified={:?}", file_name(&b.file), b.line, self.verified.borrow().get(&(b.file.clone(), b.line)))).collect();
        let app = crate::ui::window::app();
        let pages: Vec<String> = self.pages.borrow().iter().filter(|(_, n, _)| app.bottom.child_by_name(n).is_some()).map(|(_, n, _)| n.to_string()).collect();
        format!(
            "state: {state}\nstatus: {}\nbreakpoints: {marks:?}\nlocals: {:?}\nwatch: {:?}\nframes: {}\ndebug pages: {pages:?}\ndebug toolbar: {}",
            app.status.message(),
            self.locals.roots(),
            self.watch.roots(),
            self.stack_frames.borrow().len(),
            app.toolbar.debug_controls.is_visible()
        )
    }

    /// The stack frame the variable windows show.
    pub fn frame(&self) -> Option<i64> {
        self.session.borrow().as_ref().and_then(|s| s.frame)
    }

    // ---- Breakpoints ---------------------------------------------------------

    pub fn breakpoints(&self) -> Vec<Breakpoint> {
        self.breakpoints.borrow().values().flat_map(|m| m.values().cloned()).collect()
    }

    pub fn load_breakpoints(&self, list: &[Breakpoint]) {
        let mut map: BTreeMap<PathBuf, BTreeMap<u32, Breakpoint>> = BTreeMap::new();
        for b in list {
            let file = normalize(&b.file);
            map.entry(file.clone()).or_default().insert(b.line, Breakpoint { file, ..b.clone() });
        }
        *self.breakpoints.borrow_mut() = map;
        self.hits.borrow_mut().clear();
        self.refresh_breakpoint_list();
    }

    fn mark_kind(&self, b: &Breakpoint) -> &'static str {
        let key = (b.file.clone(), b.line);
        if !b.enabled {
            editor::BREAKPOINT_DISABLED
        } else if self.is_active() && self.verified.borrow().get(&key) == Some(&false) {
            editor::BREAKPOINT_PENDING
        } else if b.condition.is_some() || b.hit_count.is_some_and(|n| n > 0) {
            editor::BREAKPOINT_CONDITION
        } else {
            editor::BREAKPOINT
        }
    }

    pub fn apply_marks(&self, doc: &CodeDoc) {
        let marks: Vec<(u32, &'static str)> = self.breakpoints.borrow().get(&doc.path()).map(|m| m.values().map(|b| (b.line, self.mark_kind(b))).collect()).unwrap_or_default();
        doc.set_breakpoint_marks(&marks);
    }

    fn apply_all_marks(&self) {
        for d in crate::ui::window::app().tabs.code_docs() {
            self.apply_marks(&d);
        }
    }

    pub fn has_breakpoints(&self, file: &Path) -> bool {
        self.breakpoints.borrow().get(file).is_some_and(|m| !m.is_empty())
    }

    /// After an edit: the marks moved with the text, so the breakpoints
    /// take their lines from them (VS keeps breakpoints on their code).
    pub fn track(&self, doc: &CodeDoc) {
        let file = doc.path();
        let lines = doc.breakpoint_mark_lines();
        let moved = {
            let mut map = self.breakpoints.borrow_mut();
            let Some(old) = map.get_mut(&file) else { return };
            if lines.len() != old.len() || old.keys().copied().eq(lines.iter().copied()) {
                false
            } else {
                let mut next = BTreeMap::new();
                for (mut b, line) in std::mem::take(old).into_values().zip(lines) {
                    b.line = line;
                    next.entry(line).or_insert(b);
                }
                *old = next;
                true
            }
        };
        if moved {
            self.refresh_breakpoint_list();
            self.sync_file(&file);
        }
    }

    pub fn toggle_breakpoint(&self, file: &Path, line: u32) {
        // netcoredbg binds only the exact path the compiler saw.
        let file = &normalize(file);
        {
            let mut map = self.breakpoints.borrow_mut();
            let entry = map.entry(file.to_owned()).or_default();
            if entry.remove(&line).is_none() {
                entry.insert(line, Breakpoint { file: file.to_owned(), line, enabled: true, ..Default::default() });
            }
        }
        self.changed(file);
    }

    fn remove_breakpoint(&self, file: &Path, line: u32) {
        if let Some(m) = self.breakpoints.borrow_mut().get_mut(file) {
            m.remove(&line);
        }
        self.changed(file);
    }

    /// Marks, list, adapter and session file after a breakpoint change.
    fn changed(&self, file: &Path) {
        if let Some(d) = crate::ui::window::app().tabs.code_doc(file) {
            self.apply_marks(&d);
        }
        self.refresh_breakpoint_list();
        self.sync_file(file);
        window::save_session();
    }

    pub fn clear_breakpoints(&self) {
        let files: Vec<PathBuf> = self.breakpoints.borrow().keys().cloned().collect();
        self.breakpoints.borrow_mut().clear();
        for f in files {
            if let Some(d) = crate::ui::window::app().tabs.code_doc(&f) {
                self.apply_marks(&d);
            }
            self.sync_file(&f);
        }
        self.refresh_breakpoint_list();
        window::save_session();
    }

    fn source_breakpoints(&self, file: &Path) -> Vec<SourceBreakpoint> {
        let temp = self.session.borrow().as_ref().and_then(|s| s.temp_line.clone());
        let mut v: Vec<SourceBreakpoint> = self
            .breakpoints
            .borrow()
            .get(file)
            .map(|m| m.values().filter(|b| b.enabled).map(|b| SourceBreakpoint { line: b.line as i64, condition: b.condition.clone() }).collect())
            .unwrap_or_default();
        if let Some((f, l)) = temp
            && f == file
            && !v.iter().any(|b| b.line == l as i64)
        {
            v.push(SourceBreakpoint { line: l as i64, condition: None });
        }
        v
    }

    fn sync_file(&self, file: &Path) {
        let Some(client) = self.session.borrow().as_ref().map(|s| s.client.clone()) else { return };
        let file = file.to_owned();
        glib::spawn_future_local(async move {
            send_breakpoints(&client, &file).await;
        });
    }

    fn update(&self, file: &Path, line: u32, f: impl FnOnce(&mut Breakpoint)) {
        if let Some(bp) = self.breakpoints.borrow_mut().get_mut(file).and_then(|m| m.get_mut(&line)) {
            f(bp);
        }
        if let Some(d) = crate::ui::window::app().tabs.code_doc(file) {
            self.apply_marks(&d);
        }
        self.sync_file(file);
        window::save_session();
    }

    /// The Breakpoints window: one row per breakpoint with VS' columns.
    fn refresh_breakpoint_list(&self) {
        while let Some(c) = self.bp_list.first_child() {
            self.bp_list.remove(&c);
        }
        for b in self.breakpoints() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("bp-row");
            let enabled = gtk::CheckButton::new();
            enabled.set_active(b.enabled);
            enabled.set_tooltip_text(Some(&tr("Enabled")));
            let label = gtk::Label::builder().label(trf("{}, line {}", &[&file_name(&b.file), &b.line.to_string()])).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Middle).build();
            label.set_tooltip_text(Some(&b.file.to_string_lossy()));
            let cond = gtk::Entry::builder().placeholder_text(tr("Condition, e.g. i == 3")).text(b.condition.clone().unwrap_or_default()).width_chars(20).max_width_chars(20).css_classes(["mono"]).build();
            let modes = [tr("Always"), tr("Hit count ="), tr("Hit count ≥"), tr("Multiple of")];
            let mode_refs: Vec<&str> = modes.iter().map(String::as_str).collect();
            let mode = gtk::DropDown::from_strings(&mode_refs);
            mode.set_size_request(150, -1);
            mode.set_selected(match (b.hit_count, b.hit_mode) {
                (None | Some(0), _) => 0,
                (_, HitMode::Equal) => 1,
                (_, HitMode::AtLeast) => 2,
                (_, HitMode::MultipleOf) => 3,
            });
            let count = gtk::Entry::builder().text(b.hit_count.filter(|n| *n > 0).map(|n| n.to_string()).unwrap_or_default()).width_chars(4).max_width_chars(4).input_purpose(gtk::InputPurpose::Digits).build();
            count.set_sensitive(mode.selected() != 0);
            let hits = self.hits.borrow().get(&(b.file.clone(), b.line)).copied().unwrap_or(0);
            let hit_label = gtk::Label::builder().label(trf("hit {}×", &[&hits.to_string()])).width_chars(8).xalign(1.0).css_classes(["dim", "tnum"]).build();
            hit_label.set_tooltip_text(Some(&tr("Times reached in this debugging session")));
            let delete = gtk::Button::from_icon_name("user-trash-symbolic");
            delete.add_css_class("flat");
            delete.set_tooltip_text(Some(&tr("Delete Breakpoint")));
            for w in [enabled.upcast_ref::<gtk::Widget>(), label.upcast_ref(), cond.upcast_ref(), mode.upcast_ref(), count.upcast_ref(), hit_label.upcast_ref(), delete.upcast_ref()] {
                row.append(w);
            }
            let (f, l) = (b.file.clone(), b.line);
            enabled.connect_toggled(glib::clone!(
                #[strong]
                f,
                move |c| {
                    let on = c.is_active();
                    crate::ui::window::app().debugger.update(&f, l, |b| b.enabled = on);
                }
            ));
            // Applied on Enter and when the field loses focus.
            let apply_cond = glib::clone!(
                #[strong]
                f,
                move |text: String| crate::ui::window::app().debugger.update(&f, l, |b| b.condition = Some(text.trim().to_owned()).filter(|c| !c.is_empty()))
            );
            on_commit(&cond, apply_cond);
            let apply_hits = glib::clone!(
                #[strong]
                f,
                #[weak]
                mode,
                move |text: String| {
                    let n: Option<u32> = text.trim().parse().ok().filter(|n| *n > 0);
                    let m = match mode.selected() {
                        2 => HitMode::AtLeast,
                        3 => HitMode::MultipleOf,
                        _ => HitMode::Equal,
                    };
                    let off = mode.selected() == 0;
                    crate::ui::window::app().debugger.update(&f, l, |b| {
                        b.hit_count = if off { None } else { n };
                        b.hit_mode = m;
                    });
                }
            );
            let a2 = apply_hits.clone();
            on_commit(&count, apply_hits);
            mode.connect_selected_notify(glib::clone!(
                #[weak]
                count,
                move |m| {
                    count.set_sensitive(m.selected() != 0);
                    if m.selected() != 0 && count.text().is_empty() {
                        count.set_text("1");
                    }
                    a2(count.text().to_string());
                }
            ));
            delete.connect_clicked(glib::clone!(
                #[strong]
                f,
                move |_| {
                    let f = f.clone();
                    // Not while the row's own signal runs.
                    glib::idle_add_local_once(move || crate::ui::window::app().debugger.remove_breakpoint(&f, l));
                }
            ));
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(glib::clone!(
                #[strong]
                f,
                move |_, key, _, _| {
                    if key != gtk::gdk::Key::Delete {
                        return glib::Propagation::Proceed;
                    }
                    let f = f.clone();
                    glib::idle_add_local_once(move || crate::ui::window::app().debugger.remove_breakpoint(&f, l));
                    glib::Propagation::Stop
                }
            ));
            let list_row = gtk::ListBoxRow::builder().child(&row).build();
            list_row.add_controller(keys);
            self.bp_list.append(&list_row);
        }
        if self.breakpoints.borrow().values().all(BTreeMap::is_empty) {
            let hint = gtk::Label::builder().label(tr("No breakpoints. Click left of a line number or press F9 to set one.")).css_classes(["dim"]).margin_top(18).build();
            let row = gtk::ListBoxRow::builder().child(&hint).activatable(false).selectable(false).build();
            self.bp_list.append(&row);
        }
    }

    // ---- Session ---------------------------------------------------------------

    pub fn start_or_continue(&self) {
        self.start(false);
    }

    fn start(&self, entry: bool) {
        let cont = self.session.borrow().as_ref().filter(|s| s.stopped).map(|s| (s.client.clone(), s.thread));
        if let Some((c, Some(t))) = cont {
            self.resume();
            // As VS brings the console to the front: the program may be
            // waiting for input.
            window::show_bottom("terminal");
            crate::ui::window::app().terminal.focus();
            glib::spawn_future_local(async move {
                let _ = c.continue_(t).await;
            });
            return;
        }
        let app = crate::ui::window::app();
        if self.is_active() {
            return;
        }
        if self.starting.get() || self.downloading.get() {
            app.status.flash(&tr("The debugger is starting…"));
            return;
        }
        let bin = launch::binary(&host::home(), launch::NETCOREDBG_VERSION);
        if !bin.exists() {
            if app.settings.boolean("netcoredbg-consent") {
                download_netcoredbg(entry);
            } else {
                ask_netcoredbg_consent(entry);
            }
            return;
        }
        start_session(bin, entry);
    }

    fn resume(&self) {
        if let Some(s) = self.session.borrow_mut().as_mut() {
            s.stopped = false;
            s.current = None;
        }
        for d in crate::ui::window::app().tabs.code_docs() {
            d.set_debug_line(None);
        }
        crate::ui::window::app().status.set_message(&tr("Running"));
        self.update_controls();
    }

    pub fn step(&self, kind: &'static str) {
        let s = self.session.borrow().as_ref().filter(|s| s.stopped).map(|s| (s.client.clone(), s.thread));
        if let Some((c, Some(t))) = s {
            self.resume();
            glib::spawn_future_local(async move {
                let _ = match kind {
                    "next" => c.next(t).await,
                    "stepIn" => c.step_in(t).await,
                    _ => c.step_out(t).await,
                };
            });
        } else if !self.is_active() && kind != "stepOut" {
            // F10/F11 without a session start debugging and stop at the
            // start of Main, as VS does.
            self.start(true);
        }
    }

    pub fn pause(&self) {
        let s = self.session.borrow().as_ref().map(|s| (s.client.clone(), s.thread.unwrap_or(0)));
        if let Some((c, t)) = s {
            glib::spawn_future_local(async move {
                let t = if t == 0 { c.threads().await.ok().and_then(|v| v.first().map(|t| t.id)).unwrap_or(0) } else { t };
                let _ = c.pause(t).await;
            });
        }
    }

    pub fn stop(&self) {
        if !self.is_active() && self.starting.get() {
            let app = crate::ui::window::app();
            self.cancel_start.set(true);
            app.builder.cancel();
            app.terminal.kill();
            self.set_starting(false);
            app.status.set_message(&tr("Debugging canceled"));
            return;
        }
        let s = self.session.borrow().as_ref().map(|s| (s.client.clone(), s.id));
        if let Some((c, id)) = s {
            glib::spawn_future_local(async move {
                let _ = crate::runtime::run(async move { tokio::time::timeout(Duration::from_secs(3), c.disconnect(true)).await }).await;
                end_session(id);
            });
        }
    }

    pub fn restart(&self) {
        self.stop();
        // Start again once the old session has ended.
        glib::timeout_add_local(Duration::from_millis(100), || {
            let app = crate::ui::window::app();
            if app.debugger.is_active() {
                return glib::ControlFlow::Continue;
            }
            app.debugger.start_or_continue();
            glib::ControlFlow::Break
        });
    }

    pub fn run_to_cursor(&self) {
        let Some(doc) = crate::ui::window::app().tabs.current_code() else { return };
        let line = doc.buffer.iter_at_mark(&doc.buffer.get_insert()).line() as u32 + 1;
        let target = (normalize(&doc.path()), line);
        if let Some(s) = self.session.borrow_mut().as_mut() {
            s.temp_line = Some(target.clone());
        } else {
            *self.pending_temp.borrow_mut() = Some(target);
        }
        self.sync_file(&doc.path());
        self.start_or_continue();
    }

    pub fn quick_watch(&self) {
        let app = crate::ui::window::app();
        let initial = app.tabs.current_code().map(|d| d.selection_text()).unwrap_or_default();
        let d = adw::AlertDialog::new(Some(&tr("QuickWatch")), None);
        let entry = gtk::Entry::builder().text(initial.as_str()).activates_default(true).css_classes(["mono"]).build();
        let tree = VarTree::new();
        tree.widget.set_size_request(520, 220);
        let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
        b.append(&entry);
        b.append(&tree.widget);
        d.set_extra_child(Some(&b));
        d.add_responses(&[("close", &tr("Close")), ("add", &tr("Add Watch"))]);
        let tree = Rc::new(tree);
        let eval = glib::clone!(
            #[weak]
            entry,
            #[strong]
            tree,
            move || {
                let expr = entry.text().to_string();
                let tree = tree.clone();
                glib::spawn_future_local(async move {
                    let app = crate::ui::window::app();
                    let s = app.debugger.session.borrow().as_ref().filter(|s| s.stopped).map(|s| (s.client.clone(), s.frame));
                    let rows = match s {
                        Some((c, f)) => match c.evaluate(&expr, f, "watch").await {
                            Ok(v) => vartree::items(&c, f, vec![v], &Rc::default()).await,
                            Err(e) => vec![Item::plain(&expr, &e.to_string())],
                        },
                        None => vec![Item::plain(&expr, &tr("The debugger is not stopped."))],
                    };
                    tree.set(rows);
                });
            }
        );
        eval();
        entry.connect_activate(move |_| eval());
        d.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
            if r == "add" {
                crate::ui::window::app().session.borrow_mut().watches.push(entry.text().to_string());
                glib::spawn_future_local(refresh_watch());
            }
        });
    }

    pub fn exception_settings(&self) {
        let app = crate::ui::window::app();
        let d = adw::AlertDialog::new(Some(&tr("Exception Settings")), None);
        let thrown = gtk::CheckButton::with_label(&tr("Break when an exception is thrown (Common Language Runtime Exceptions)"));
        thrown.set_active(self.break_on_thrown.get());
        d.set_extra_child(Some(&thrown));
        d.add_responses(&[("cancel", &tr("Cancel")), ("ok", &tr("OK"))]);
        d.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
            if r != "ok" {
                return;
            }
            let app = crate::ui::window::app();
            app.debugger.break_on_thrown.set(thrown.is_active());
            let c = app.debugger.session.borrow().as_ref().map(|s| s.client.clone());
            if let Some(c) = c {
                let on = thrown.is_active();
                glib::spawn_future_local(async move {
                    let _ = c.set_exception_breakpoints(on).await;
                });
            }
        });
    }

    /// The DataTip for the expression under the pointer while stopped:
    /// the same Name/Value/Type tree as Locals, expandable.
    pub async fn data_tip(&self, doc: &CodeDoc, byte: usize) -> Option<gtk::Widget> {
        let (c, f) = self.session.borrow().as_ref().filter(|s| s.stopped).map(|s| (s.client.clone(), s.frame))?;
        let text = doc.text();
        let expr = hover_expression(&text, byte)?;
        let v = c.evaluate(&expr, f, "hover").await.ok()?;
        let rows = vartree::items(&c, f, vec![v], &self.snapshot).await;
        let tree = VarTree::new();
        tree.set(rows);
        tree.widget.set_size_request(480, -1);
        tree.widget.set_propagate_natural_height(true);
        tree.widget.set_max_content_height(260);
        tree.widget.add_css_class("data-tip");
        Some(tree.widget.upcast())
    }

    /// Attaches to a running .NET process (Debug Test: testhost waits).
    pub fn attach(&self, pid: u32) {
        let bin = launch::binary(&host::home(), launch::NETCOREDBG_VERSION);
        if !bin.exists() {
            window::toast(&tr("netcoredbg is not installed yet; start debugging once with F5."));
            return;
        }
        if self.is_active() || self.starting.get() {
            window::toast(&tr("A debugging session is already running."));
            return;
        }
        let id = self.next_session_id();
        glib::spawn_future_local(async move {
            if let Err(e) = attach_adapter(bin, pid, false, id).await {
                fail(&tr("The debugger could not attach"), &e);
                end_session(id);
            }
        });
    }

    fn next_session_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    fn set_starting(&self, on: bool) {
        self.starting.set(on);
        if on {
            self.cancel_start.set(false);
            crate::ui::jobs::start("debug", &tr("Starting the debugger…"));
        } else {
            crate::ui::jobs::finish("debug");
        }
        crate::ui::window::app().toolbar.set_busy(on);
        self.update_controls();
    }

    fn canceled(&self) -> bool {
        self.cancel_start.get()
    }

    /// Greys out what cannot run now (see [`command_states`]).
    pub fn update_controls(&self) {
        let app = crate::ui::window::app();
        for (name, on) in command_states(self.is_active(), self.is_stopped(), self.is_starting()) {
            if let Some(a) = app.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                a.set_enabled(on);
            }
        }
    }
}

/// Which debug commands can act, as in VS: Continue and the steps only
/// while stopped (F5, F10 and F11 also start a session), Break All only
/// while running, Stop while a session runs or starts (it cancels the
/// start), Restart while a session runs.
fn command_states(active: bool, stopped: bool, starting: bool) -> [(&'static str, bool); 10] {
    let can_go = !starting && (!active || stopped);
    [
        ("debug.start", can_go),
        ("debug.start-without", !active && !starting),
        ("debug.break-all", active && !stopped),
        ("debug.stop", active || starting),
        ("debug.restart", active),
        ("debug.step-over", can_go),
        ("debug.step-into", can_go),
        ("debug.step-out", stopped),
        ("debug.run-to-cursor", can_go),
        ("debug.quick-watch", stopped),
    ]
}

/// Calls `apply` with the entry's text on Enter and when focus leaves it.
fn on_commit(entry: &gtk::Entry, apply: impl Fn(String) + Clone + 'static) {
    let a = apply.clone();
    entry.connect_activate(move |e| a(e.text().to_string()));
    let focus = gtk::EventControllerFocus::new();
    let e = entry.downgrade();
    focus.connect_leave(move |_| {
        if let Some(e) = e.upgrade() {
            apply(e.text().to_string());
        }
    });
    entry.add_controller(focus);
}

fn breakpoint_header() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("bp-header");
    for (text, expand, width) in [(tr("Name"), true, 0), (tr("Condition"), false, 22), (tr("Hit Count"), false, 22), (String::new(), false, 10)] {
        let l = gtk::Label::builder().label(text).xalign(0.0).hexpand(expand).build();
        if width > 0 {
            l.set_width_chars(width);
        }
        row.append(&l);
    }
    row
}

fn mark_current_frame(list: &gtk::ListBox, index: i32) {
    let mut i = 0;
    while let Some(row) = list.row_at_index(i) {
        if i == index {
            row.add_css_class("current-frame");
        } else {
            row.remove_css_class("current-frame");
        }
        i += 1;
    }
}

/// The expression under the pointer: the identifier with the member
/// accesses to its left (`person.Name` when pointing at `Name`).
fn hover_expression(text: &str, byte: usize) -> Option<String> {
    let byte = byte.min(text.len());
    let is_word = |ch: char| ch.is_alphanumeric() || ch == '_' || ch == '.';
    let start = text[..byte].rfind(|ch: char| !is_word(ch)).map_or(0, |i| i + 1);
    let end = text[byte..].find(|ch: char| !is_word(ch) || ch == '.').map_or(text.len(), |i| byte + i);
    let expr = text.get(start..end)?.trim_matches('.').to_owned();
    (!expr.is_empty() && !expr.starts_with(|c: char| c.is_ascii_digit())).then_some(expr)
}

fn append(view: &gtk::TextView, text: &str) {
    let b = view.buffer();
    let mut end = b.end_iter();
    b.insert(&mut end, text);
}

fn ask_netcoredbg_consent(entry: bool) {
    let app = crate::ui::window::app();
    let d = adw::AlertDialog::new(
        Some(&tr("Download the debugger?")),
        Some(&trf("Debugging uses Samsung's netcoredbg {} (MIT licence, about 4 MB). Anvil downloads it once from GitHub into ~/.local/share/anvil/netcoredbg. Microsoft's own debugger may only be used in Microsoft products.", &[launch::NETCOREDBG_VERSION])),
    );
    d.add_responses(&[("later", &tr("Not Now")), ("download", &tr("Download"))]);
    d.set_default_response(Some("download"));
    d.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
        if r == "download" {
            let _ = crate::ui::window::app().settings.set_boolean("netcoredbg-consent", true);
            download_netcoredbg(entry);
        }
    });
}

/// Downloads and unpacks netcoredbg next to a `.partial` folder that is
/// renamed into place, so a broken download never counts as installed.
fn download_netcoredbg(then_entry: bool) {
    let app = crate::ui::window::app();
    if app.debugger.downloading.replace(true) {
        return;
    }
    app.status.set_message(&tr("Downloading netcoredbg…"));
    app.toolbar.set_busy(true);
    let home = host::home();
    let dir = launch::install_dir(&home, launch::NETCOREDBG_VERSION);
    glib::spawn_future_local(async move {
        let bytes = crate::runtime::run(async move {
            let r = reqwest::get(launch::download_url(launch::NETCOREDBG_VERSION)).await.map_err(|e| e.to_string())?;
            r.error_for_status().map_err(|e| e.to_string())?.bytes().await.map_err(|e| e.to_string())
        })
        .await;
        let partial = dir.with_extension("partial");
        let result = async {
            let b = bytes?;
            let _ = std::fs::remove_dir_all(&partial);
            std::fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
            let tar = partial.join("netcoredbg.tar.gz");
            std::fs::write(&tar, &b).map_err(|e| e.to_string())?;
            let out = host::run(Command::new("tar").args(["-xzf", "netcoredbg.tar.gz"]).cwd(&partial), None).await.map_err(|e| e.to_string())?;
            if !out.ok() {
                return Err(out.stderr_text());
            }
            let _ = std::fs::remove_file(&tar);
            if !partial.join("netcoredbg/netcoredbg").exists() {
                return Err(tr("The download did not contain netcoredbg/netcoredbg."));
            }
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::rename(&partial, &dir).map_err(|e| e.to_string())
        }
        .await;
        let app = crate::ui::window::app();
        app.debugger.downloading.set(false);
        app.toolbar.set_busy(false);
        match result {
            Ok(()) => {
                app.status.set_message("");
                app.debugger.start(then_entry);
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&partial);
                app.status.set_message("");
                fail(&tr("netcoredbg could not be downloaded"), &e);
            }
        }
    });
}

/// Builds the startup hook once (it waits for the debugger, see
/// `anvil_dap::launch`) with the user's own .NET SDK.
async fn ensure_hook() -> Result<PathBuf, String> {
    let home = host::home();
    let dll = launch::hook_dll(&home);
    if dll.exists() {
        return Ok(dll);
    }
    let dir = launch::hook_dir(&home);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("AnvilStartupHook.csproj"), launch::HOOK_PROJECT).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("StartupHook.cs"), launch::HOOK_SOURCE).map_err(|e| e.to_string())?;
    crate::ui::window::app().status.set_message(&tr("Preparing the debugger (once)…"));
    let out = host::run(Command::dotnet().args(["build", "-c", "Debug", "-o", "out", "-nologo"]).cwd(&dir), None).await.map_err(|e| e.to_string())?;
    if !out.ok() || !dll.exists() {
        return Err(format!("{}\n{}", tr("The debugger's startup hook could not be built."), out.stdout_text()));
    }
    Ok(dll)
}

fn start_session(bin: PathBuf, entry: bool) {
    let app = crate::ui::window::app();
    let Some(name) = window::startup_project_name() else {
        app.status.flash(&tr("Open a solution first."));
        return;
    };
    let Some(p) = app.solution.borrow().as_ref().and_then(|s| s.project_named(&name).cloned()) else {
        app.status.flash(&tr("The startup project is not in the solution."));
        return;
    };
    app.debugger.set_starting(true);
    crate::ui::build::build_then(crate::ui::build::Target::Project(name.clone()), crate::ui::build::Verb::Build, move |ok| {
        let app = crate::ui::window::app();
        if !ok || app.debugger.canceled() {
            app.debugger.set_starting(false);
            return;
        }
        glib::spawn_future_local(async move {
            let app = crate::ui::window::app();
            let id = app.debugger.next_session_id();
            let result = launch_program(bin, p, entry, id).await;
            let app = crate::ui::window::app();
            app.debugger.set_starting(false);
            if let Err(e) = result {
                end_session(id);
                if app.debugger.canceled() {
                    app.terminal.kill();
                    app.status.set_message(&tr("Debugging canceled"));
                } else {
                    fail(&tr("The debugger did not start"), &e);
                }
            }
        });
    });
}

/// Runs the program in the terminal with the waiting hook, then attaches.
async fn launch_program(bin: PathBuf, p: anvil_core::solution::Project, entry: bool, id: u64) -> Result<(), String> {
    let app = crate::ui::window::app();
    let home = host::home();
    let hook = ensure_hook().await?;
    let dll = crate::ui::build::built_program(&p).await?;
    if !dll.exists() {
        return Err(trf("The built program was not found at {}.", &[&dll.to_string_lossy()]));
    }
    let token = format!("{}-{id}", std::process::id());
    let pid_file = launch::pid_file(&home, &token);
    let exit_file = pid_file.with_extension("exit");
    if let Some(dir) = pid_file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(&pid_file);
    let _ = std::fs::remove_file(&exit_file);
    let mut cmd = Command::new("dotnet").arg(dll.to_string_lossy()).args(crate::ui::build::launch_args(&p));
    for (k, v) in crate::ui::build::launch_env(&p).into_iter().chain(launch::program_env(&hook, &pid_file)) {
        cmd = cmd.env(k, v);
    }
    // VS runs the program in its output folder.
    let cwd = dll.parent().map(Path::to_path_buf).unwrap_or_else(|| p.dir.clone());
    window::show_bottom("terminal");
    app.status.set_message(&trf("Starting {}…", &[&p.name]));
    app.terminal.run_program_noting_exit(&cwd, &cmd, Some(&exit_file), None);
    // The hook writes the process id, then waits for the debugger.
    let mut waited = Duration::ZERO;
    let pid: u32 = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|t| t.trim().parse().ok()) {
            break pid;
        }
        if app.debugger.canceled() {
            return Err(tr("Debugging canceled"));
        }
        if exit_file.exists() {
            return Err(tr("The program ended before the debugger could attach. The terminal panel shows its output."));
        }
        if waited > Duration::from_secs(30) {
            return Err(tr("The program did not start within 30 seconds. The terminal panel shows its output."));
        }
        glib::timeout_future(Duration::from_millis(20)).await;
        waited += Duration::from_millis(20);
    };
    let _ = std::fs::remove_file(&pid_file);
    app.status.set_message(&tr("Attaching the debugger…"));
    attach_adapter(bin, pid, entry, id).await?;
    // The terminal reports the end of the program too (if the debugger
    // went away first).
    glib::spawn_future_local(async move {
        while crate::ui::window::app().debugger.session.borrow().as_ref().is_some_and(|s| s.id == id) {
            if exit_file.exists() {
                glib::timeout_future(Duration::from_millis(500)).await;
                end_session(id);
                break;
            }
            glib::timeout_future(Duration::from_millis(250)).await;
        }
        let _ = std::fs::remove_file(&exit_file);
    });
    Ok(())
}

/// Starts netcoredbg over stdin/stdout, attaches to `pid`, sends the
/// breakpoints and runs the session's event loop.
async fn attach_adapter(bin: PathBuf, pid: u32, entry: bool, id: u64) -> Result<(), String> {
    let cmd = Command::new(bin.to_string_lossy()).args(launch::adapter_args());
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let (client, rx) = crate::runtime::run(async move {
        let mut c = host::tokio_command(&cmd);
        c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        c.kill_on_drop(false);
        let mut child = c.spawn().map_err(|e| e.to_string())?;
        let (i, o) = (child.stdin.take().ok_or("stdin")?, child.stdout.take().ok_or("stdout")?);
        // netcoredbg leaves after `disconnect`; if it has not two seconds
        // after the session ended, it is killed.
        tokio::spawn(async move {
            tokio::select! {
                _ = child.wait() => {}
                _ = stop_rx => {
                    if tokio::time::timeout(Duration::from_secs(2), child.wait()).await.is_err() {
                        let _ = child.kill().await;
                    }
                }
            }
        });
        Ok::<_, String>(Client::start(o, i))
    })
    .await?;
    let timeout = |what: &'static str, c: Client| async move {
        let r = crate::runtime::run(async move {
            let fut = async {
                match what {
                    "initialize" => c.initialize().await.map(|_| ()),
                    _ => c.request("attach", launch::attach_args(pid)).await.map(|_| ()),
                }
            };
            tokio::time::timeout(Duration::from_secs(15), fut).await
        })
        .await;
        match r {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(format!("{what}: {e}")),
            Err(_) => Err(trf("netcoredbg did not answer {} within 15 seconds.", &[what])),
        }
    };
    let app = crate::ui::window::app();
    *app.debugger.session.borrow_mut() = Some(Session { id, client: client.clone(), stop_adapter: Some(stop_tx), thread: None, frame: None, stopped: false, temp_line: app.debugger.pending_temp.borrow_mut().take(), current: None, entry_stop: entry });
    app.debugger.hits.borrow_mut().clear();
    app.debugger.verified.borrow_mut().clear();
    app.debugger.bp_ids.borrow_mut().clear();
    timeout("initialize", client.clone()).await?;
    timeout("attach", client.clone()).await?;
    // Configuration: breakpoints, entry stop, exceptions, then run.
    let files: Vec<PathBuf> = app.debugger.breakpoints.borrow().keys().cloned().collect();
    let temp = app.debugger.session.borrow().as_ref().and_then(|s| s.temp_line.as_ref().map(|t| t.0.clone()));
    for f in files.iter().cloned().chain(temp.filter(|t| !files.contains(t))) {
        send_breakpoints(&client, &f).await;
    }
    if entry {
        let _ = client.set_function_breakpoints(&launch::ENTRY_FUNCTIONS).await;
    }
    let _ = client.set_exception_breakpoints(app.debugger.break_on_thrown.get()).await;
    let _ = client.configuration_done().await;
    app.debugger.show_pages(true);
    app.toolbar.debug_controls.set_visible(true);
    app.toolbar.start_label.set_label(&tr("Continue"));
    app.status.set_message(&tr("Running"));
    app.debugger.refresh_breakpoint_list();
    app.debugger.update_controls();
    glib::spawn_future_local(event_loop(client, rx, id));
    Ok(())
}

/// Sends one file's breakpoints and records which ones the debugger bound.
async fn send_breakpoints(client: &Client, file: &Path) {
    let app = crate::ui::window::app();
    let bps = app.debugger.source_breakpoints(file);
    let Ok(states) = client.set_breakpoints(&file.to_string_lossy(), &bps).await else { return };
    for (b, s) in bps.iter().zip(states) {
        let key = (file.to_owned(), b.line as u32);
        if let Some(id) = s.id {
            app.debugger.bp_ids.borrow_mut().insert(id, key.clone());
        }
        app.debugger.verified.borrow_mut().insert(key, s.verified);
    }
    if let Some(d) = app.tabs.code_doc(file) {
        app.debugger.apply_marks(&d);
    }
}

async fn event_loop(client: Client, mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>, id: u64) {
    let app = crate::ui::window::app();
    while let Some(ev) = rx.recv().await {
        if !app.debugger.session.borrow().as_ref().is_some_and(|s| s.id == id) {
            break;
        }
        match ev {
            Event::Initialized => {}
            Event::Stopped { reason, thread_id, text } => on_stopped(&client, thread_id, &reason, text).await,
            Event::Continued => app.debugger.resume(),
            Event::Output { category, output } => match category.as_str() {
                // The program writes to the terminal itself; anything that
                // still arrives here belongs there too.
                "stdout" | "stderr" => app.terminal.feed_text(&output),
                _ => app.output.append("Debug", &output),
            },
            Event::Breakpoint(state) => {
                let key = state.id.and_then(|i| app.debugger.bp_ids.borrow().get(&i).cloned());
                if let Some(key) = key {
                    let file = key.0.clone();
                    app.debugger.verified.borrow_mut().insert(key, state.verified);
                    if let Some(d) = app.tabs.code_doc(&file) {
                        app.debugger.apply_marks(&d);
                    }
                }
            }
            Event::Exited { code } => app.output.append("Debug", &trf("The program has exited with code {}.\n", &[&code.to_string()])),
            Event::Terminated | Event::Closed => break,
            Event::Other { .. } => {}
        }
    }
    // Let netcoredbg leave; it exits after `disconnect`.
    let _ = crate::runtime::run(async move { tokio::time::timeout(Duration::from_secs(2), client.disconnect(false)).await }).await;
    end_session(id);
}

async fn on_stopped(client: &Client, thread: Option<i64>, reason: &str, text: Option<String>) {
    let app = crate::ui::window::app();
    let thread = match thread {
        Some(t) => Some(t),
        None => client.threads().await.ok().and_then(|t| t.first().map(|t| t.id)),
    };
    let frames = match thread {
        Some(t) => client.stack_trace(t).await.unwrap_or_default(),
        None => Vec::new(),
    };
    // The first frame with source is where the user is.
    let top = frames.iter().find(|f| f.source.as_ref().and_then(|s| s.path.as_ref()).is_some()).cloned();
    let location = top.as_ref().and_then(|f| f.source.as_ref().and_then(|s| s.path.clone()).map(|p| (normalize(Path::new(&p)), f.line as u32)));

    // The first stop of an F10/F11 start: the entry breakpoints go.
    let entry = app.debugger.session.borrow_mut().as_mut().map(|s| std::mem::replace(&mut s.entry_stop, false)).unwrap_or(false);
    if entry {
        let _ = client.set_function_breakpoints(&[]).await;
    }
    // Hit counts: count every arrival, stop only when the condition holds.
    if reason == "breakpoint"
        && !entry
        && let Some(loc) = &location
    {
        let bp = app.debugger.breakpoints.borrow().get(&loc.0).and_then(|m| m.get(&loc.1)).cloned();
        let is_temp = app.debugger.session.borrow().as_ref().and_then(|s| s.temp_line.clone()).as_ref() == Some(loc);
        if let Some(bp) = bp {
            let hits = {
                let mut h = app.debugger.hits.borrow_mut();
                let n = h.entry(loc.clone()).or_insert(0);
                *n += 1;
                *n
            };
            app.debugger.refresh_breakpoint_list();
            if !bp.should_break(hits)
                && !is_temp
                && let Some(t) = thread
            {
                let _ = client.continue_(t).await;
                return;
            }
        }
    }

    let temp = if let Some(s) = app.debugger.session.borrow_mut().as_mut() {
        s.thread = thread;
        s.frame = top.as_ref().map(|f| f.id);
        s.stopped = true;
        s.temp_line.take()
    } else {
        return;
    };
    app.debugger.update_controls();
    if let Some(t) = temp {
        app.debugger.sync_file(&t.0);
    }
    *app.debugger.stack_frames.borrow_mut() = frames.clone();
    let stack = &app.debugger.stack;
    while let Some(c) = stack.first_child() {
        stack.remove(&c);
    }
    for f in &frames {
        let loc = f.source.as_ref().and_then(|s| s.name.clone()).map(|n| trf("{}, line {}", &[&n, &f.line.to_string()])).unwrap_or_default();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.append(&gtk::Label::builder().label(&f.name).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["mono"]).build());
        row.append(&gtk::Label::builder().label(loc).css_classes(["dim"]).build());
        stack.append(&row);
    }
    let top_index = frames.iter().position(|f| Some(f) == top.as_ref()).unwrap_or(0) as i32;
    mark_current_frame(stack, top_index);
    if let Some((path, line)) = location {
        app.tabs.open(&path, false);
        if let Some(d) = app.tabs.code_doc(&path) {
            d.set_debug_line(Some(line));
        }
        if let Some(s) = app.debugger.session.borrow_mut().as_mut() {
            s.current = Some((path, line));
        }
    }
    let msg = match reason {
        "exception" => trf("Exception thrown: {}", &[&text.unwrap_or_default()]),
        "breakpoint" => tr("Breakpoint hit"),
        "entry" | "function breakpoint" => tr("Paused at the start of the program"),
        "step" => tr("Paused"),
        _ => tr("Paused"),
    };
    app.status.set_message(&msg);
    let shown = app.bottom.visible_child_name();
    if !matches!(shown.as_deref(), Some("locals" | "autos" | "watch" | "call-stack" | "immediate")) {
        window::show_bottom("locals");
    }
    app.window.present();
    refresh_variables().await;
    refresh_watch().await;
}

async fn refresh_variables() {
    let app = crate::ui::window::app();
    let s = app.debugger.session.borrow().as_ref().map(|s| (s.client.clone(), s.frame, s.current.clone()));
    let Some((client, Some(frame), current)) = s else { return };
    let scopes = client.scopes(frame).await.unwrap_or_default();
    let mut vars = Vec::new();
    for sc in scopes {
        vars.extend(client.variables(sc.variables_reference).await.unwrap_or_default());
    }
    let rows = vartree::items(&client, Some(frame), vars, &app.debugger.snapshot).await;
    // Autos: the variables used on the current and the previous line, as
    // VS shows them.
    let names: Vec<String> = current
        .and_then(|(p, l)| {
            let text = app.tabs.code_doc(&p)?.text();
            let lines: Vec<&str> = text.lines().collect();
            let a = lines.get((l as usize).saturating_sub(2)).copied().unwrap_or("");
            let b = lines.get((l as usize).saturating_sub(1)).copied().unwrap_or("");
            Some(format!("{a} {b}").split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| !w.is_empty()).map(str::to_owned).collect())
        })
        .unwrap_or_default();
    let autos: Vec<Item> = rows.iter().filter(|r| names.contains(&r.var.name) || r.var.name == "this").cloned().collect();
    app.debugger.locals.set(rows);
    app.debugger.autos.set(autos);
}

async fn refresh_watch() {
    let app = crate::ui::window::app();
    let watches = app.session.borrow().watches.clone();
    let s = app.debugger.session.borrow().as_ref().filter(|s| s.stopped).map(|s| (s.client.clone(), s.frame));
    let mut rows = Vec::new();
    for w in watches {
        match &s {
            Some((c, f)) => match c.evaluate(&w, *f, "watch").await {
                Ok(v) => rows.extend(vartree::items(c, *f, vec![v], &app.debugger.watch_snapshot).await),
                Err(e) => rows.push(Item::plain(&w, &e.to_string())),
            },
            None => rows.push(Item::plain(&w, "")),
        }
    }
    app.debugger.watch.set(rows);
}

fn end_session(id: u64) {
    let app = crate::ui::window::app();
    let s = {
        let mut cur = app.debugger.session.borrow_mut();
        if !cur.as_ref().is_some_and(|s| s.id == id) {
            return;
        }
        cur.take()
    };
    if let Some(stop) = s.and_then(|mut s| s.stop_adapter.take()) {
        let _ = stop.send(());
    }
    for d in app.tabs.code_docs() {
        d.set_debug_line(None);
    }
    app.debugger.verified.borrow_mut().clear();
    app.debugger.apply_all_marks();
    app.debugger.stack_frames.borrow_mut().clear();
    app.debugger.locals.clear();
    app.debugger.autos.clear();
    app.debugger.show_pages(false);
    app.toolbar.debug_controls.set_visible(false);
    window::refresh_startup_dropdown();
    app.status.set_message(&tr("Debugging stopped"));
    app.debugger.update_controls();
    // Watch keeps its expressions, without the last session's values.
    glib::spawn_future_local(refresh_watch());
}

/// A failed start or download: the reason goes to Output > Debug and the
/// status bar, a toast offers it, nothing blocks the window.
fn fail(title: &str, detail: &str) {
    let app = crate::ui::window::app();
    app.output.append("Debug", &format!("{title}\n{detail}\n"));
    // A short reason is the message itself; a long log stays behind Details.
    let short = detail.trim();
    let text = if !short.is_empty() && short.len() <= 200 && !short.contains('\n') { short } else { title };
    app.status.set_message(text);
    let toast = adw::Toast::new(text);
    toast.set_button_label(Some(&tr("Details")));
    toast.set_timeout(8);
    toast.connect_button_clicked(|_| crate::ui::window::app().output.show("Debug"));
    app.toast.add_toast(toast);
    if crate::shots::requested().is_some() {
        println!("anvil: {title}: {detail}");
    }
}

#[cfg(test)]
mod tests {
    use super::{command_states, hover_expression};

    fn enabled(active: bool, stopped: bool, starting: bool) -> Vec<&'static str> {
        command_states(active, stopped, starting).into_iter().filter(|(_, on)| *on).map(|(n, _)| n).collect()
    }

    #[test]
    fn debug_commands_follow_the_session() {
        assert_eq!(enabled(false, false, false), ["debug.start", "debug.start-without", "debug.step-over", "debug.step-into", "debug.run-to-cursor"]);
        assert_eq!(enabled(false, false, true), ["debug.stop"]);
        assert_eq!(enabled(true, false, false), ["debug.break-all", "debug.stop", "debug.restart"]);
        assert_eq!(enabled(true, true, false), ["debug.start", "debug.stop", "debug.restart", "debug.step-over", "debug.step-into", "debug.step-out", "debug.run-to-cursor", "debug.quick-watch"]);
    }

    #[test]
    fn hover_expressions() {
        let t = "var n = person.Name.Length + count;";
        assert_eq!(hover_expression(t, t.find("Name").unwrap() + 1).as_deref(), Some("person.Name"));
        assert_eq!(hover_expression(t, t.find("count").unwrap()).as_deref(), Some("count"));
        assert_eq!(hover_expression("x = 42;", 5), None);
    }
}
