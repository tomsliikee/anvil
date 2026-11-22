//! Feature Search (`Ctrl+Q`), as VS' search in the title bar: one field in
//! the header that finds every command of the registry (with its shortcut
//! and menu), the settings pages, the open documents, the files of the
//! solution and the recent solutions. Fuzzy matches are highlighted,
//! recent picks come first, arrows move, Enter runs, Escape closes.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use anvil_core::search::{self, Candidate, Mru};
use gtk::glib;

use crate::i18n::{tr, trf};

#[derive(Clone)]
enum Target {
    Command(&'static str),
    Settings(&'static str),
    File(PathBuf),
    Solution(PathBuf),
}

struct Entry {
    target: Target,
    title: String,
    /// Matched with less weight: VS' command name, a relative path.
    alt: Option<String>,
    detail: String,
    shortcut: String,
    key: String,
    weight: i32,
    icon: &'static str,
}

struct Palette {
    button: gtk::Button,
    popover: gtk::Popover,
    entry: gtk::SearchEntry,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    empty: gtk::Label,
    entries: RefCell<Vec<Entry>>,
    shown: RefCell<Vec<usize>>,
    mru: RefCell<Mru>,
}

thread_local! {
    static PALETTE: RefCell<Option<Rc<Palette>>> = const { RefCell::new(None) };
}

const LIMIT: usize = 60;
const MAX_FILES: usize = 5000;

fn mru_path() -> PathBuf {
    crate::config_dir().join("feature-search.json")
}

fn load_mru() -> Mru {
    std::fs::read_to_string(mru_path()).map(|j| Mru::from_json(&j)).unwrap_or_default()
}

fn save_mru(m: &Mru) {
    let dir = crate::config_dir();
    let saved = std::fs::create_dir_all(&dir).and_then(|_| anvil_core::text::write_atomic(&mru_path(), m.to_json().as_bytes()));
    if let Err(e) = saved {
        // Not worth a toast: only the order of the next search suffers.
        eprintln!("anvil: feature search history not saved: {e}");
    }
}

/// The search field in the header bar. Built once with the window.
pub fn header_button() -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&gtk::Image::from_icon_name("system-search-symbolic"));
    row.append(&gtk::Label::new(Some(&tr("Search"))));
    row.append(&gtk::Label::builder().label("Ctrl+Q").css_classes(["dim", "shortcut"]).build());
    let button = gtk::Button::builder().child(&row).valign(gtk::Align::Center).css_classes(["header-pill", "feature-search-button"]).build();
    button.set_tooltip_text(Some(&tr("Feature Search: commands, settings and files")));

    let entry = gtk::SearchEntry::builder().placeholder_text(tr("Search commands, settings and files")).hexpand(true).build();
    entry.add_css_class("search-entry");
    let list = gtk::ListBox::new();
    list.add_css_class("results");
    list.add_css_class("feature-search");
    list.set_selection_mode(gtk::SelectionMode::Browse);
    // The entry keeps the focus; the list is reached by the arrows.
    list.set_can_focus(false);
    let scroller = gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).max_content_height(440).propagate_natural_height(true).build();
    let empty = gtk::Label::builder().label(tr("No results.")).xalign(0.0).css_classes(["hint"]).margin_start(12).margin_top(6).margin_bottom(6).visible(false).build();
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    b.set_margin_top(10);
    b.set_margin_bottom(10);
    b.set_margin_start(10);
    b.set_margin_end(10);
    b.append(&entry);
    b.append(&empty);
    b.append(&scroller);
    b.set_size_request(620, -1);
    let popover = gtk::Popover::builder().child(&b).has_arrow(false).position(gtk::PositionType::Bottom).build();
    popover.add_css_class("anvil");
    popover.set_parent(&button);

    let p = Rc::new(Palette { button: button.clone(), popover, entry, list, scroller, empty, entries: RefCell::default(), shown: RefCell::default(), mru: RefCell::new(load_mru()) });
    PALETTE.with(|c| *c.borrow_mut() = Some(p.clone()));

    button.connect_clicked(|_| open());
    p.entry.connect_search_changed(|e| fill(&e.text()));
    p.entry.connect_activate(|_| run_selected());
    p.entry.connect_stop_search(|_| close());
    p.list.connect_row_activated(|_, row| run_row(row.index()));
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(|_, key, _, state| {
        let step = match key {
            gtk::gdk::Key::Down | gtk::gdk::Key::KP_Down => 1,
            gtk::gdk::Key::Up | gtk::gdk::Key::KP_Up => -1,
            gtk::gdk::Key::Page_Down | gtk::gdk::Key::KP_Page_Down => 10,
            gtk::gdk::Key::Page_Up | gtk::gdk::Key::KP_Page_Up => -10,
            _ => return glib::Propagation::Proceed,
        };
        if state.intersects(gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::ALT_MASK) {
            return glib::Propagation::Proceed;
        }
        if let Some(p) = get() {
            move_selection(&p.list, &p.scroller, step);
        }
        glib::Propagation::Stop
    });
    p.entry.add_controller(keys);
    button.upcast()
}

