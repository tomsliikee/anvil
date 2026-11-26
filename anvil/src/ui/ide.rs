//! Roslyn's IDE commands in the editor: Quick Actions (Ctrl+.) with a
//! lightbulb in the gutter and a preview of each fix, Go To Definition and
//! Implementation (into metadata as source too), Peek Definition, Find All
//! References in a panel grouped by file, the `using` a completion item
//! brings, and Format Document / Selection (optionally on save).
//!
//! Every request has a time limit: a server still loading projects ends
//! in a status message, never in silence.

use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use anvil_lsp::client::{Client, Error};
use anvil_lsp::navigation;
use anvil_lsp::position::to_position;
use anvil_lsp::roslyn;
use gtk::{gio, glib};
use lsp_types as lt;
use serde_json::{json, Value};
use sourceview5::prelude::*;

use crate::host;
use crate::i18n::{tr, trf};
use crate::ui::editor::CodeDoc;

pub const LIGHTBULB: &str = "anvil-lightbulb";

/// Seconds until a command gives up: navigation and fixes should feel
/// instant; references search the whole solution.
const QUICK: u64 = 10;
const SEARCH: u64 = 30;

async fn ask(client: &Client, method: &str, params: Value, secs: u64) -> Result<Value, Error> {
    let (id, rx) = client.send_request(method, params);
    match glib::future_with_timeout(Duration::from_secs(secs), rx).await {
        Ok(r) => r.unwrap_or(Err(Error::Closed)),
        Err(_) => {
            client.abandon(id);
            Err(Error::Timeout)
        }
    }
}

fn flash(text: &str) {
    crate::ui::window::app().status.flash(text);
}

fn working(text: &str) {
    crate::ui::window::app().status.set_message(text);
}

fn report(e: &Error) {
    match e {
        Error::Timeout => flash(&tr("The language server did not answer in time; it may still be loading the projects.")),
        Error::Rpc { code, .. } if *code == anvil_lsp::client::REQUEST_CANCELLED => flash(""),
        other => flash(&trf("Language server: {}", &[&other.to_string()])),
    }
}

fn server() -> Option<(Client, Rc<CodeDoc>)> {
    let app = crate::ui::window::app();
    let doc = app.tabs.current_code()?;
    if !doc.is_csharp() {
        flash(&tr("Only C# files have language features."));
        return None;
    }
    match app.lsp.client() {
        Some(c) => Some((c, doc)),
        None => {
            flash(&tr("The language server is not ready yet."));
            None
        }
    }
}

fn uri(p: &Path) -> String {
    roslyn::file_uri(p)
}

fn position_params(doc: &CodeDoc) -> Value {
    let text = doc.text();
    json!({"textDocument": {"uri": uri(&doc.path())}, "position": to_position(&text, doc.cursor_byte())})
}

/// The text of a file as the editor has it (unsaved changes count), or
/// from disk.
fn file_text(path: &Path) -> Option<String> {
    crate::ui::window::app().tabs.code_doc(path).map(|d| d.text()).or_else(|| anvil_core::text::TextFile::read(path).ok().map(|f| f.text))
}

/// A location's file and text: on disk, or a source-generated document
/// that only the server has.
async fn location_text(client: &Client, l: &lt::Location) -> Option<(String, Option<PathBuf>, String)> {
    if let Some(p) = roslyn::path_from_uri(l.uri.as_str()) {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return file_text(&p).map(|t| (name, Some(p), t));
    }
    let v = ask(client, roslyn::SOURCE_GENERATED_TEXT, json!({"textDocument": {"uri": l.uri.as_str()}}), QUICK).await.ok()?;
    let name = l.uri.as_str().rsplit('/').next().unwrap_or("").to_owned();
    v.get("text").and_then(Value::as_str).map(|t| (name, None, t.to_owned()))
}

// ---- Go To Definition / Implementation, Peek ---------------------------

