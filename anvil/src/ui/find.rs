//! Find and Replace in a file (`Ctrl+F`, `Ctrl+H`), Find in Files
//! (`Ctrl+Shift+F`) and the Find Results list that also shows Find All
//! References.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use sourceview5::prelude::*;

use crate::i18n::{tr, trf};

pub struct FindBar {
    pub widget: gtk::Box,
    pub entry: gtk::SearchEntry,
    pub replace: gtk::Entry,
    replace_row: gtk::Box,
    settings: sourceview5::SearchSettings,
    context: sourceview5::SearchContext,
    count: gtk::Label,
}

impl FindBar {
    pub fn new(buffer: &sourceview5::Buffer, view: &gtk::TextView) -> FindBar {
        let settings = sourceview5::SearchSettings::new();
        settings.set_wrap_around(true);
        let context = sourceview5::SearchContext::new(buffer, Some(&settings));
        context.set_highlight(true);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 4);
        widget.add_css_class("find-bar");
        widget.set_halign(gtk::Align::End);
        widget.set_valign(gtk::Align::Start);
        widget.set_margin_top(10);
        widget.set_margin_end(18);
        widget.set_visible(false);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let entry = gtk::SearchEntry::new();
        entry.set_width_chars(28);
        let count = gtk::Label::new(None);
        count.add_css_class("count");
        let case = gtk::ToggleButton::with_label("Aa");
        case.set_tooltip_text(Some(&tr("Match case")));
        let word = gtk::ToggleButton::with_label("ab");
        word.set_tooltip_text(Some(&tr("Match whole word")));
        let regex = gtk::ToggleButton::with_label(".*");
        regex.set_tooltip_text(Some(&tr("Use regular expressions")));
        for t in [&case, &word, &regex] {
            t.add_css_class("flat");
        }
        let prev = gtk::Button::from_icon_name("go-up-symbolic");
        let next = gtk::Button::from_icon_name("go-down-symbolic");
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        for b in [&prev, &next, &close] {
            b.add_css_class("flat");
        }
        row.append(&entry);
        row.append(&count);
        row.append(&case);
        row.append(&word);
        row.append(&regex);
        row.append(&prev);
        row.append(&next);
        row.append(&close);
        widget.append(&row);

        let replace_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let replace = gtk::Entry::new();
        replace.set_placeholder_text(Some(&tr("Replace")));
        replace.set_width_chars(28);
        let one = gtk::Button::with_label(&tr("Replace"));
        let all = gtk::Button::with_label(&tr("Replace All"));
        one.add_css_class("flat");
        all.add_css_class("flat");
        replace_row.append(&replace);
        replace_row.append(&one);
        replace_row.append(&all);
        replace_row.set_visible(false);
        widget.append(&replace_row);

        settings.bind_property("case-sensitive", &case, "active").bidirectional().sync_create().build();
        settings.bind_property("at-word-boundaries", &word, "active").bidirectional().sync_create().build();
        settings.bind_property("regex-enabled", &regex, "active").bidirectional().sync_create().build();
        entry.connect_search_changed(glib::clone!(
            #[weak]
            settings,
            move |e| settings.set_search_text(Some(&e.text()).filter(|t| !t.is_empty()).map(|t| t.as_str()))
        ));
        context.connect_occurrences_count_notify(glib::clone!(
            #[weak]
            count,
            move |c| {
                let n = c.occurrences_count();
                count.set_label(&if n < 0 { String::new() } else { trf("{} results", &[&n.to_string()]) });
            }
        ));
        let go = glib::clone!(
            #[weak]
            context,
            #[weak]
            buffer,
            #[weak]
            view,
            move |forward: bool| {
                let (s, e) = buffer.selection_bounds().unwrap_or_else(|| {
                    let it = buffer.iter_at_mark(&buffer.get_insert());
                    (it, it)
                });
                let found = if forward { context.forward(&e) } else { context.backward(&s) };
                if let Some((a, b, _)) = found {
                    buffer.select_range(&a, &b);
                    view.scroll_to_mark(&buffer.get_insert(), 0.2, false, 0.0, 0.0);
                }
            }
        );
        let g = go.clone();
        next.connect_clicked(move |_| g(true));
        let g = go.clone();
        prev.connect_clicked(move |_| g(false));
        let g = go.clone();
        entry.connect_activate(move |_| g(true));
        let g = go.clone();
        entry.connect_next_match(move |_| g(true));
        let g = go.clone();
        entry.connect_previous_match(move |_| g(false));
        // Weak: the bar lives inside the tab, a strong reference here would
        // keep every closed tab (view, buffer, undo history) alive.
        let w = widget.downgrade();
        let v = view.downgrade();
        let hide = move || {
            if let Some(w) = w.upgrade() {
                w.set_visible(false);
            }
            if let Some(v) = v.upgrade() {
                v.grab_focus();
            }
        };
        let h = hide.clone();
        close.connect_clicked(move |_| h());
        let h = hide.clone();
        entry.connect_stop_search(move |_| h());
        one.connect_clicked(glib::clone!(
            #[weak]
            context,
            #[weak]
            buffer,
            #[weak]
            replace,
            move |_| {
                if let Some((mut a, mut b)) = buffer.selection_bounds() {
                    let _ = context.replace(&mut a, &mut b, &replace.text());
                }
                go(true);
            }
        ));
        all.connect_clicked(glib::clone!(
            #[weak]
            context,
            #[weak]
            replace,
            move |_| {
                let _ = context.replace_all(&replace.text());
            }
        ));
        FindBar { widget, entry, replace, replace_row, settings, context, count }
    }

    pub fn show(&self, replace: bool, initial: &str) {
        self.widget.set_visible(true);
        self.replace_row.set_visible(replace);
        if !initial.is_empty() && !initial.contains('\n') {
            self.entry.set_text(initial);
        }
        self.entry.grab_focus();
        let _ = (&self.settings, &self.context, &self.count, &self.replace);
    }
}