fn get() -> Option<Rc<Palette>> {
    PALETTE.with(|c| c.borrow().clone())
}

/// Moves the selected row of `list` by `step`, clamped, and scrolls it
/// into view; the focus stays where it is (in a search field).
pub fn move_selection(list: &gtk::ListBox, scroller: &gtk::ScrolledWindow, step: i32) {
    let cur = list.selected_row().map_or(-1, |r| r.index());
    let mut n = 0;
    while list.row_at_index(n).is_some() {
        n += 1;
    }
    if n == 0 {
        return;
    }
    let i = (cur + step).clamp(0, n - 1);
    let Some(row) = list.row_at_index(i) else { return };
    list.select_row(Some(&row));
    if let Some(bounds) = row.compute_bounds(list) {
        let adj = scroller.vadjustment();
        let (top, bottom) = (bounds.y() as f64, (bounds.y() + bounds.height()) as f64);
        if top < adj.value() {
            adj.set_value(top);
        } else if bottom > adj.value() + adj.page_size() {
            adj.set_value(bottom - adj.page_size());
        }
    }
}

/// Up, Down, Page Up and Page Down in `entry` move through `list`, as in
/// Feature Search (Go To All and other pickers use it).
pub fn arrow_keys(entry: &impl IsA<gtk::Widget>, list: &gtk::ListBox, scroller: &gtk::ScrolledWindow) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let (l, s) = (list.downgrade(), scroller.downgrade());
    keys.connect_key_pressed(move |_, key, _, _| {
        let step = match key {
            gtk::gdk::Key::Down | gtk::gdk::Key::KP_Down => 1,
            gtk::gdk::Key::Up | gtk::gdk::Key::KP_Up => -1,
            gtk::gdk::Key::Page_Down | gtk::gdk::Key::KP_Page_Down => 10,
            gtk::gdk::Key::Page_Up | gtk::gdk::Key::KP_Page_Up => -10,
            _ => return glib::Propagation::Proceed,
        };
        if let (Some(l), Some(s)) = (l.upgrade(), s.upgrade()) {
            move_selection(&l, &s, step);
        }
        glib::Propagation::Stop
    });
    entry.add_controller(keys);
}

fn category(id: &str) -> String {
    match id.split('.').next().unwrap_or("") {
        "file" => tr("File"),
        "edit" => tr("Edit"),
        "view" => tr("View"),
        "window" => tr("Window"),
        "build" => tr("Build"),
        "debug" => tr("Debug"),
        "test" => tr("Test"),
        "project" => tr("Project"),
        "explorer" => tr("Solution Explorer"),
        "designer" => tr("Class Designer"),
        "git" => "Git".into(),
        "tools" => tr("Tools"),
        _ => tr("Help"),
    }
}

