//! VS' Ctrl+Tab window switcher: the open files, most recently used first;
//! Tab and Shift+Tab (or the arrows) move while Ctrl is held, releasing
//! Ctrl switches, Escape stays.

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use gtk::{gdk, glib};

use crate::i18n::tr;

pub struct Switcher {
    pub widget: gtk::Box,
    list: gtk::ListBox,
    pages: RefCell<Vec<adw::TabPage>>,
    index: Cell<usize>,
}

impl Switcher {
    pub fn new() -> Switcher {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 6);
        widget.add_css_class("popup");
        widget.add_css_class("tab-switcher");
        widget.set_halign(gtk::Align::Center);
        widget.set_valign(gtk::Align::Center);
        widget.set_visible(false);
        let title = gtk::Label::builder().label(tr("Active Files")).xalign(0.0).css_classes(["group-label"]).build();
        let list = gtk::ListBox::new();
        list.add_css_class("results");
        list.set_selection_mode(gtk::SelectionMode::Single);
        let scroller = gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).propagate_natural_height(true).max_content_height(420).min_content_width(360).build();
        widget.append(&title);
        widget.append(&scroller);
        list.connect_row_activated(|_, row| {
            let app = crate::ui::window::app();
            app.switcher.index.set(row.index().max(0) as usize);
            app.switcher.commit();
        });
        Switcher { widget, list, pages: RefCell::default(), index: Cell::new(0) }
    }

    /// Watches for Ctrl being let go anywhere in the window.
    pub fn connect(&self, window: &adw::ApplicationWindow) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_released(|_, key, _, _| {
            if matches!(key, gdk::Key::Control_L | gdk::Key::Control_R)
                && let Some(app) = crate::ui::window::try_app()
                && app.switcher.is_open()
            {
                app.switcher.commit();
            }
        });
        window.add_controller(keys);
        // Ctrl released while another window had the focus never arrives.
        window.connect_is_active_notify(|w| {
            if !w.is_active()
                && let Some(app) = crate::ui::window::try_app()
            {
                app.switcher.close();
            }
        });
    }

    pub fn is_open(&self) -> bool {
        self.widget.is_visible()
    }

    /// Ctrl+Tab (forward) or Ctrl+Shift+Tab: opens the switcher on the
    /// previous file, or moves while it is open.
    pub fn step(&self, forward: bool) {
        if self.is_open() {
            self.select(anvil_core::mru::step(self.pages.borrow().len(), self.index.get(), forward));
            return;
        }
        let app = crate::ui::window::app();
        let items = app.tabs.mru_pages();
        if items.len() < 2 {
            return;
        }
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        for (_, name, dir) in &items {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            b.append(&gtk::Label::builder().label(name).xalign(0.0).hexpand(true).css_classes(["strong"]).build());
            b.append(&gtk::Label::builder().label(dir).xalign(1.0).css_classes(["dim"]).build());
            self.list.append(&b);
        }
        *self.pages.borrow_mut() = items.into_iter().map(|(p, _, _)| p).collect();
        self.widget.set_visible(true);
        crate::motion::fade_in(&self.widget, 0.0, crate::motion::FAST_MS);
        self.select(anvil_core::mru::first_choice(self.pages.borrow().len(), forward));
    }

    fn select(&self, i: usize) {
        self.index.set(i);
        if let Some(row) = self.list.row_at_index(i as i32) {
            // The focus stays in the editor; only the selection moves.
            self.list.select_row(Some(&row));
        }
    }

    fn commit(&self) {
        let page = self.pages.borrow().get(self.index.get()).cloned();
        self.close();
        if let Some(p) = page {
            crate::ui::window::app().tabs.select_page(&p);
        }
    }

    fn close(&self) {
        self.widget.set_visible(false);
        self.pages.borrow_mut().clear();
    }

    /// Keys while open, before the keymap sees them.
    pub fn handle_key(&self, key: gdk::Key) -> glib::Propagation {
        if !self.is_open() {
            return glib::Propagation::Proceed;
        }
        match key {
            gdk::Key::Escape => {
                self.close();
                crate::ui::window::app().tabs.focus_current();
            }
            gdk::Key::Return | gdk::Key::KP_Enter => self.commit(),
            gdk::Key::Down => self.step(true),
            gdk::Key::Up => self.step(false),
            gdk::Key::Tab => self.step(true),
            gdk::Key::ISO_Left_Tab => self.step(false),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }
}
