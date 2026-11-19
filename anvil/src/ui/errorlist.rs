//! The Error List: build and IntelliSense diagnostics merged, filtered by
//! severity, source and code (StyleCop's `SA` rules can be hidden).
//! Errors carry `down`; warnings are told apart by weight, not amber.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use anvil_core::diagnostics::{Diagnostic, Severity};
use gtk::{gio, glib};

use crate::i18n::{tr, trf};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub project: String,
    pub file: PathBuf,
    pub line: u32,
    pub col: u32,
    pub intellisense: bool,
}

impl Entry {
    fn is_stylecop(&self) -> bool {
        (self.code.starts_with("SA") || self.code.starts_with("SX")) && self.code[2..].chars().all(|c| c.is_ascii_digit())
    }
}

/// Errors, warnings, messages, StyleCop shown; source index; search text.
type Filter = (bool, bool, bool, bool, u32, String);

pub struct ErrorList {
    pub widget: gtk::Box,
    store: gio::ListStore,
    filter: gtk::CustomFilter,
    view: gtk::ColumnView,
    selection: gtk::SingleSelection,
    build: RefCell<Vec<Entry>>,
    intellisense: RefCell<Vec<Entry>>,
    show: Rc<RefCell<Filter>>,
    errors: gtk::ToggleButton,
    warnings: gtk::ToggleButton,
    messages: gtk::ToggleButton,
    stylecop: gtk::CheckButton,
    source: gtk::DropDown,
    search: gtk::SearchEntry,
}

/// A column; `width` > 0 fixes it, so the headers line up with the rows
/// even while the list is empty.
fn column(view: &gtk::ColumnView, title: &str, width: i32, f: impl Fn(&Entry, &gtk::Label) + 'static) {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        item.set_child(Some(&gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build()));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let Some(obj) = item.item().and_downcast::<glib::BoxedAnyObject>() else { return };
        let Some(label) = item.child().and_downcast::<gtk::Label>() else { return };
        f(&obj.borrow::<Entry>(), &label);
    });
    let c = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    c.set_expand(width == 0);
    if width > 0 {
        c.set_fixed_width(width);
    }
    c.set_resizable(true);
    view.append_column(&c);
}