/// F12 (`implementation == false`) and Ctrl+F12.
pub fn goto(implementation: bool) {
    let Some((client, doc)) = server() else { return };
    let params = position_params(&doc);
    let method = if implementation { "textDocument/implementation" } else { "textDocument/definition" };
    working(&if implementation { tr("Finding implementations…") } else { tr("Finding the definition…") });
    glib::spawn_future_local(async move {
        match ask(&client, method, params, QUICK).await {
            Ok(v) => {
                let l = navigation::locations(v);
                match l.len() {
                    0 => flash(&tr("Cannot navigate to the symbol under the caret.")),
                    1 => {
                        flash("");
                        open_location(&client, &doc, &l[0]).await;
                    }
                    _ => {
                        flash("");
                        show_references(&if implementation { tr("Go To Implementation") } else { tr("Go To Definition") }, &l);
                    }
                }
            }
            Err(e) => report(&e),
        }
    });
}

/// Opens a location: a file in a tab (read-only for metadata as source),
/// a source-generated document in a peek view.
async fn open_location(client: &Client, from: &Rc<CodeDoc>, l: &lt::Location) {
    let app = crate::ui::window::app();
    let Some(path) = roslyn::path_from_uri(l.uri.as_str()) else {
        match location_text(client, l).await {
            Some((name, _, text)) => peek_popover(from, &name, &text, l.range.start),
            None => flash(&tr("The definition is not available as source.")),
        }
        return;
    };
    if !path.exists() {
        flash(&trf("{} cannot be read.", &[&path.display().to_string()]));
        return;
    }
    app.tabs.goto(&path, l.range.start.line + 1, l.range.start.character + 1);
    if navigation::is_metadata(&path, &host::home())
        && let Some(c) = app.tabs.code_doc(&path)
    {
        // Decompiled from an assembly: read, never edited.
        c.view.set_editable(false);
        flash(&tr("Metadata as source: decompiled, read-only."));
    }
}

/// Alt+F12: the definition in a read-only view under the caret.
pub fn peek_definition() {
    let Some((client, doc)) = server() else { return };
    let params = position_params(&doc);
    working(&tr("Finding the definition…"));
    glib::spawn_future_local(async move {
        let l = match ask(&client, "textDocument/definition", params, QUICK).await {
            Ok(v) => navigation::locations(v),
            Err(e) => return report(&e),
        };
        let Some(l) = l.into_iter().next() else { return flash(&tr("Cannot navigate to the symbol under the caret.")) };
        match location_text(&client, &l).await {
            Some((name, _, text)) => {
                flash("");
                peek_popover(&doc, &name, &text, l.range.start);
            }
            None => flash(&tr("The definition is not available as source.")),
        }
    });
}

fn peek_popover(doc: &CodeDoc, title: &str, text: &str, at: lt::Position) {
    let buffer = sourceview5::Buffer::new(None);
    buffer.set_language(sourceview5::LanguageManager::default().language("anvil-csharp").as_ref());
    buffer.set_style_scheme(sourceview5::StyleSchemeManager::default().scheme(if adw::StyleManager::default().is_dark() { "anvil-dark" } else { "anvil-light" }).as_ref());
    buffer.set_text(text);
    let view = sourceview5::View::with_buffer(&buffer);
    view.set_editable(false);
    view.set_monospace(true);
    view.set_show_line_numbers(true);
    view.set_highlight_current_line(true);
    view.add_css_class("editor");
    let width = (doc.view.width() - 80).clamp(480, 1100);
    let scroller = gtk::ScrolledWindow::builder().child(&view).min_content_height(280).min_content_width(width).build();
    let label = gtk::Label::builder().label(title).xalign(0.0).hexpand(true).css_classes(["panel-title"]).build();
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.append(&label);
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&head);
    b.append(&scroller);
    let pop = gtk::Popover::new();
    pop.set_child(Some(&b));
    pop.set_parent(&doc.view);
    pop.set_has_arrow(false);
    let rect = doc.view.iter_location(&doc.buffer.iter_at_mark(&doc.buffer.get_insert()));
    let (x, y) = doc.view.buffer_to_window_coords(gtk::TextWindowType::Widget, rect.x(), rect.y() + rect.height());
    pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x, y, 1, 1)));
    pop.set_position(gtk::PositionType::Bottom);
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    pop.popup();
    if let Some(mut it) = buffer.iter_at_line(at.line as i32) {
        it.forward_chars(at.character as i32);
        buffer.place_cursor(&it);
        glib::idle_add_local_once(move || {
            view.scroll_to_mark(&buffer.get_insert(), 0.0, true, 0.0, 0.3);
        });
    }
}

