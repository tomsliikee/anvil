//! The status line under the window: pending chord, messages, caret
//! position, language server state.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::glib;

pub struct StatusBar {
    pub widget: gtk::Box,
    message: gtk::Label,
    chord: gtk::Label,
    position: gtk::Label,
    server: gtk::Label,
    flash_source: Rc<RefCell<Option<glib::SourceId>>>,
}

impl StatusBar {
    pub fn new() -> StatusBar {
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        widget.add_css_class("statusbar");
        let message = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
        let chord = gtk::Label::new(None);
        chord.add_css_class("chord");
        let position = gtk::Label::new(None);
        position.add_css_class("tnum");
        let server = gtk::Label::new(None);
        widget.append(&message);
        widget.append(&chord);
        widget.append(&server);
        widget.append(&position);
        StatusBar { widget, message, chord, position, server, flash_source: Rc::default() }
    }

    pub fn set_message(&self, text: &str) {
        self.message.set_label(text);
    }

    /// A message that clears itself after a few seconds.
    pub fn flash(&self, text: &str) {
        self.message.set_label(text);
        if let Some(id) = self.flash_source.borrow_mut().take() {
            id.remove();
        }
        let label = self.message.downgrade();
        let slot = self.flash_source.clone();
        let id = glib::timeout_add_local_once(std::time::Duration::from_secs(5), move || {
            slot.borrow_mut().take();
            if let Some(l) = label.upgrade() {
                l.set_label("");
            }
        });
        *self.flash_source.borrow_mut() = Some(id);
    }

    pub fn set_chord(&self, text: Option<&str>) {
        self.chord.set_label(text.unwrap_or(""));
    }

    pub fn set_position(&self, line: i32, col: i32) {
        self.position.set_label(&crate::i18n::trf("Ln {}, Col {}", &[&line.to_string(), &col.to_string()]));
    }

    pub fn clear_position(&self) {
        self.position.set_label("");
    }

    pub fn set_server(&self, text: &str) {
        self.server.set_label(text);
    }
}
