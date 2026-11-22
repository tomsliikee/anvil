//! The Output window with VS' "Show output from" panes, one per source.
//! Text is batched (a build writes hundreds of lines a second), each pane
//! keeps its last [`MAX_LINES`] lines, `file(line,col)` places are links,
//! and the view follows new text until the user scrolls up.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use crate::i18n::tr;

/// The pane keys other parts write to; the label is translated.
const PANES: &[&str] = &["Build", "Run", "Debug", "Tests", "Language Server", "Git", "Package Manager", "AI", "General"];

/// Lines kept per pane; older ones go, so a chatty server or a long
/// session cannot grow memory and slow the view without bound.
const MAX_LINES: i32 = 20_000;

/// How long text waits so a burst becomes one insert and one scroll.
const FLUSH: Duration = Duration::from_millis(40);

pub struct Output {
    pub widget: gtk::Box,
    view: gtk::TextView,
    buffers: BTreeMap<&'static str, gtk::TextBuffer>,
    pending: Rc<RefCell<BTreeMap<&'static str, String>>>,
    flush_source: Rc<RefCell<Option<glib::SourceId>>>,
    /// Whether the view sticks to the end; scrolling up turns it off,
    /// scrolling back to the end turns it on.
    follow: Rc<Cell<bool>>,
    pane: gtk::DropDown,
}

fn key(pane: &str) -> &'static str {
    PANES.iter().find(|p| **p == pane).copied().unwrap_or("General")
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
        let view = gtk::TextView::builder().editable(false).cursor_visible(false).monospace(true).wrap_mode(gtk::WrapMode::WordChar).left_margin(12).top_margin(4).bottom_margin(8).build();
        view.add_css_class("output");
        let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 4);
        widget.append(&bar);
        widget.append(&scroller);
        let buffers: BTreeMap<&'static str, gtk::TextBuffer> = PANES
            .iter()
            .map(|p| {
                let b = gtk::TextBuffer::new(None);
                b.create_tag(Some("link"), &[("underline", &gtk::pango::Underline::Single)]);
                (*p, b)
            })
            .collect();
        view.set_buffer(Some(&buffers["Build"]));
        let follow = Rc::new(Cell::new(true));

        // Follow: content growing keeps the end in view while `follow`
        // holds; the user's own scrolling decides `follow`.
        let adj = scroller.vadjustment();
        let f = follow.clone();
        adj.connect_changed(move |a| {
            if f.get() {
                a.set_value(a.upper() - a.page_size());
            }
        });
        let f = follow.clone();
        adj.connect_value_changed(move |a| f.set(a.value() + a.page_size() >= a.upper() - 8.0));

        let out = Output { widget, view, buffers, pending: Rc::default(), flush_source: Rc::default(), follow, pane };
        let bufs = out.buffers.clone();
        let v = out.view.clone();
        let f = out.follow.clone();
        out.pane.connect_selected_notify(move |dd| {
            if let Some(name) = PANES.get(dd.selected() as usize) {
                f.set(true);
                v.set_buffer(Some(&bufs[name]));
                let a = adj.clone();
                glib::idle_add_local_once(move || a.set_value(a.upper() - a.page_size()));
            }
        });
        let v = out.view.clone();
        clear.connect_clicked(move |_| {
            let app = crate::ui::window::app();
            if let Some(name) = PANES.iter().find(|p| app.output.buffers[**p] == v.buffer()) {
                app.output.clear(name);
            }
        });
        out.connect_links();
        out
    }

    /// Clicking a `file(line,col)` opens it there; the pointer shows
    /// which text is a link.
    fn connect_links(&self) {
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        let v = self.view.clone();
        click.connect_released(move |g, _, x, y| {
            // A drag that selected text is not a click on a link.
            if v.buffer().has_selection() {
                return;
            }
            if let Some((file, line, col)) = link_at(&v, x, y) {
                g.set_state(gtk::EventSequenceState::Claimed);
                crate::ui::window::app().tabs.goto(&file, line.max(1), col.max(1));
            }
        });
        self.view.add_controller(click);
        let motion = gtk::EventControllerMotion::new();
        let v = self.view.clone();
        motion.connect_motion(move |_, x, y| {
            let on = link_at(&v, x, y).is_some();
            v.set_cursor_from_name(Some(if on { "pointer" } else { "text" }));
        });
        self.view.add_controller(motion);
    }

    pub fn append(&self, pane: &str, text: &str) {
        let k = key(pane);
        self.pending.borrow_mut().entry(k).or_default().push_str(text);
        if self.flush_source.borrow().is_some() {
            return;
        }
        let id = glib::timeout_add_local_once(FLUSH, || {
            let app = crate::ui::window::app();
            app.output.flush_source.borrow_mut().take();
            app.output.flush();
        });
        *self.flush_source.borrow_mut() = Some(id);
    }

    fn flush(&self) {
        let _span = crate::frame_log::span("output append");
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        for (pane, text) in pending {
            let b = &self.buffers[pane];
            let first_new = b.line_count() - 1;
            let mut end = b.end_iter();
            b.insert(&mut end, &text);
            tag_links(b, first_new);
            // Dropped in blocks of a tenth, so a full pane is not trimmed
            // (and re-laid out) on every flush.
            let excess = b.line_count() - MAX_LINES;
            if excess > MAX_LINES / 10
                && let Some(mut cut) = b.iter_at_line(excess)
            {
                b.delete(&mut b.start_iter(), &mut cut);
            }
        }
    }

    pub fn clear(&self, pane: &str) {
        let k = key(pane);
        self.pending.borrow_mut().remove(k);
        self.buffers[k].set_text("");
        self.follow.set(true);
    }

    /// Selects a pane; the bottom panel stays as it is.
    pub fn show(&self, pane: &str) {
        if let Some(i) = PANES.iter().position(|p| *p == pane) {
            self.pane.set_selected(i as u32);
        }
    }
}