// ---- Find All References ------------------------------------------------

struct References {
    widget: gtk::Box,
    title: gtk::Label,
    list: gtk::ListBox,
    /// Per row: the place it opens (file headers open nothing).
    rows: RefCell<Vec<Option<(PathBuf, u32, u32)>>>,
}

thread_local! {
    static REFS: OnceCell<Rc<References>> = const { OnceCell::new() };
}

fn refs() -> Rc<References> {
    REFS.with(|r| {
        r.get_or_init(|| {
            let widget = gtk::Box::new(gtk::Orientation::Vertical, 4);
            let title = gtk::Label::builder().xalign(0.0).margin_start(14).css_classes(["dim"]).build();
            let list = gtk::ListBox::new();
            list.add_css_class("results");
            let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();
            widget.append(&title);
            widget.append(&scroller);
            list.connect_row_activated(|_, row| {
                let place = refs().rows.borrow().get(row.index() as usize).cloned().flatten();
                if let Some((p, line, col)) = place {
                    crate::ui::window::app().tabs.goto(&p, line, col);
                }
            });
            Rc::new(References { widget, title, list, rows: RefCell::default() })
        })
        .clone()
    })
}

/// The panel for the bottom stack.
pub fn references_widget() -> gtk::Widget {
    refs().widget.clone().upcast()
}

/// Shift+F12.
pub fn find_references() {
    let Some((client, doc)) = server() else { return };
    let mut params = position_params(&doc);
    params["context"] = json!({"includeDeclaration": true});
    working(&tr("Finding references…"));
    glib::spawn_future_local(async move {
        match ask(&client, "textDocument/references", params, SEARCH).await {
            Ok(v) => {
                let l = navigation::locations(v);
                if l.is_empty() {
                    flash(&tr("No references found."));
                } else {
                    flash("");
                    show_references(&tr("Find All References"), &l);
                }
            }
            Err(e) => report(&e),
        }
    });
}

/// The panel: one header per file with its count, then each place with
/// its line, the symbol in bold.
fn show_references(title: &str, locs: &[lt::Location]) {
    let r = refs();
    let groups = navigation::group_by_file(locs);
    let total: usize = groups.iter().map(|(_, v)| v.len()).sum();
    r.title.set_label(&format!("{title} · {}", trf("{} references in {} files", &[&total.to_string(), &groups.len().to_string()])));
    while let Some(c) = r.list.first_child() {
        r.list.remove(&c);
    }
    let mut rows = Vec::new();
    let root = crate::ui::window::solution_dir();
    for (path, ranges) in &groups {
        let rel = root.as_ref().and_then(|d| path.strip_prefix(d).ok()).unwrap_or(path);
        let head = gtk::Label::builder().label(format!("{}  ({})", rel.display(), ranges.len())).xalign(0.0).css_classes(["strong"]).build();
        let row = gtk::ListBoxRow::builder().child(&head).activatable(false).selectable(false).build();
        r.list.append(&row);
        rows.push(None);
        let text = file_text(path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        for range in ranges {
            let line = lines.get(range.start.line as usize).copied().unwrap_or("");
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            b.set_margin_start(18);
            let loc = gtk::Label::builder().label(format!("{}:{}", range.start.line + 1, range.start.character + 1)).width_chars(8).xalign(0.0).css_classes(["dim", "mono", "tnum"]).build();
            let l = gtk::Label::builder().use_markup(true).label(highlight_markup(line, range)).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["mono"]).build();
            b.append(&loc);
            b.append(&l);
            r.list.append(&b);
            rows.push(Some((path.clone(), range.start.line + 1, range.start.character + 1)));
        }
    }
    *r.rows.borrow_mut() = rows;
    crate::ui::window::show_bottom("references");
}