fn shown_path(p: &Path, root: Option<&Path>) -> String {
    if let Some(r) = root
        && let Ok(rel) = p.strip_prefix(r)
    {
        return rel.display().to_string();
    }
    let home = glib::home_dir();
    p.strip_prefix(&home).map(|r| format!("~/{}", r.display())).unwrap_or_else(|_| p.display().to_string())
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Everything Feature Search can find right now. Commands come from the
/// registry, so a new command is searchable without listing it here.
fn collect() -> Vec<Entry> {
    let app = crate::ui::window::app();
    let menus = crate::ui::menus::menu_paths();
    let keymap = app.keymap.borrow().clone();
    let mut out = Vec::new();
    for c in anvil_core::commands::COMMANDS {
        out.push(Entry {
            target: Target::Command(c.id),
            title: tr(c.title),
            alt: Some(c.vs_name.to_owned()),
            detail: menus.get(c.id).cloned().unwrap_or_else(|| category(c.id)),
            shortcut: keymap.label(c.id).unwrap_or_default(),
            key: format!("cmd:{}", c.id),
            weight: 12,
            icon: "system-run-symbolic",
        });
    }
    for (name, title) in [("general", tr("General")), ("editor", tr("Editor")), ("keyboard", tr("Keyboard")), ("ai", tr("AI")), ("tools", tr("Tools"))] {
        out.push(Entry { target: Target::Settings(name), detail: tr("Settings"), alt: Some(format!("{} {title}", tr("Settings"))), title, shortcut: String::new(), key: format!("settings:{name}"), weight: 10, icon: "emblem-system-symbolic" });
    }
    let root = crate::ui::window::solution_dir();
    let open = app.tabs.open_paths();
    for p in &open {
        out.push(Entry { target: Target::File(p.clone()), title: file_name(p), alt: Some(shown_path(p, root.as_deref())), detail: trf("Open · {}", &[&shown_path(p.parent().unwrap_or(p), root.as_deref())]), shortcut: String::new(), key: format!("file:{}", p.display()), weight: 8, icon: "text-x-generic-symbolic" });
    }
    if let Some(sol) = app.solution.borrow().as_ref() {
        let mut files = Vec::new();
        for proj in &sol.projects {
            anvil_core::solution::walk(&proj.dir, &proj.dir, &mut |_, abs| {
                if files.len() < MAX_FILES {
                    files.push(abs.to_owned());
                }
            });
            files.push(proj.path.clone());
        }
        files.sort();
        files.dedup();
        for p in files.into_iter().filter(|p| !open.contains(p)) {
            out.push(Entry { target: Target::File(p.clone()), title: file_name(&p), alt: Some(shown_path(&p, root.as_deref())), detail: shown_path(p.parent().unwrap_or(&p), root.as_deref()), shortcut: String::new(), key: format!("file:{}", p.display()), weight: 0, icon: "text-x-generic-symbolic" });
        }
    }
    for s in app.settings.strv("recent-solutions").iter() {
        let p = PathBuf::from(s.as_str());
        if p.exists() {
            out.push(Entry { target: Target::Solution(p.clone()), title: file_name(&p), alt: Some(shown_path(&p, None)), detail: trf("Recent · {}", &[&shown_path(p.parent().unwrap_or(&p), None)]), shortcut: String::new(), key: format!("recent:{}", p.display()), weight: 4, icon: "document-open-recent-symbolic" });
        }
    }
    out
}

/// `text` as Pango markup with the chars at `positions` in bold.
pub fn highlight(text: &str, positions: &[usize]) -> String {
    let mut out = String::new();
    for (i, c) in text.chars().enumerate() {
        let esc = glib::markup_escape_text(&c.to_string());
        if positions.contains(&i) {
            out.push_str(&format!("<b>{esc}</b>"));
        } else {
            out.push_str(&esc);
        }
    }
    out
}

fn fill(query: &str) {
    let Some(p) = get() else { return };
    while let Some(c) = p.list.first_child() {
        p.list.remove(&c);
    }
    let entries = p.entries.borrow();
    let cands: Vec<Candidate> = entries.iter().map(|e| Candidate { text: &e.title, alt: e.alt.as_deref(), key: &e.key, weight: e.weight }).collect();
    let ranked = search::rank(&cands, query, &p.mru.borrow(), LIMIT);
    for r in &ranked {
        let e = &entries[r.index];
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.append(&gtk::Image::builder().icon_name(e.icon).css_classes(["dim"]).build());
        row.append(&gtk::Label::builder().label(highlight(&e.title, &r.text_positions)).use_markup(true).xalign(0.0).build());
        // When only the second text matched, it is shown, so the
        // highlight explains the hit.
        let detail = if r.alt_positions.is_empty() { glib::markup_escape_text(&e.detail).to_string() } else { highlight(e.alt.as_deref().unwrap_or(""), &r.alt_positions) };
        row.append(&gtk::Label::builder().label(detail).use_markup(true).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Middle).css_classes(["dim"]).build());
        if !e.shortcut.is_empty() {
            row.append(&gtk::Label::builder().label(&e.shortcut).css_classes(["dim", "shortcut"]).build());
        }
        p.list.append(&row);
    }
    if let Some(r) = p.list.row_at_index(0) {
        p.list.select_row(Some(&r));
    }
    p.scroller.vadjustment().set_value(0.0);
    p.empty.set_visible(ranked.is_empty());
    p.scroller.set_visible(!ranked.is_empty());
    *p.shown.borrow_mut() = ranked.iter().map(|r| r.index).collect();
}

/// Ctrl+Q: opens the search, or selects its text when it is open.
pub fn open() {
    let Some(p) = get() else { return };
    if p.popover.is_visible() {
        p.entry.grab_focus();
        p.entry.select_region(0, -1);
        return;
    }
    *p.entries.borrow_mut() = collect();
    // A popover needs a mapped parent (not the case behind a dialog).
    if !p.button.is_mapped() {
        crate::ui::window::app().status.flash(&tr("Feature Search needs the window in front."));
        return;
    }
    p.entry.set_text("");
    fill("");
    p.popover.popup();
    p.entry.grab_focus();
}

pub fn close() {
    if let Some(p) = get() {
        p.popover.popdown();
    }
}

/// For screenshot runs: types `q` into the open search.
pub fn type_query(q: &str) {
    if let Some(p) = get() {
        p.entry.set_text(q);
        fill(q);
    }
}

fn run_selected() {
    let Some(p) = get() else { return };
    if let Some(r) = p.list.selected_row() {
        run_row(r.index());
    }
}

fn run_row(i: i32) {
    let Some(p) = get() else { return };
    let Some(idx) = p.shown.borrow().get(i as usize).copied() else { return };
    let (target, key) = {
        let entries = p.entries.borrow();
        (entries[idx].target.clone(), entries[idx].key.clone())
    };
    p.mru.borrow_mut().push(&key);
    save_mru(&p.mru.borrow());
    p.popover.popdown();
    // After the popover is gone, so a dialog the command opens gets the
    // focus and the command sees the editor as focused.
    glib::idle_add_local_once(move || match target {
        Target::Command(id) => crate::ui::actions::dispatch(id),
        Target::Settings(name) => crate::ui::settings::show(Some(name)),
        Target::File(path) => {
            if path.exists() {
                crate::ui::window::app().tabs.open(&path, false);
            } else {
                crate::ui::window::toast(&trf("{} no longer exists.", &[&file_name(&path)]));
            }
        }
        Target::Solution(path) => crate::ui::window::open_path(&path),
    });
}