/// Underlines the places in the lines from `first` to the end.
fn tag_links(b: &gtk::TextBuffer, first: i32) {
    for n in first.max(0)..b.line_count() {
        let Some(start) = b.iter_at_line(n) else { continue };
        let mut end = start;
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        let line = b.text(&start, &end, false);
        for loc in anvil_core::diagnostics::locations(&line) {
            let chars = |byte: usize| line[..byte].chars().count() as i32;
            let (Some(s), Some(e)) = (b.iter_at_line_offset(n, chars(loc.start)), b.iter_at_line_offset(n, chars(loc.end))) else { continue };
            b.apply_tag_by_name("link", &s, &e);
        }
    }
}

/// The place under the pointer, resolved to a file that exists.
fn link_at(v: &gtk::TextView, x: f64, y: f64) -> Option<(PathBuf, u32, u32)> {
    let (bx, by) = v.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
    let iter = v.iter_at_location(bx, by)?;
    let b = v.buffer();
    let tag = b.tag_table().lookup("link")?;
    if !iter.has_tag(&tag) {
        return None;
    }
    let mut start = iter;
    start.set_line_offset(0);
    let mut end = start;
    if !end.ends_line() {
        end.forward_to_line_end();
    }
    let line = b.text(&start, &end, false);
    let offset = iter.line_offset() as usize;
    let byte = line.char_indices().nth(offset).map_or(line.len(), |(i, _)| i);
    let loc = anvil_core::diagnostics::locations(&line).into_iter().find(|l| l.start <= byte && byte < l.end)?;
    Some((resolve(&loc.file)?, loc.line, loc.col))
}

/// An absolute path as it is; a relative one (a build without full
/// paths, a tool's own output) against the solution and its projects.
fn resolve(file: &str) -> Option<PathBuf> {
    let p = PathBuf::from(file.replace('\\', "/"));
    if p.is_absolute() {
        return p.is_file().then_some(p);
    }
    let app = crate::ui::window::app();
    let sol = app.solution.borrow();
    let sol = sol.as_ref()?;
    std::iter::once(sol.dir.clone()).chain(sol.projects.iter().map(|pr| pr.dir.clone())).map(|d| d.join(&p)).find(|c| c.is_file())
}