/// The trimmed line with the referenced span in bold (UTF-16 columns).
fn highlight_markup(line: &str, range: &lt::Range) -> String {
    let at = |ch: u32| anvil_lsp::position::to_offset(line, lt::Position { line: 0, character: ch });
    let (a, b) = (at(range.start.character), if range.end.line == range.start.line { at(range.end.character) } else { line.len() });
    let lead = line.len() - line.trim_start().len();
    let a = a.max(lead).min(line.len());
    let b = b.max(a).min(line.len());
    let esc = |s: &str| glib::markup_escape_text(s).to_string();
    format!("{}<b>{}</b>{}", esc(&line[lead..a]), esc(&line[a..b]), esc(line[b..].trim_end()))
}

// ---- Quick Actions --------------------------------------------------------

/// `#rrggbb` for Pango markup and SVG (neither reads every CSS form).
fn hex(c: &gtk::gdk::RGBA) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.red()), b(c.green()), b(c.blue()))
}

/// The lightbulb's glyph: a bulb outline in the secondary ink.
fn lightbulb_icon() -> gio::BytesIcon {
    let ink = hex(&crate::palette::current().ink2);
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path d="M8 1.8a4.3 4.3 0 0 0-2.6 7.7c.5.4.8 1 .8 1.6v.4h3.6v-.4c0-.6.3-1.2.8-1.6A4.3 4.3 0 0 0 8 1.8z" fill="none" stroke="{ink}" stroke-width="1.3" stroke-linejoin="round"/><path d="M6.3 13.3h3.4M6.8 14.8h2.4" stroke="{ink}" stroke-width="1.3" stroke-linecap="round"/></svg>"##
    );
    gio::BytesIcon::new(&glib::Bytes::from_owned(svg.into_bytes()))
}

/// Gives a C# editor its lightbulb: shown in the gutter on the caret's
/// line when a diagnostic is there, clicked like Ctrl+.
pub fn attach(doc: &Rc<CodeDoc>) {
    if !doc.is_csharp() {
        return;
    }
    let a = sourceview5::MarkAttributes::new();
    a.set_gicon(&lightbulb_icon());
    let tip = crate::commands::tooltip("edit.quick-actions");
    a.connect_query_tooltip_text(move |_, _| tip.clone());
    doc.view.set_mark_attributes(LIGHTBULB, &a, 5);
    let weak = Rc::downgrade(doc);
    doc.buffer.connect_cursor_position_notify(move |_| {
        if let Some(d) = weak.upgrade() {
            update_lightbulb(&d);
        }
    });
}

/// Moves the lightbulb to the caret's line, or removes it.
pub fn update_lightbulb(doc: &CodeDoc) {
    let line = doc.buffer.iter_at_mark(&doc.buffer.get_insert()).line();
    let has = doc.diagnostics.borrow().iter().any(|d| d.range.start.line as i32 <= line && d.range.end.line as i32 >= line);
    let current = doc.buffer.source_marks_at_line(line, Some(LIGHTBULB));
    if has && !current.is_empty() {
        return;
    }
    let (s, e) = doc.buffer.bounds();
    doc.buffer.remove_source_marks(&s, &e, Some(LIGHTBULB));
    if has && let Some(it) = doc.buffer.iter_at_line(line) {
        doc.buffer.create_source_mark(None, LIGHTBULB, &it);
    }
}

/// A gutter click on the lightbulb opens Quick Actions; `true` when it
/// was the lightbulb (so the click sets no breakpoint).
pub fn lightbulb_clicked(doc: &CodeDoc, line: i32) -> bool {
    if doc.buffer.source_marks_at_line(line, Some(LIGHTBULB)).is_empty() {
        return false;
    }
    if let Some(it) = doc.buffer.iter_at_line(line)
        && doc.buffer.iter_at_mark(&doc.buffer.get_insert()).line() != line
    {
        doc.buffer.place_cursor(&it);
    }
    quick_actions();
    true
}

/// A code action as offered, and once resolved.
struct Action {
    title: String,
    offered: anvil_lsp::code_actions::Offered,
    resolved: Option<Value>,
}

