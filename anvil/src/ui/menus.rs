//! VS' menu bar (File, Edit, View, Project, Build, Debug, Test, Git,
//! Tools, Help) and the context menus. Shortcuts are shown as VS shows
//! them, chords included (`Ctrl+K, Ctrl+D`), which GTK's own accelerator
//! labels cannot express, so menu rows are built here.

use std::cell::RefCell;

use adw::prelude::*;
use anvil_core::keymap::Combo;
use gtk::{gio, glib};

use crate::commands::title;
use crate::i18n::tr;
use crate::ui::editor::CodeDoc;
use crate::ui::window::try_app;

pub enum Item {
    Command(&'static str),
    /// A plain action with its own label (`clipboard.copy` on a text view).
    Action(String, &'static str),
    Separator,
    Submenu(String, Vec<Item>),
    /// A row that runs a closure.
    Run(String, std::rc::Rc<dyn Fn()>),
}

fn shortcut_label(id: &str) -> Option<String> {
    try_app().and_then(|a| a.keymap.borrow().label(id))
}

/// GTK's accelerator syntax for a single key combination, for menu rows
/// GTK draws itself.
fn gtk_accel(c: &Combo) -> Option<String> {
    let key = match c.key.as_str() {
        "Enter" => "Return".to_owned(),
        "Backspace" => "BackSpace".to_owned(),
        "PageUp" => "Page_Up".to_owned(),
        "PageDown" => "Page_Down".to_owned(),
        "Space" => "space".to_owned(),
        "-" => "minus".to_owned(),
        "=" => "equal".to_owned(),
        "+" => "plus".to_owned(),
        "." => "period".to_owned(),
        "," => "comma".to_owned(),
        ";" => "semicolon".to_owned(),
        "/" => "slash".to_owned(),
        "\\" => "backslash".to_owned(),
        "[" => "bracketleft".to_owned(),
        "]" => "bracketright".to_owned(),
        "`" => "grave".to_owned(),
        "'" => "apostrophe".to_owned(),
        k if k.chars().count() == 1 => k.to_lowercase(),
        k => k.to_owned(),
    };
    let mut s = String::new();
    if c.ctrl {
        s.push_str("<Control>");
    }
    if c.shift {
        s.push_str("<Shift>");
    }
    if c.alt {
        s.push_str("<Alt>");
    }
    s.push_str(&key);
    gtk::accelerator_parse(&s).map(|_| s)
}

/// A row for a chord: the title left, the chord right in ink3.
fn chord_row(id: &'static str, chord: &str) -> gtk::Widget {
    let b = gtk::Button::new();
    b.add_css_class("flat");
    b.add_css_class("menu-row");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    row.append(&gtk::Label::builder().label(title(id)).xalign(0.0).hexpand(true).build());
    row.append(&gtk::Label::builder().label(chord).css_classes(["dim"]).build());
    b.set_child(Some(&row));
    b.connect_clicked(move |b| {
        if let Some(p) = b.ancestor(gtk::Popover::static_type()).and_downcast::<gtk::Popover>() {
            p.popdown();
        }
        crate::ui::actions::dispatch(id);
    });
    b.upcast()
}

struct Custom {
    id: String,
    widget: gtk::Widget,
}

fn model(items: &[Item], custom: &mut Vec<Custom>) -> gio::Menu {
    let menu = gio::Menu::new();
    let mut section = gio::Menu::new();
    for it in items {
        match it {
            Item::Separator => {
                if section.n_items() > 0 {
                    menu.append_section(None, &section);
                }
                section = gio::Menu::new();
            }
            Item::Command(id) => {
                let label = shortcut_label(id);
                let chord = label.as_ref().is_some_and(|l| l.contains(", "));
                if chord {
                    let cid = format!("c{}", custom.len());
                    let item = gio::MenuItem::new(None, None);
                    item.set_attribute_value("custom", Some(&cid.to_variant()));
                    section.append_item(&item);
                    custom.push(Custom { id: cid, widget: chord_row(id, label.as_deref().unwrap_or("")) });
                } else {
                    let item = gio::MenuItem::new(Some(&title(id)), Some(&format!("win.{id}")));
                    if let Some(accel) = label.and_then(|l| Combo::parse(&l)).and_then(|c| gtk_accel(&c)) {
                        item.set_attribute_value("accel", Some(&accel.to_variant()));
                    }
                    section.append_item(&item);
                }
            }
            Item::Action(label, action) => section.append(Some(label), Some(action)),
            Item::Submenu(label, sub) => section.append_submenu(Some(label), &model(sub, custom)),
            Item::Run(label, f) => {
                let cid = format!("c{}", custom.len());
                let item = gio::MenuItem::new(None, None);
                item.set_attribute_value("custom", Some(&cid.to_variant()));
                section.append_item(&item);
                let b = gtk::Button::with_label(label);
                b.add_css_class("flat");
                if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                    l.set_xalign(0.0);
                }
                let f = f.clone();
                b.connect_clicked(move |b| {
                    if let Some(p) = b.ancestor(gtk::Popover::static_type()).and_downcast::<gtk::Popover>() {
                        p.popdown();
                    }
                    f();
                });
                custom.push(Custom { id: cid, widget: b.upcast() });
            }
        }
    }
    if section.n_items() > 0 {
        menu.append_section(None, &section);
    }
    menu
}

fn top_menus() -> Vec<(String, Vec<Item>)> {
    use Item::{Command as C, Separator as S};
    vec![
        (tr("_File"), vec![C("file.new-project"), C("file.open-solution"), C("file.open-folder"), C("file.open-file"), S, C("file.close-solution"), S, C("file.save"), C("file.save-all"), S, C("file.handin"), S, C("file.exit")]),
        (
            tr("_Edit"),
            vec![
                C("edit.undo"),
                C("edit.redo"),
                S,
                C("edit.find"),
                C("edit.replace"),
                C("edit.find-in-files"),
                C("edit.replace-in-files"),
                S,
                C("edit.goto-all"),
                C("edit.goto-line"),
                S,
                Item::Submenu(tr("Advanced"), vec![C("edit.format-document"), C("edit.format-selection"), C("edit.comment"), C("edit.uncomment"), C("edit.toggle-comment"), C("edit.uppercase"), C("edit.lowercase"), C("edit.word-wrap"), C("edit.view-whitespace")]),
                C("edit.duplicate"),
                C("edit.move-line-up"),
                C("edit.move-line-down"),
                C("edit.delete-line"),
                S,
                Item::Submenu(tr("IntelliSense"), vec![C("edit.complete-word"), C("edit.parameter-info"), C("edit.quick-info"), C("edit.ai-suggest"), C("edit.insert-snippet"), C("edit.surround-with")]),
                Item::Submenu(tr("Refactor"), vec![C("edit.rename"), C("edit.quick-actions")]),
            ],
        ),
        (
            tr("_View"),
            vec![
                C("view.solution-explorer"),
                C("view.git-changes"),
                C("view.git-repository"),
                C("view.error-list"),
                C("view.output"),
                C("view.terminal"),
                C("view.test-explorer"),
                C("view.chat"),
                C("edit.document-outline"),
                S,
                C("edit.navigate-back"),
                C("edit.navigate-forward"),
                C("edit.next-error"),
                S,
                C("view.full-screen"),
                C("view.toggle-theme"),
            ],
        ),
        (
            tr("_Project"),
            vec![
                C("project.add-new-item"),
                C("project.add-existing-item"),
                C("project.add-class"),
                C("project.add-folder"),
                C("project.add-class-diagram"),
                C("project.view-class-diagram"),
                S,
                C("project.add-reference"),
                C("view.nuget"),
                C("project.new-project-in-solution"),
                S,
                C("project.set-startup"),
                C("project.properties"),
            ],
        ),
        (tr("_Build"), vec![C("build.solution"), C("build.rebuild-solution"), C("build.clean-solution"), S, C("build.project"), S, C("build.cancel")]),
        (
            tr("_Debug"),
            vec![
                C("debug.start"),
                C("debug.start-without"),
                C("debug.hot-reload"),
                C("debug.stop"),
                C("debug.restart"),
                C("debug.break-all"),
                S,
                C("debug.step-over"),
                C("debug.step-into"),
                C("debug.step-out"),
                C("debug.run-to-cursor"),
                S,
                C("debug.toggle-breakpoint"),
                C("debug.delete-breakpoints"),
                C("debug.quick-watch"),
                S,
                Item::Submenu(tr("Windows"), vec![C("debug.breakpoints"), C("debug.locals"), C("debug.autos"), C("debug.watch"), C("debug.call-stack"), C("debug.immediate")]),
                C("debug.exception-settings"),
            ],
        ),
        (tr("_Test"), vec![C("test.run-all"), C("test.debug-all"), C("test.run-failed"), C("test.repeat-last"), S, C("view.test-explorer")]),
        (tr("_Git"), vec![C("git.commit"), C("git.fetch"), C("git.pull"), C("git.push"), S, C("view.git-changes"), C("view.git-repository"), S, C("git.init")]),
        (tr("_Tools"), vec![C("tools.options"), C("tools.ai-off"), C("tools.restart-roslyn")]),
        (tr("_Help"), vec![C("help.shortcuts")]),
    ]
}

thread_local! {
    static MENUBAR: RefCell<Option<gtk::PopoverMenuBar>> = const { RefCell::new(None) };
}

pub fn menubar() -> gtk::PopoverMenuBar {
    let bar = gtk::PopoverMenuBar::from_model(None::<&gio::MenuModel>);
    MENUBAR.with(|m| *m.borrow_mut() = Some(bar.clone()));
    rebuild_menubar();
    bar
}

/// Rebuilds the bar after the keymap changed (shortcut labels).
pub fn rebuild_menubar() {
    let Some(bar) = MENUBAR.with(|m| m.borrow().clone()) else { return };
    let root = gio::Menu::new();
    let mut custom = Vec::new();
    for (label, items) in top_menus() {
        root.append_submenu(Some(&label), &model(&items, &mut custom));
    }
    bar.set_menu_model(Some(&root));
    for c in custom {
        bar.add_child(&c.widget, &c.id);
    }
}

/// A context menu at (x, y) of `parent`.
pub fn popup(parent: &gtk::Widget, x: f64, y: f64, items: &[Item]) {
    let mut custom = Vec::new();
    let m = model(items, &mut custom);
    let pop = gtk::PopoverMenu::from_model(Some(&m));
    for c in custom {
        pop.add_child(&c.widget, &c.id);
    }
    pop.set_parent(parent);
    pop.set_has_arrow(false);
    pop.set_halign(gtk::Align::Start);
    pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    pop.popup();
}

thread_local! {
    static CARET_POPUP: RefCell<Option<gtk::Popover>> = const { RefCell::new(None) };
}

fn caret_popover(doc: &CodeDoc, child: &impl IsA<gtk::Widget>) -> gtk::Popover {
    close_caret_popup();
    let pop = gtk::Popover::new();
    pop.set_child(Some(child));
    pop.set_parent(&doc.view);
    pop.set_autohide(true);
    pop.set_has_arrow(false);
    let it = doc.buffer.iter_at_mark(&doc.buffer.get_insert());
    let rect = doc.view.iter_location(&it);
    let (x, y) = doc.view.buffer_to_window_coords(gtk::TextWindowType::Widget, rect.x(), rect.y());
    pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x, y, 1, rect.height())));
    pop.set_position(gtk::PositionType::Bottom);
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    CARET_POPUP.with(|c| *c.borrow_mut() = Some(pop.clone()));
    pop
}