impl ErrorList {
    pub fn new() -> ErrorList {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let show = Rc::new(RefCell::new((true, true, true, true, 0u32, String::new())));
        let s = show.clone();
        let filter = gtk::CustomFilter::new(move |o| {
            let Some(obj) = o.downcast_ref::<glib::BoxedAnyObject>() else { return false };
            let e = obj.borrow::<Entry>();
            let (errors, warnings, messages, stylecop, source, ref query) = *s.borrow();
            let sev = match e.severity {
                Severity::Error => errors,
                Severity::Warning => warnings,
                Severity::Info => messages,
            };
            let src = match source {
                1 => !e.intellisense,
                2 => e.intellisense,
                _ => true,
            };
            let q = query.is_empty() || e.code.to_lowercase().contains(query) || e.message.to_lowercase().contains(query);
            sev && src && q && (stylecop || !e.is_stylecop())
        });
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        let selection = gtk::SingleSelection::new(Some(filtered));
        selection.set_autoselect(false);
        let view = gtk::ColumnView::new(Some(selection.clone()));
        view.add_css_class("diagnostics");
        view.set_show_row_separators(false);
        column(&view, "", 34, |e, l| {
            // A shape per severity: a filled mark for errors in `down`, an
            // outlined one for warnings, a dot for messages.
            l.set_label(match e.severity {
                Severity::Error => "\u{25cf}",
                Severity::Warning => "\u{25b2}",
                Severity::Info => "\u{00b7}",
            });
            for c in ["sev-error", "sev-warning", "sev-info"] {
                l.remove_css_class(c);
            }
            l.add_css_class(match e.severity {
                Severity::Error => "sev-error",
                Severity::Warning => "sev-warning",
                Severity::Info => "sev-info",
            });
        });
        column(&view, &tr("Code"), 96, |e, l| {
            l.set_label(&e.code);
            l.add_css_class("mono");
        });
        column(&view, &tr("Description"), 0, |e, l| {
            l.set_label(&e.message);
            l.set_tooltip_text(Some(&e.message));
        });
        column(&view, &tr("Project"), 130, |e, l| l.set_label(&e.project));
        column(&view, &tr("File"), 170, |e, l| l.set_label(&e.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        column(&view, &tr("Line"), 60, |e, l| {
            l.set_label(&if e.line > 0 { e.line.to_string() } else { String::new() });
            l.add_css_class("tnum");
        });
        column(&view, &tr("Source"), 110, |e, l| l.set_label(&if e.intellisense { tr("IntelliSense") } else { tr("Build") }));

        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bar.add_css_class("filter-bar");
        bar.set_margin_start(10);
        bar.set_margin_end(10);
        let errors = gtk::ToggleButton::builder().active(true).css_classes(["filter-toggle"]).build();
        let warnings = gtk::ToggleButton::builder().active(true).css_classes(["filter-toggle"]).build();
        let messages = gtk::ToggleButton::builder().active(true).css_classes(["filter-toggle"]).build();
        // StyleCop and the source live in one Filter menu; the counters
        // are the only pills in the row.
        let stylecop = gtk::CheckButton::with_label(&tr("Show StyleCop (SA) warnings"));
        stylecop.set_active(true);
        let source = gtk::DropDown::from_strings(&[&tr("Build + IntelliSense"), &tr("Build Only"), &tr("IntelliSense Only")]);
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 8);
        menu.set_margin_top(8);
        menu.set_margin_bottom(8);
        menu.set_margin_start(8);
        menu.set_margin_end(8);
        menu.append(&gtk::Label::builder().label(tr("Source")).xalign(0.0).css_classes(["dim"]).build());
        menu.append(&source);
        menu.append(&stylecop);
        let filter_button = gtk::MenuButton::builder().label(tr("Filter")).always_show_arrow(true).popover(&gtk::Popover::builder().child(&menu).build()).css_classes(["filter-toggle"]).build();
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search Error List")));
        for w in [errors.upcast_ref::<gtk::Widget>(), warnings.upcast_ref(), messages.upcast_ref(), filter_button.upcast_ref()] {
            bar.append(w);
        }
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        bar.append(&spacer);
        bar.append(&search);
        let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let bar_scroller = gtk::ScrolledWindow::builder().child(&bar).hscrollbar_policy(gtk::PolicyType::External).vscrollbar_policy(gtk::PolicyType::Never).propagate_natural_height(true).build();
        widget.append(&bar_scroller);
        widget.append(&scroller);
        ErrorList { widget, store, filter, view, selection, build: RefCell::default(), intellisense: RefCell::default(), show, errors, warnings, messages, stylecop, source, search }
    }

    pub fn connect(&self) {
        let app = crate::ui::window::app();
        self.stylecop.set_active(app.settings.boolean("show-stylecop"));
        let changed = || {
            let app = crate::ui::window::app();
            let me = &app.errors;
            *me.show.borrow_mut() = (
                me.errors.is_active(),
                me.warnings.is_active(),
                me.messages.is_active(),
                me.stylecop.is_active(),
                me.source.selected(),
                me.search.text().to_lowercase(),
            );
            let _ = app.settings.set_boolean("show-stylecop", me.stylecop.is_active());
            me.filter.changed(gtk::FilterChange::Different);
        };
        for t in [&self.errors, &self.warnings, &self.messages] {
            t.connect_toggled(move |_| changed());
        }
        self.stylecop.connect_toggled(move |_| changed());
        self.source.connect_selected_notify(move |_| changed());
        self.search.connect_search_changed(move |_| changed());
        changed();
        self.view.connect_activate(|v, pos| {
            let Some(obj) = v.model().and_then(|m| m.item(pos)).and_downcast::<glib::BoxedAnyObject>() else { return };
            let e = obj.borrow::<Entry>().clone();
            if e.file.is_file() {
                crate::ui::window::app().tabs.goto(&e.file, e.line.max(1), e.col.max(1));
            }
        });
        self.refresh();
    }

    fn refresh(&self) {
        let mut all: Vec<Entry> = self.build.borrow().clone();
        // IntelliSense entries for the same place as a build entry are the
        // same diagnostic; VS shows it once.
        for e in self.intellisense.borrow().iter() {
            if !all.iter().any(|b| b.code == e.code && b.file == e.file && b.line == e.line) {
                all.push(e.clone());
            }
        }
        all.sort_by(|a, b| (a.severity, &a.file, a.line).cmp(&(b.severity, &b.file, b.line)));
        let objs: Vec<glib::BoxedAnyObject> = all.iter().cloned().map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &objs);
        let count = |s: Severity| all.iter().filter(|e| e.severity == s).count();
        self.errors.set_label(&crate::i18n::ntr("{} Error", "{} Errors", count(Severity::Error) as u32).replace("{}", &count(Severity::Error).to_string()));
        self.warnings.set_label(&crate::i18n::ntr("{} Warning", "{} Warnings", count(Severity::Warning) as u32).replace("{}", &count(Severity::Warning).to_string()));
        self.messages.set_label(&crate::i18n::ntr("{} Message", "{} Messages", count(Severity::Info) as u32).replace("{}", &count(Severity::Info).to_string()));
        let _ = &self.selection;
    }

