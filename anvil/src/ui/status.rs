//! The status line under the window: the running job (see
//! [`crate::ui::jobs`]), pending chord, messages, caret position,
//! language server state.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::glib;

pub struct StatusBar {
    pub widget: gtk::Box,
    job: gtk::Box,
    job_text: gtk::Label,
    job_time: gtk::Label,
    job_cancel: gtk::Button,
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
        // The job area: a small spinner, what runs, for how long, and a
        // way to stop it. It is there only while something runs.
        let job = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        job.add_css_class("job");
        job.set_visible(false);
        let spinner = adw::Spinner::new();
        spinner.set_size_request(12, 12);
        let job_text = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(48).build();
        let job_time = gtk::Label::new(None);
        job_time.add_css_class("tnum");
        job_time.add_css_class("job-time");
        let job_cancel = gtk::Button::from_icon_name("window-close-symbolic");
        job_cancel.add_css_class("flat");
        job_cancel.add_css_class("job-cancel");
        job_cancel.set_valign(gtk::Align::Center);
        job_cancel.set_tooltip_text(Some(&crate::i18n::tr("Cancel")));
        job_cancel.connect_clicked(|_| crate::ui::jobs::cancel_current());
        job.append(&spinner);
        job.append(&job_text);
        job.append(&job_time);
        job.append(&job_cancel);
        widget.append(&job);
        widget.append(&message);
        widget.append(&chord);
        widget.append(&server);
        widget.append(&position);
        StatusBar { widget, job, job_text, job_time, job_cancel, message, chord, position, server, flash_source: Rc::default() }
    }

    /// The newest running job, or none; called by [`crate::ui::jobs`].
    pub fn show_job(&self, view: Option<&crate::ui::jobs::View>) {
        let Some(v) = view else {
            self.job.set_visible(false);
            return;
        };
        self.job.set_visible(true);
        let text = if v.others.is_empty() { v.text.clone() } else { format!("{} (+{})", v.text, v.others.len()) };
        if self.job_text.label() != text {
            self.job_text.set_label(&text);
        }
        self.job_text.set_tooltip_text((!v.others.is_empty()).then(|| [v.text.as_str()].into_iter().chain(v.others.iter().map(String::as_str)).collect::<Vec<_>>().join("\n")).as_deref());
        self.job_time.set_label(&v.elapsed);
        self.job_cancel.set_visible(v.cancellable);
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
