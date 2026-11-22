//! Add → New Item… (`Ctrl+Shift+A`) and Add → Class… (`Shift+Alt+C`):
//! VS 2026's C# item templates, written as VS writes them.

use std::path::Path;

use adw::prelude::*;
use anvil_core::templates::{self, Ctx, ItemTemplate};
use gtk::{gio, glib};

use crate::i18n::{tr, trf};
use crate::ui::explorer;
use crate::ui::window::{self};

fn template_detail(t: ItemTemplate) -> String {
    match t {
        ItemTemplate::Class => tr("An empty class definition"),
        ItemTemplate::Interface => tr("An empty interface definition"),
        ItemTemplate::Enum => tr("An empty enumeration"),
        ItemTemplate::Struct => tr("An empty structure definition"),
        ItemTemplate::Record => tr("An empty record definition"),
        ItemTemplate::ClassDiagram => tr("A diagram of the classes of the project (.cd)"),
        ItemTemplate::MsTestClass => tr("A class with an MSTest test method"),
        ItemTemplate::NUnitTest => tr("A class with NUnit set-up and test methods"),
        ItemTemplate::Json => tr("An empty JSON file"),
        ItemTemplate::Text => tr("An empty text file"),
        ItemTemplate::EditorConfig => tr("The .NET code style settings file"),
    }
}

pub fn show(preselect: Option<ItemTemplate>) {
    let app = crate::ui::window::app();
    let Some((project, dir)) = explorer::target_folder() else {
        app.status.flash(&tr("Select a project in the Solution Explorer first."));
        return;
    };
    let list = gtk::ListBox::new();
    list.add_css_class("templates");
    for t in ItemTemplate::ALL {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        b.append(&gtk::Label::builder().label(tr(t.title())).xalign(0.0).css_classes(["template-title"]).build());
        b.append(&gtk::Label::builder().label(template_detail(t)).xalign(0.0).css_classes(["hint"]).build());
        list.append(&b);
    }
    let name = gtk::Entry::builder().activates_default(true).hexpand(true).build();
    let initial = preselect.unwrap_or(ItemTemplate::Class);
    let pos = ItemTemplate::ALL.iter().position(|t| *t == initial).unwrap_or(0);
    let set_default_name = {
        let name = name.clone();
        let dir = dir.clone();
        move |t: ItemTemplate| {
            let (stem, ext) = t.default_name();
            let n = if ext.is_empty() { stem.to_owned() } else { templates::next_free_name(&dir, stem, ext) };
            name.set_text(&n);
        }
    };
    set_default_name(initial);
    if let Some(row) = list.row_at_index(pos as i32) {
        list.select_row(Some(&row));
    }
    let sdn = set_default_name.clone();
    list.connect_row_selected(move |_, row| {
        if let Some(r) = row {
            sdn(ItemTemplate::ALL[r.index() as usize]);
        }
    });
    let scroller = gtk::ScrolledWindow::builder().child(&list).min_content_height(320).min_content_width(460).css_classes(["templates-scroll"]).overflow(gtk::Overflow::Hidden).hscrollbar_policy(gtk::PolicyType::Never).build();
    let b = gtk::Box::new(gtk::Orientation::Vertical, 10);
    b.append(&scroller);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&gtk::Label::new(Some(&tr("Name:"))));
    row.append(&name);
    b.append(&row);
    // Problems with the name show here while typing, instead of an error
    // after Add.
    let problem = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["hint", "inline-error"]).visible(false).build();
    b.append(&problem);
    let rel = app.solution.borrow().as_ref().map(|s| dir.strip_prefix(&s.dir).unwrap_or(&dir).display().to_string()).unwrap_or_default();
    // The folder is worth a line only below the project's own folder.
    let body = (rel != project && !rel.is_empty()).then_some(rel);
    let dialog = adw::AlertDialog::new(Some(&trf("Add New Item - {}", &[&project])), body.as_deref());
    dialog.set_extra_child(Some(&b));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("add", &tr("Add"))]);
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("add"));
    dialog.set_close_response("cancel");
    let check = glib::clone!(
        #[weak]
        dialog,
        #[weak]
        list,
        #[weak]
        problem,
        #[strong]
        dir,
        move |text: &str| {
            let t = list.selected_row().map(|r| ItemTemplate::ALL[r.index() as usize]).unwrap_or(initial);
            let msg = item_problem(&dir, t, text);
            problem.set_label(msg.as_deref().unwrap_or(""));
            problem.set_visible(msg.is_some());
            dialog.set_response_enabled("add", msg.is_none());
        }
    );
    let c = check.clone();
    name.connect_changed(move |e| c(&e.text()));
    check(&name.text());
    glib::idle_add_local_once(glib::clone!(
        #[weak]
        name,
        move || {
            name.grab_focus();
            let text = name.text();
            let stem = text.rfind('.').unwrap_or(text.len());
            name.select_region(0, stem as i32);
        }
    ));
    dialog.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
        if r != "add" {
            return;
        }
        let t = list.selected_row().map(|r| ItemTemplate::ALL[r.index() as usize]).unwrap_or(initial);
        let file_name = final_name(t, &name.text());
        add_item(&project, &dir, t, &file_name);
    });
}

/// The file name the dialog would write: the template's extension added.
fn final_name(t: ItemTemplate, text: &str) -> String {
    let mut file_name = text.trim().to_owned();
    let (_, ext) = t.default_name();
    if !ext.is_empty() && !file_name.to_lowercase().ends_with(ext) {
        file_name.push_str(ext);
    }
    file_name
}

/// Why `text` cannot be the new item's name, if it cannot.
fn item_problem(dir: &Path, t: ItemTemplate, text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return Some(tr("Enter a name."));
    }
    if text.contains(['/', '\\']) || text.starts_with('.') && text.len() == 1 {
        return Some(tr("The name cannot contain / or \\."));
    }
    let file_name = final_name(t, text);
    dir.join(&file_name).exists().then(|| trf("A file with the name '{}' already exists.", &[&file_name]))
}

/// Writes the item and opens it. Returns the new file.
pub fn add_item(project: &str, dir: &Path, t: ItemTemplate, file_name: &str) -> Option<std::path::PathBuf> {
    let app = crate::ui::window::app();
    let path = dir.join(file_name);
    if path.exists() {
        window::show_error(&tr("Add New Item"), &trf("A file with the name '{}' already exists.", &[file_name]));
        return None;
    }
    let sol = app.solution.borrow().clone()?;
    let p = sol.project_named(project)?;
    let rel = dir.strip_prefix(&p.dir).map(|r| r.to_string_lossy().into_owned()).unwrap_or_default();
    let ns = templates::namespace_for(&p.root_namespace(), &rel);
    let stem = Path::new(file_name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let bytes = templates::render(t, &Ctx { namespace: &ns, name: &stem });
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|_| anvil_core::text::write_atomic(&path, &bytes)) {
        window::show_error(&tr("Add New Item"), &e.to_string());
        return None;
    }
    app.explorer.reload();
    app.tabs.open(&path, false);
    if let Some(d) = app.tabs.current_code() {
        // VS leaves the caret on the empty line in the body.
        let text = d.text();
        if let Some(i) = text.find("{\n\n") {
            let it = d.iter_at_byte(i + 2);
            d.buffer.place_cursor(&it);
        }
    }
    Some(path)
}