pub fn close_caret_popup() {
    CARET_POPUP.with(|c| {
        if let Some(p) = c.borrow_mut().take() {
            p.popdown();
        }
    });
}

/// A list under the caret (quick actions, outline); `on_pick` gets the
/// index.
pub fn choose_at_caret(doc: &CodeDoc, titles: &[String], on_pick: impl Fn(usize) + 'static) {
    let list = gtk::ListBox::new();
    list.add_css_class("results");
    for t in titles {
        list.append(&gtk::Label::builder().label(t).xalign(0.0).build());
    }
    let scroller = gtk::ScrolledWindow::builder().child(&list).max_content_height(360).propagate_natural_height(true).propagate_natural_width(true).build();
    let pop = caret_popover(doc, &scroller);
    let p = pop.clone();
    list.connect_row_activated(move |_, row| {
        p.popdown();
        on_pick(row.index() as usize);
    });
    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    }
    pop.popup();
    list.grab_focus();
}

pub fn tooltip_at_caret(doc: &CodeDoc, text: &str) {
    let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).max_width_chars(90).css_classes(["mono"]).selectable(true).build();
    caret_popover(doc, &l).popup();
}

pub fn markup_at_caret(doc: &CodeDoc, markup: &str) {
    let l = gtk::Label::builder().use_markup(true).label(markup).xalign(0.0).css_classes(["mono"]).build();
    let pop = caret_popover(doc, &l);
    pop.set_autohide(false);
    pop.set_can_focus(false);
    pop.popup();
    doc.view.grab_focus();
}