/// Roslyn's list with nested actions ("Suppress or configure issues") and
/// each Fix All scope offered as an entry of its own.
fn flatten_actions(list: Vec<Value>) -> Vec<Action> {
    anvil_lsp::code_actions::flatten(&list)
        .into_iter()
        .map(|o| {
            let mut title = o.path.join(" › ");
            if let Some(scope) = &o.scope {
                let scope = match scope.as_str() {
                    "Document" => tr("in Document"),
                    "Project" => tr("in Project"),
                    "Solution" => tr("in Solution"),
                    "ContainingType" => tr("in Containing Type"),
                    "ContainingMember" => tr("in Containing Member"),
                    other => other.to_owned(),
                };
                title = format!("{title} › {scope}");
            }
            Action { title, offered: o, resolved: None }
        })
        .collect()
}

async fn resolve(client: &Client, o: &anvil_lsp::code_actions::Offered) -> Value {
    match anvil_lsp::code_actions::resolve_request(o) {
        Some((method, params)) => ask(client, method, params, QUICK).await.unwrap_or_else(|e| {
            report(&e);
            o.raw.clone()
        }),
        None => o.raw.clone(),
    }
}

/// The file-header rules StyleCop's own "Add StyleCop settings file" fix
/// is offered for (`SettingsFileCodeFixProvider` in 1.1.118).
const SETTINGS_FIX_RULES: [&str; 8] = ["SA1633", "SA1634", "SA1635", "SA1636", "SA1637", "SA1638", "SA1640", "SA1641"];

/// Marks the one action Anvil itself carries out (Roslyn's server cannot
/// add an additional document to a project).
const ANVIL_ACTION: &str = "anvilAction";
const ADD_STYLECOP_SETTINGS: &str = "addStyleCopSettings";

/// "Add StyleCop settings file to the project", as VS offers it on a
/// file-header warning while the project has no `stylecop.json`.
fn stylecop_settings_action(doc: &CodeDoc, diags: &[lt::Diagnostic]) -> Option<Action> {
    let header = diags.iter().any(|d| matches!(&d.code, Some(lt::NumberOrString::String(c)) if SETTINGS_FIX_RULES.contains(&c.as_str())));
    if !header {
        return None;
    }
    let app = crate::ui::window::app();
    let sol = app.solution.borrow();
    let project = sol.as_ref()?.project_of(&doc.path())?;
    let text = std::fs::read_to_string(&project.path).ok()?;
    let c = anvil_core::csproj::Csproj::parse(&text).ok()?;
    if c.has_additional_file("stylecop.json") {
        return None;
    }
    let title = tr("Add StyleCop settings file to the project");
    let raw = json!({"title": title, ANVIL_ACTION: ADD_STYLECOP_SETTINGS, "project": project.path, "dir": project.dir});
    Some(Action { title: title.clone(), offered: anvil_lsp::code_actions::Offered { path: vec![title], raw, scope: None }, resolved: None })
}

/// Writes `stylecop.json` beside the project (kept if it exists) and
/// lists it as `<AdditionalFiles>`, as StyleCop's fix does in VS.
fn add_stylecop_settings(action: &Value) {
    let (Some(project), Some(dir)) = (action.get("project").and_then(Value::as_str), action.get("dir").and_then(Value::as_str)) else { return };
    let path = Path::new(dir).join("stylecop.json");
    if !path.exists() {
        let bytes = anvil_core::templates::render(anvil_core::templates::ItemTemplate::StyleCopSettings, &anvil_core::templates::Ctx { namespace: "", name: "stylecop" });
        if let Err(e) = anvil_core::text::write_atomic(&path, &bytes) {
            return crate::ui::window::show_error(&tr("Add StyleCop settings file to the project"), &e.to_string());
        }
    }
    if let Err(e) = crate::ui::newitem::register_additional_file(Path::new(project), "stylecop.json") {
        return crate::ui::window::show_error(&tr("Add StyleCop settings file to the project"), &e);
    }
    crate::ui::explorer::reload_solution_model();
    crate::ui::window::app().tabs.open(&path, false);
}

