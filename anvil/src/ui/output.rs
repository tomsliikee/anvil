//! The Output window with VS' "Show output from" panes.

use std::cell::RefCell;
use std::collections::BTreeMap;

use adw::prelude::*;

use crate::i18n::tr;

/// Lines a pane keeps; older ones are dropped in blocks of a tenth.
const MAX_LINES: i32 = 20_000;

const PANES:&[&str] = &["Build", "Debug", "Tests", "Git", "Package Manager", "Language Server", "AI"];

pub struct Output {
    pub widget: gtk::Box,
    view: gtk::TextView,
    buffers: RefCell<BTreeMap<&'static str, gtk::TextBuffer>>,
    pane: gtk::DropDown,
}

impl Output {
    pub fn new() -> Output {
        let labels: Vec<String> = PANES.iter().map(|p| tr(p)).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let pane = gtk::DropDown::from_strings(&refs);
        let clear = gtk::Button::from_icon_name("edit-clear-all-symbolic");
        clear.set_tooltip_text(Some(&tr("Clear All")));
        clear.add_css_class("flat");
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.set_margin_start(10);
        bar.append(&gtk::Label::new(Some(&tr("Show output from:"))));
        bar.append(&pane);
        bar.append(&clear);
        let view = gtk::TextView::builder().editable(false).monospace(true).wrap_mode(gtk::WrapMode::WordChar).left_margin(12).top_margin(4).build();
        view.add_css_class("output");
        let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 4);
        widget.append(&bar);
        widget.append(&scroller);
        let buffers: BTreeMap<&'static str, gtk::TextBuffer> = PANES.iter().map(|p| (*p, gtk::TextBuffer::new(None))).collect();
        view.set_buffer(Some(&buffers["Build"]));
        let out = Output { widget, view, buffers: RefCell::new(buffers), pane };
        let bufs = out.buffers.borrow().clone();
        let v = out.view.clone();
        out.pane.connect_selected_notify(move |dd| {
            if let Some(name) = PANES.get(dd.selected() as usize) {
                v.set_buffer(Some(&bufs[name]));
            }
        });
        let v = out.view.clone();
        clear.connect_clicked(move |_| v.buffer().set_text(""));
        out
    }

    fn buffer(&self, pane: &str) -> gtk::TextBuffer {
        let key = PANES.iter().find(|p| **p == pane).copied().unwrap_or("Build");
        self.buffers.borrow()[key].clone()
    }

    pub fn append(&self, pane: &str, text: &str) {
        let _s = crate::frame_log::span("output append");
        let b = self.buffer(pane);
        let mut end = b.end_iter();
        b.insert(&mut end, text);
        // Roslyn and long builds write without end; a pane keeps its last
        // lines, so appending and wrapping stay cheap after hours.
        let extra = b.line_count() - MAX_LINES;
        if extra > MAX_LINES / 10 {
            let mut s = b.start_iter();
            let mut cut = b.iter_at_line(extra).unwrap_or_else(|| b.end_iter());
            b.delete(&mut s, &mut cut);
        }
        if self.view.buffer() == b {
            let mark = b.create_mark(None, &b.end_iter(), false);
            self.view.scroll_mark_onscreen(&mark);
            b.delete_mark(&mark);
        }
    }

    pub fn clear(&self, pane: &str) {
        self.buffer(pane).set_text("");
    }

    /// Shows a pane (Build when a build starts, as VS does).
    pub fn show(&self, pane: &str) {
        if let Some(i) = PANES.iter().position(|p| *p == pane) {
            self.pane.set_selected(i as u32);
        }
    }
}
