//! The start page: a statement, the recent solutions, and VS' four ways in.

use adw::prelude::*;
use gtk::glib;

use crate::i18n::tr;
use crate::ui::window::open_path;

pub struct StartPage {
    pub widget: gtk::ScrolledWindow,
    recent: gtk::ListBox,
    empty: gtk::Label,
}

fn big_button(title: &str, detail: &str, icon: &str, command: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("card-panel");
    b.add_css_class("flat");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    row.set_margin_top(14);
    row.set_margin_bottom(14);
    row.set_margin_start(16);
    row.set_margin_end(16);
    row.append(&gtk::Image::from_icon_name(icon));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.append(&gtk::Label::builder().label(title).xalign(0.0).css_classes(["strong"]).build());
    text.append(&gtk::Label::builder().label(detail).xalign(0.0).wrap(true).css_classes(["hint"]).build());
    text.set_hexpand(true);
    row.append(&text);
    // The shortcut, as VS' start window teaches it: keyboard first. The
    // page is built before the keymap is loaded, so VS' default is shown.
    if let Some(k) = anvil_core::commands::find(command).and_then(|c| c.keys.first()) {
        row.append(&gtk::Label::builder().label(*k).valign(gtk::Align::Center).css_classes(["dim", "shortcut"]).build());
    }
    b.set_child(Some(&row));
    b.set_action_name(Some(&format!("win.{command}")));
    b
}

impl StartPage {
    pub fn new() -> StartPage {
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 28);
        outer.set_margin_top(56);
        outer.set_margin_bottom(40);
        outer.set_margin_start(40);
        outer.set_margin_end(40);
        outer.set_halign(gtk::Align::Center);
        outer.set_width_request(760);
        // The logo comes from the icon theme; the application adds its
        // resource path (icons/) to it, so dev builds find it too.
        let brand = gtk::Box::new(gtk::Orientation::Vertical, 12);
        brand.set_halign(gtk::Align::Start);
        let logo = gtk::Image::from_icon_name(crate::APP_ID);
        logo.set_pixel_size(96);
        logo.set_halign(gtk::Align::Start);
        brand.append(&logo);
        brand.append(&gtk::Label::builder().label("Anvil").xalign(0.0).css_classes(["statement", "brand-title"]).build());
        outer.append(&brand);
        let cols = gtk::Box::new(gtk::Orientation::Horizontal, 32);
        cols.set_homogeneous(true);
        let left = gtk::Box::new(gtk::Orientation::Vertical, 10);
        left.append(&gtk::Label::builder().label(tr("Open recent")).xalign(0.0).css_classes(["group-label"]).build());
        let recent = gtk::ListBox::new();
        recent.add_css_class("recent");
        recent.set_selection_mode(gtk::SelectionMode::None);
        let empty = gtk::Label::builder().label(tr("No solutions opened yet.")).xalign(0.0).css_classes(["hint"]).build();
        left.append(&recent);
        left.append(&empty);
        let right = gtk::Box::new(gtk::Orientation::Vertical, 10);
        right.append(&gtk::Label::builder().label(tr("Get started")).xalign(0.0).css_classes(["group-label"]).build());
        right.append(&big_button(&tr("Open a project or solution"), &tr("Open a .slnx or .sln written by Visual Studio"), "document-open-symbolic", "file.open-solution"));
        right.append(&big_button(&tr("Open a local folder"), &tr("A folder that holds one solution"), "folder-open-symbolic", "file.open-folder"));
        right.append(&big_button(&tr("Create a new project"), &tr("Console App, with StyleCop as in the course, or a test project"), "document-new-symbolic", "file.new-project"));
        cols.append(&left);
        cols.append(&right);
        outer.append(&cols);
        let widget = gtk::ScrolledWindow::builder().child(&outer).hscrollbar_policy(gtk::PolicyType::Never).build();
        StartPage { widget, recent, empty }
    }

    pub fn connect(&self) {
        self.recent.connect_row_activated(|_, row| {
            let path = row.widget_name().to_string();
            open_path(std::path::Path::new(&path));
        });
    }

    pub fn refresh(&self) {
        while let Some(c) = self.recent.first_child() {
            self.recent.remove(&c);
        }
        let list: Vec<String> = crate::ui::window::app().settings.strv("recent-solutions").iter().map(|s| s.to_string()).filter(|p| std::path::Path::new(p).exists()).collect();
        self.empty.set_visible(list.is_empty());
        self.recent.set_visible(!list.is_empty());
        for p in list {
            let path = std::path::Path::new(&p);
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&p);
            let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
            b.append(&gtk::Label::builder().label(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).xalign(0.0).css_classes(["strong"]).build());
            let home = glib::home_dir();
            let shown = path.parent().map(|d| d.strip_prefix(&home).map(|r| format!("~/{}", r.display())).unwrap_or_else(|_| d.display().to_string())).unwrap_or_default();
            b.append(&gtk::Label::builder().label(shown).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::Middle).css_classes(["recent-path"]).build());
            row.set_child(Some(&b));
            self.recent.append(&row);
        }
    }
}