/// Ctrl+. : the code actions at the caret (with the diagnostics of its
/// line), with a preview of the selected one's change.
pub fn quick_actions() {
    let Some((client, doc)) = server() else { return };
    let text = doc.text();
    let at = doc.cursor_byte();
    let (start, end) = match doc.buffer.selection_bounds() {
        Some((a, b)) => {
            let byte = |it: &gtk::TextIter| doc.buffer.text(&doc.buffer.start_iter(), it, true).len();
            (byte(&a), byte(&b))
        }
        None => (at, at),
    };
    let range = lt::Range { start: to_position(&text, start), end: to_position(&text, end) };
    let line = range.start.line;
    let diags: Vec<lt::Diagnostic> = doc.diagnostics.borrow().iter().filter(|d| d.range.start.line <= line && d.range.end.line >= line).cloned().collect();
    let params = json!({"textDocument": {"uri": uri(&doc.path())}, "range": range, "context": {"diagnostics": diags, "triggerKind": 1}});
    working(&tr("Finding quick actions…"));
    glib::spawn_future_local(async move {
        let list = match ask(&client, "textDocument/codeAction", params, QUICK).await {
            Ok(v) => v.as_array().cloned().unwrap_or_default(),
            Err(e) => return report(&e),
        };
        let mut actions = flatten_actions(list);
        if let Some(a) = stylecop_settings_action(&doc, &diags) {
            actions.insert(0, a);
        }
        if actions.is_empty() {
            return flash(&tr("No quick actions here."));
        }
        flash("");
        show_actions(client, doc, actions);
    });
}

fn show_actions(client: Client, doc: Rc<CodeDoc>, actions: Vec<Action>) {
    let list = gtk::ListBox::new();
    list.add_css_class("results");
    for a in &actions {
        list.append(&gtk::Label::builder().label(&a.title).xalign(0.0).build());
    }
    let left = gtk::ScrolledWindow::builder().child(&list).max_content_height(320).propagate_natural_height(true).propagate_natural_width(true).build();
    let preview = gtk::Label::builder().xalign(0.0).yalign(0.0).use_markup(true).css_classes(["mono"]).selectable(false).build();
    let right = gtk::ScrolledWindow::builder().child(&preview).min_content_width(460).max_content_height(320).propagate_natural_height(true).build();
    right.add_css_class("well");
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    b.append(&left);
    b.append(&right);
    let pop = gtk::Popover::new();
    pop.set_child(Some(&b));
    pop.set_parent(&doc.view);
    pop.set_has_arrow(false);
    let rect = doc.view.iter_location(&doc.buffer.iter_at_mark(&doc.buffer.get_insert()));
    let (x, y) = doc.view.buffer_to_window_coords(gtk::TextWindowType::Widget, rect.x(), rect.y());
    pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x, y, 1, rect.height())));
    pop.set_position(gtk::PositionType::Bottom);
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    let actions = Rc::new(RefCell::new(actions));
    // Preview of the selected action: resolved once, kept.
    let (c, acts, pv) = (client.clone(), actions.clone(), preview.clone());
    list.connect_row_selected(move |_, row| {
        let Some(i) = row.map(|r| r.index() as usize) else { return };
        let (c, acts, pv) = (c.clone(), acts.clone(), pv.clone());
        glib::spawn_future_local(async move {
            let cached = acts.borrow().get(i).and_then(|a| a.resolved.clone());
            let resolved = match cached {
                Some(r) => r,
                None => {
                    pv.set_markup(&format!("<i>{}</i>", glib::markup_escape_text(&tr("Loading preview…"))));
                    let Some(o) = acts.borrow().get(i).map(|a| a.offered.clone()) else { return };
                    let r = resolve(&c, &o).await;
                    if let Some(a) = acts.borrow_mut().get_mut(i) {
                        a.resolved = Some(r.clone());
                    }
                    r
                }
            };
            pv.set_markup(&preview_markup(&resolved));
        });
    });
    let (p, acts) = (pop.clone(), actions.clone());
    list.connect_row_activated(move |_, row| {
        let i = row.index() as usize;
        p.popdown();
        let (client, acts) = (client.clone(), acts.clone());
        glib::spawn_future_local(async move {
            let cached = acts.borrow().get(i).and_then(|a| a.resolved.clone());
            let action = match cached {
                Some(r) => r,
                None => {
                    let Some(o) = acts.borrow().get(i).map(|a| a.offered.clone()) else { return };
                    resolve(&client, &o).await
                }
            };
            apply_action(&client, &action).await;
        });
    });
    pop.popup();
    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
        first.grab_focus();
    }
}