/// One line of Find Results.
#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    /// 1-based.
    pub line: u32,
    pub col: u32,
    pub text: String,
}

pub struct FindResults {
    pub widget: gtk::Box,
    list: gtk::ListBox,
    title: gtk::Label,
    hits: Rc<RefCell<Vec<Hit>>>,
}

impl FindResults {
    pub fn new() -> FindResults {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let title = gtk::Label::builder().xalign(0.0).margin_start(14).css_classes(["dim"]).build();
        let list = gtk::ListBox::new();
        list.add_css_class("results");
        list.set_activate_on_single_click(false);
        let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();
        widget.append(&title);
        widget.append(&scroller);
        // One handler for all searches; it reads the current results.
        let hits: Rc<RefCell<Vec<Hit>>> = Rc::default();
        let h = hits.clone();
        list.connect_row_activated(move |_, row| {
            let hit = h.borrow().get(row.index() as usize).cloned();
            if let Some(h) = hit {
                crate::ui::window::app().tabs.goto(&h.path, h.line, h.col);
            }
        });
        FindResults { widget, list, title, hits }
    }

    pub fn show(&self, title: &str, hits: &[Hit]) {
        self.title.set_label(&format!("{title} · {}", trf("{} results", &[&hits.len().to_string()])));
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        let root = crate::ui::window::solution_dir();
        for h in hits {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let rel = root.as_ref().and_then(|r| h.path.strip_prefix(r).ok()).unwrap_or(&h.path);
            let loc = gtk::Label::builder().label(format!("{}({}, {})", rel.display(), h.line, h.col)).xalign(0.0).css_classes(["dim", "mono"]).build();
            let text = gtk::Label::builder().label(h.text.trim()).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["mono"]).build();
            row.append(&loc);
            row.append(&text);
            self.list.append(&row);
        }
        *self.hits.borrow_mut() = hits.to_vec();
        crate::ui::window::show_bottom("find");
    }
}

/// Searches all `.cs` files of the solution (open buffers first, so
/// unsaved text counts) for `needle`.
pub fn find_in_files(needle: &str, case: bool) -> Vec<Hit> {
    let app = crate::ui::window::app();
    let Some(sol) = app.solution.borrow().clone() else { return Vec::new() };
    let mut files: Vec<PathBuf> = sol.projects.iter().flat_map(|p| p.source_files()).collect();
    files.sort();
    files.dedup();
    let lower = needle.to_lowercase();
    let mut hits = Vec::new();
    for f in files {
        let text = match app.tabs.code_doc(&f) {
            Some(d) => d.text(),
            None => match anvil_core::text::TextFile::read(&f) {
                Ok(t) => t.text,
                Err(_) => continue,
            },
        };
        for (i, line) in text.lines().enumerate() {
            let pos = if case { line.find(needle) } else { line.to_lowercase().find(&lower) };
            if let Some(p) = pos {
                hits.push(Hit { path: f.clone(), line: i as u32 + 1, col: line[..p.min(line.len())].chars().count() as u32 + 1, text: line.to_owned() });
            }
        }
    }
    hits
}

pub fn find_in_files_dialog(replace: bool) {
    let app = crate::ui::window::app();
    let initial = app.tabs.current_code().map(|d| d.selection_text()).unwrap_or_default();
    let dialog = adw::AlertDialog::new(Some(&if replace { tr("Replace in Files") } else { tr("Find in Files") }), None);
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let entry = gtk::Entry::builder().text(initial.lines().next().unwrap_or("")).activates_default(true).placeholder_text(tr("Find")).build();
    let with = gtk::Entry::builder().placeholder_text(tr("Replace")).visible(replace).build();
    let case = gtk::CheckButton::with_label(&tr("Match case"));
    b.append(&entry);
    b.append(&with);
    b.append(&case);
    dialog.set_extra_child(Some(&b));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("ok", &if replace { tr("Replace All") } else { tr("Find All") })]);
    dialog.set_default_response(Some("ok"));
    dialog.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
        if r != "ok" || entry.text().is_empty() {
            return;
        }
        let needle = entry.text().to_string();
        let hits = find_in_files(&needle, case.is_active());
        if replace {
            replace_in_files(&hits, &needle, &with.text(), case.is_active());
        }
        crate::ui::window::app().find_results.show(&trf("Find \"{}\"", &[&needle]), &hits);
    });
}

fn replace_in_files(hits: &[Hit], needle: &str, with: &str, case: bool) {
    let app = crate::ui::window::app();
    let mut files: Vec<&Path> = hits.iter().map(|h| h.path.as_path()).collect();
    files.dedup();
    for f in files {
        let replace = |t: &str| if case { t.replace(needle, with) } else { replace_ci(t, needle, with) };
        if let Some(doc) = app.tabs.code_doc(f) {
            let new = replace(&doc.text());
            doc.replace_text(&new);
        } else if let Ok(mut file) = anvil_core::text::TextFile::read(f) {
            file.text = replace(&file.text);
            let _ = file.write(f);
        }
    }
}

fn replace_ci(text: &str, needle: &str, with: &str) -> String {
    let lower = text.to_lowercase();
    let n = needle.to_lowercase();
    if lower.len() != text.len() {
        return text.replace(needle, with);
    }
    let mut out = String::new();
    let mut last = 0;
    for (i, _) in lower.match_indices(&n) {
        out.push_str(&text[last..i]);
        out.push_str(with);
        last = i + n.len();
    }
    out.push_str(&text[last..]);
    out
}