    pub fn set_build(&self, diags: &[Diagnostic], solution_dir: Option<&std::path::Path>) {
        let entries = diags
            .iter()
            .map(|d| Entry {
                severity: d.severity,
                code: d.code.clone(),
                message: d.text().to_owned(),
                project: d.project.as_deref().map(anvil_core::slnx::stem).unwrap_or_default(),
                file: {
                    let p = PathBuf::from(&d.file);
                    if p.is_absolute() { p } else { solution_dir.map(|s| s.join(&p)).unwrap_or(p) }
                },
                line: d.line,
                col: d.col,
                intellisense: false,
            })
            .collect();
        *self.build.borrow_mut() = entries;
        self.refresh();
    }

    pub fn set_intellisense(&self, diags: Vec<(PathBuf, lsp_types::Diagnostic)>) {
        let app = crate::ui::window::app();
        let sol = app.solution.borrow();
        let entries = diags
            .into_iter()
            .filter(|(_, d)| d.severity != Some(lsp_types::DiagnosticSeverity::HINT))
            .map(|(p, d)| Entry {
                severity: match d.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => Severity::Error,
                    Some(lsp_types::DiagnosticSeverity::WARNING) => Severity::Warning,
                    _ => Severity::Info,
                },
                code: match &d.code {
                    Some(lsp_types::NumberOrString::String(s)) => s.clone(),
                    Some(lsp_types::NumberOrString::Number(n)) => n.to_string(),
                    None => String::new(),
                },
                message: d.message.clone(),
                project: sol.as_ref().and_then(|s| s.project_of(&p)).map(|p| p.name.clone()).unwrap_or_default(),
                line: d.range.start.line + 1,
                col: d.range.start.character + 1,
                file: p,
                intellisense: true,
            })
            .collect();
        drop(sol);
        // Diagnostics are pulled after every pause in typing; most pulls
        // bring the same list, and rebuilding the view would rebind every row.
        if *self.intellisense.borrow() == entries {
            return;
        }
        *self.intellisense.borrow_mut() = entries;
        self.refresh();
    }

    pub fn clear_all(&self) {
        self.build.borrow_mut().clear();
        self.intellisense.borrow_mut().clear();
        self.refresh();
    }

    pub fn counts(&self) -> (usize, usize) {
        let b = self.build.borrow();
        (b.iter().filter(|e| e.severity == Severity::Error).count(), b.iter().filter(|e| e.severity == Severity::Warning).count())
    }

    pub fn selected_text(&self) -> Option<String> {
        let obj = self.selection.selected_item().and_downcast::<glib::BoxedAnyObject>()?;
        let e = obj.borrow::<Entry>();
        Some(format!("{} {} ({}:{})", e.code, e.message, e.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), e.line))
    }

    /// Ctrl+Shift+F12: the next error after the current selection.
    pub fn next(&self) {
        let n = self.selection.n_items();
        if n == 0 {
            crate::ui::window::app().status.flash(&tr("No errors."));
            return;
        }
        let next = if self.selection.selected() == gtk::INVALID_LIST_POSITION { 0 } else { (self.selection.selected() + 1) % n };
        self.selection.set_selected(next);
        if let Some(obj) = self.selection.item(next).and_downcast::<glib::BoxedAnyObject>() {
            let e = obj.borrow::<Entry>().clone();
            crate::ui::window::app().status.flash(&trf("{}: {}", &[&e.code, &e.message]));
            if e.file.is_file() {
                crate::ui::window::app().tabs.goto(&e.file, e.line.max(1), e.col.max(1));
            }
        }
    }
}