async fn apply_action(client: &Client, action: &Value) {
    if action.get(ANVIL_ACTION).and_then(Value::as_str) == Some(ADD_STYLECOP_SETTINGS) {
        return add_stylecop_settings(action);
    }
    let mut did = false;
    if let Some(edit) = action.get("edit").and_then(|e| serde_json::from_value::<lt::WorkspaceEdit>(e.clone()).ok()) {
        did = crate::ui::lsp::apply_workspace_edit(&edit);
        if !did {
            flash(&tr("The change could not be applied to every file."));
        }
    }
    // `roslyn.client.*` commands (nested actions, Fix All) exist only in
    // VS Code; their edit came from resolving, the server cannot run them.
    if let Some(cmd) = action.get("command").filter(|c| c.get("command").and_then(Value::as_str).is_some_and(|n| !anvil_lsp::code_actions::is_client_command(n))) {
        did = true;
        if let Err(e) = ask(client, "workspace/executeCommand", json!({"command": cmd.get("command"), "arguments": cmd.get("arguments")}), QUICK).await {
            report(&e);
        }
    }
    if !did {
        flash(&tr("This action has no change Anvil can apply."));
    }
}

/// Removed lines in the error colour, added ones in the success colour,
/// per changed file.
fn preview_markup(action: &Value) -> String {
    let p = crate::palette::current();
    if action.get(ANVIL_ACTION).and_then(Value::as_str) == Some(ADD_STYLECOP_SETTINGS) {
        let add = |l: &str| format!("<span foreground=\"{}\">+ {}</span>", hex(&p.up), glib::markup_escape_text(l));
        return ["<b>stylecop.json</b>".to_owned(), add("\"settings\": { \"documentationRules\": { \"companyName\": … } }"), String::new(), "<b>.csproj</b>".to_owned(), add("<AdditionalFiles Include=\"stylecop.json\" />")].join("\n");
    }
    let Some(edit) = action.get("edit").and_then(|e| serde_json::from_value::<lt::WorkspaceEdit>(e.clone()).ok()) else {
        return glib::markup_escape_text(&tr("No preview: this action runs a command.")).to_string();
    };
    let files = navigation::workspace_edit_files(&edit);
    let mut out = String::new();
    for (path, edits) in files.iter().take(3) {
        let Some(old) = file_text(path) else { continue };
        let new = navigation::apply_lsp_edits(&old, edits);
        if files.len() > 1 {
            out.push_str(&format!("<b>{}</b>\n", glib::markup_escape_text(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())));
        }
        let lines = navigation::preview_lines(&old, &new, 2);
        for (kind, l) in lines.iter().take(40) {
            let esc = glib::markup_escape_text(l);
            match kind {
                '-' => out.push_str(&format!("<span foreground=\"{}\" strikethrough=\"true\">- {esc}</span>\n", hex(&p.down))),
                '+' => out.push_str(&format!("<span foreground=\"{}\">+ {esc}</span>\n", hex(&p.up))),
                _ => out.push_str(&format!("<span foreground=\"{}\">  {esc}</span>\n", hex(&p.ink3))),
            }
        }
    }
    if out.is_empty() { glib::markup_escape_text(&tr("No preview available.")).to_string() } else { out.trim_end().to_owned() }
}

// ---- Completion: the `using` an item brings -----------------------------

/// After a completion item is inserted: its additional edits (the `using`
/// of a type from another namespace). Roslyn sends them only on
/// `completionItem/resolve`, so an item without them is resolved first.
pub fn additional_edits(doc: &Rc<CodeDoc>, item: lt::CompletionItem) {
    if let Some(extra) = item.additional_text_edits.clone().filter(|v| !v.is_empty()) {
        apply_doc_edits(doc, &extra);
        return;
    }
    if item.data.is_none() {
        return;
    }
    let Some(client) = crate::ui::window::app().lsp.client() else { return };
    let doc = doc.clone();
    glib::spawn_future_local(async move {
        let Ok(v) = ask(&client, "completionItem/resolve", serde_json::to_value(&item).unwrap_or(Value::Null), QUICK).await else { return };
        let extra: Vec<lt::TextEdit> = v.get("additionalTextEdits").and_then(|e| serde_json::from_value(e.clone()).ok()).unwrap_or_default();
        if !extra.is_empty() {
            apply_doc_edits(&doc, &extra);
        }
    });
}

fn apply_doc_edits(doc: &CodeDoc, edits: &[lt::TextEdit]) {
    let text = doc.text();
    let core: Vec<anvil_core::edit::TextEdit> = navigation::byte_edits(&text, edits).into_iter().map(|(s, e, t)| anvil_core::edit::TextEdit::new(s, e, t)).collect();
    doc.apply_edits(&core);
}

// ---- Formatting -----------------------------------------------------------

/// Ctrl+K, Ctrl+D (`selection_only == false`) and Ctrl+K, Ctrl+F.
pub fn format(selection_only: bool) {
    let Some((client, doc)) = server() else { return };
    glib::spawn_future_local(async move {
        if let Err(e) = format_doc(&client, &doc, selection_only).await {
            report(&e);
        }
    });
}

/// Formats through the server and applies the edits as one undo step;
/// the buffer keeps `\n`, the file's BOM and line endings come back on
/// save. Edits for an older text (the user typed meanwhile) are dropped.
async fn format_doc(client: &Client, doc: &CodeDoc, selection_only: bool) -> Result<(), Error> {
    let text = doc.text();
    let version = doc.version.get();
    let opts = json!({"tabSize": doc.view.tab_width(), "insertSpaces": doc.view.is_insert_spaces_instead_of_tabs()});
    let selection = doc.buffer.selection_bounds().filter(|_| selection_only).map(|(a, b)| {
        let byte = |it: &gtk::TextIter| doc.buffer.text(&doc.buffer.start_iter(), it, true).len();
        (byte(&a), byte(&b))
    });
    let (method, params) = match selection {
        Some((a, b)) => ("textDocument/rangeFormatting", json!({"textDocument": {"uri": uri(&doc.path())}, "range": {"start": to_position(&text, a), "end": to_position(&text, b)}, "options": opts})),
        None => ("textDocument/formatting", json!({"textDocument": {"uri": uri(&doc.path())}, "options": opts})),
    };
    let v = ask(client, method, params, QUICK).await?;
    if doc.version.get() != version {
        return Ok(());
    }
    let edits: Vec<lt::TextEdit> = serde_json::from_value(v).unwrap_or_default();
    if !edits.is_empty() {
        apply_doc_edits(doc, &edits);
    }
    Ok(())
}

/// Ctrl+S with "Format on save": formats, then saves. `true` when it took
/// the save over; a failed format still saves, with a status message.
pub fn format_before_save(doc: &Rc<CodeDoc>) -> bool {
    let app = crate::ui::window::app();
    if !app.settings.boolean("format-on-save") || !doc.is_csharp() {
        return false;
    }
    let Some(client) = app.lsp.client() else { return false };
    let doc = doc.clone();
    glib::spawn_future_local(async move {
        if let Err(e) = format_doc(&client, &doc, false).await {
            report(&e);
        }
        let app = crate::ui::window::app();
        match doc.save() {
            Ok(()) => app.lsp.did_save(&doc),
            Err(e) => crate::ui::window::show_error(&trf("{} could not be saved", &[&doc.path().display().to_string()]), &e.to_string()),
        }
        app.tabs.refresh_titles();
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_line_markup_bolds_the_symbol() {
        let r = lt::Range { start: lt::Position { line: 0, character: 12 }, end: lt::Position { line: 0, character: 16 } };
        assert_eq!(highlight_markup("        var Area = a < b;", &r), "var <b>Area</b> = a &lt; b;");
    }

    #[test]
    fn fix_all_entries_name_their_scope() {
        let list = vec![json!({"title": "Fix All: Declare accessibility", "command": {"command": "roslyn.client.fixAllCodeAction", "arguments": [{"UniqueIdentifier": "Fix All: Declare accessibility", "FixAllFlavors": ["Document", "Solution"]}]}})];
        let t: Vec<String> = flatten_actions(list).into_iter().map(|a| a.title).collect();
        assert_eq!(t, ["Fix All: Declare accessibility › in Document", "Fix All: Declare accessibility › in Solution"]);
    }
}
