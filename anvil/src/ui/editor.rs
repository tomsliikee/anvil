//! Editor tabs. Each code tab is a GtkSourceView over a buffer that knows
//! the file's encoding and line endings, so saving writes back exactly what
//! was there plus the edits. Class diagrams open as tabs too.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use anvil_core::highlight;
use anvil_core::keymap::Combo;
use anvil_core::text::{Encoding, Eol, TextFile};
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib, graphene};
use sourceview5::prelude::*;

use crate::i18n::{tr, trf};
use crate::palette;

pub const BREAKPOINT: &str = "anvil-breakpoint";
pub const BREAKPOINT_DISABLED: &str = "anvil-breakpoint-disabled";
pub const BREAKPOINT_CONDITION: &str = "anvil-breakpoint-condition";
pub const BREAKPOINT_PENDING: &str = "anvil-breakpoint-pending";
const BREAKPOINT_KINDS: [&str; 4] = [BREAKPOINT, BREAKPOINT_DISABLED, BREAKPOINT_CONDITION, BREAKPOINT_PENDING];
pub const CURRENT_LINE: &str = "anvil-current";

/// A snippet's fields still to visit (start, end) and its `$end$`.
pub type SnippetFields = (Vec<(gtk::TextMark, gtk::TextMark)>, gtk::TextMark);

/// VS' breakpoint red and current-statement yellow, readable on both the
/// light and the dark editor.
const BREAKPOINT_RED: &str = "#d13438";
const CURRENT_YELLOW: &str = "#f2c230";

/// The gutter glyphs as small SVGs: filled (set), hollow (disabled), with
/// a plus (condition or hit count), hollow with a dot (not bound yet).
fn gutter_icon(kind: &str) -> gio::BytesIcon {
    let r = BREAKPOINT_RED;
    let body = match kind {
        BREAKPOINT_DISABLED => format!(r##"<circle cx="8" cy="8" r="5.6" fill="none" stroke="{r}" stroke-width="1.6"/>"##),
        BREAKPOINT_CONDITION => format!(r##"<circle cx="8" cy="8" r="6.2" fill="{r}"/><path d="M8 4.8v6.4M4.8 8h6.4" stroke="#fff" stroke-width="1.7" stroke-linecap="round"/>"##),
        BREAKPOINT_PENDING => format!(r##"<circle cx="8" cy="8" r="5.6" fill="none" stroke="{r}" stroke-width="1.6"/><circle cx="8" cy="8" r="1.8" fill="{r}"/>"##),
        CURRENT_LINE => format!(r##"<path d="M2.5 5.5h5V2.8L13.5 8l-6 5.2v-2.7h-5z" fill="{CURRENT_YELLOW}" stroke="#7a5b00" stroke-width="0.8" stroke-linejoin="round"/>"##),
        _ => format!(r##"<circle cx="8" cy="8" r="6.2" fill="{r}"/>"##),
    };
    let svg = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">{body}</svg>"##);
    gio::BytesIcon::new(&glib::Bytes::from_owned(svg.into_bytes()))
}

/// Adds Anvil's language definitions and colour schemes to GtkSourceView's
/// search paths (the GNOME runtime ships none).
pub fn register_sourceview_paths() {
    let dir = crate::data_dir().join("sourceview");
    let lm = sourceview5::LanguageManager::default();
    let mut paths: Vec<String> = lm.search_path().iter().map(|p| p.to_string()).collect();
    paths.insert(0, dir.join("language-specs").to_string_lossy().into_owned());
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    lm.set_search_path(&refs);
    let sm = sourceview5::StyleSchemeManager::default();
    sm.prepend_search_path(&dir.join("styles").to_string_lossy());
}

fn scheme() -> Option<sourceview5::StyleScheme> {
    let id = if adw::StyleManager::default().is_dark() { "anvil-dark" } else { "anvil-light" };
    sourceview5::StyleSchemeManager::default().scheme(id)
}

fn language_for(path: &Path) -> Option<sourceview5::Language> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let id = match ext.as_str() {
        "cs" | "csx" => "anvil-csharp",
        "csproj" | "slnx" | "cd" | "xml" | "props" | "targets" | "config" | "resx" | "ruleset" => "anvil-xml",
        _ => return None,
    };
    sourceview5::LanguageManager::default().language(id)
}

// ---- The view: GtkSourceView plus ghost text -----------------------------

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct AnvilView {
        /// Ghost text and the offset it belongs to.
        pub ghost: RefCell<Option<(i32, String)>>,
        /// The debugger's current line (0-based), drawn as a fill band.
        pub current_line: Cell<Option<i32>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AnvilView {
        const NAME: &'static str = "AnvilView";
        type Type = super::AnvilView;
        type ParentType = sourceview5::View;
    }

    impl ObjectImpl for AnvilView {}
    impl WidgetImpl for AnvilView {}
    impl sourceview5::subclass::prelude::ViewImpl for AnvilView {}

    impl TextViewImpl for AnvilView {
        fn snapshot_layer(&self, layer: gtk::TextViewLayer, snapshot: gtk::Snapshot) {
            self.parent_snapshot_layer(layer, snapshot.clone());
            let view = self.obj();
            let p = palette::current();
            // The statement the debugger stopped at: VS' yellow band.
            if layer == gtk::TextViewLayer::BelowText
                && let Some(line) = self.current_line.get()
                && let Some(it) = view.buffer().iter_at_line(line)
            {
                let (y, h) = view.line_yrange(&it);
                let w = view.width().max(4000);
                let alpha = if adw::StyleManager::default().is_dark() { 0.24 } else { 0.38 };
                let band = gdk::RGBA::parse(super::CURRENT_YELLOW).map(|c| c.with_alpha(alpha)).unwrap_or(gdk::RGBA::BLACK);
                snapshot.append_color(&band, &graphene::Rect::new(0.0, y as f32, w as f32, h as f32));
            }
            if layer != gtk::TextViewLayer::AboveText {
                return;
            }
            let ghost = self.ghost.borrow();
            let Some((offset, text)) = ghost.as_ref() else { return };
            let it = view.buffer().iter_at_offset(*offset);
            let rect = view.iter_location(&it);
            let ctx = view.pango_context();
            let mut first = true;
            let (mut y, line_h) = (rect.y() as f32, rect.height().max(14) as f32);
            for line in text.split('\n') {
                let layout = gtk::pango::Layout::new(&ctx);
                layout.set_text(line);
                let x = if first { rect.x() as f32 } else { view.left_margin() as f32 + 0.0 };
                snapshot.save();
                snapshot.translate(&graphene::Point::new(x, y));
                snapshot.append_layout(&layout, &p.ink3);
                snapshot.restore();
                first = false;
                y += line_h;
            }
        }
    }
}

glib::wrapper! {
    pub struct AnvilView(ObjectSubclass<imp::AnvilView>)
        @extends sourceview5::View, gtk::TextView, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Scrollable;
}

impl AnvilView {
    pub fn new(buffer: &sourceview5::Buffer) -> AnvilView {
        glib::Object::builder().property("buffer", buffer).build()
    }

    pub fn set_ghost(&self, ghost: Option<(i32, String)>) {
        *self.imp().ghost.borrow_mut() = ghost;
        self.queue_draw();
    }

    pub fn ghost(&self) -> Option<(i32, String)> {
        self.imp().ghost.borrow().clone()
    }

    pub fn set_current_line(&self, line: Option<i32>) {
        self.imp().current_line.set(line);
        self.queue_draw();
    }
}

// ---- Documents ----------------------------------------------------------

pub struct CodeDoc {
    pub path: RefCell<PathBuf>,
    pub buffer: sourceview5::Buffer,
    pub view: AnvilView,
    pub root: gtk::Overlay,
    pub find: crate::ui::find::FindBar,
    pub encoding: Cell<Encoding>,
    pub eol: Cell<Eol>,
    /// LSP document version, bumped on every change.
    pub version: Cell<i32>,
    /// True while Anvil itself changes the buffer (reload, applied edits
    /// that should not reach the AI or close the preview).
    pub quiet: Cell<bool>,
    pub diagnostics: RefCell<Vec<lsp_types::Diagnostic>>,
    pub change_serial: Cell<u64>,
    /// Type, method and property spans from the syntax tree, as last
    /// applied, and the character count they were computed for.
    highlights: RefCell<(Vec<highlight::Span>, i32)>,
    highlight_timer: RefCell<Option<glib::SourceId>>,
    /// The breakpoint marks drawn last; they move with the text.
    bp_marks: RefCell<Vec<sourceview5::Mark>>,
    /// An expanded snippet's remaining tab stops (start, end) and where
    /// the caret ends; Tab walks them, Enter or Escape leaves.
    pub snippet: RefCell<Option<SnippetFields>>,
    /// Gliding scroll and Anvil's own caret.
    pub smooth: Rc<crate::ui::smooth::Smooth>,
}

const HIGHLIGHT_TAGS: [(&str, highlight::Kind, &str); 3] = [
    ("hl-type", highlight::Kind::Type, "anvil:type"),
    ("hl-method", highlight::Kind::Method, "anvil:method"),
    ("hl-property", highlight::Kind::Property, "anvil:property"),
];

/// Type and method names: Cascadia Mono's italic with its cursive forms
/// (`ss01`), as VS Code themes set them; same advance, so columns stay
/// straight.
const NAME_FEATURES: &str = "ss01";

impl CodeDoc {
    /// Re-tags the names that changed since the last pass, 80 ms after
    /// the last edit.
    fn schedule_highlight(self: &Rc<Self>) {
        if !self.is_csharp() {
            return;
        }
        if let Some(id) = self.highlight_timer.borrow_mut().take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(80), move || {
            if let Some(d) = weak.upgrade() {
                d.highlight_timer.borrow_mut().take();
                d.highlight_now();
            }
        });
        *self.highlight_timer.borrow_mut() = Some(id);
    }

    /// Parses on a worker thread (a full tree-sitter pass over a large
    /// file takes longer than a frame) and tags on the main thread, unless
    /// the text changed meanwhile: the next pass is already scheduled then.
    fn highlight_now(self: &Rc<Self>) {
        let text = {
            let _s = crate::frame_log::span("name highlight copy");
            self.text()
        };
        let serial = self.change_serial.get();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(new) = gio::spawn_blocking(move || highlight::spans(&text)).await else { return };
            if let Some(d) = weak.upgrade()
                && d.change_serial.get() == serial
            {
                d.apply_highlights(new);
            }
        });
    }

    fn apply_highlights(&self, new: Vec<highlight::Span>) {
        let _s = crate::frame_log::span("name highlight");
        let count = self.buffer.char_count();
        let (old, old_count) = std::mem::take(&mut *self.highlights.borrow_mut());
        let range = if old.is_empty() && old_count == 0 { Some((0, count as usize)) } else { highlight::changed_range(&old, &new, (count - old_count) as isize) };
        if let Some((a, b)) = range {
            let (ia, ib) = (self.buffer.iter_at_offset(a as i32), self.buffer.iter_at_offset(b as i32));
            for (tag, _, _) in HIGHLIGHT_TAGS {
                self.buffer.remove_tag_by_name(tag, &ia, &ib);
            }
            for s in new.iter().filter(|s| s.end >= a && s.start <= b) {
                let Some((tag, _, _)) = HIGHLIGHT_TAGS.iter().find(|(_, k, _)| *k == s.kind) else { continue };
                self.buffer.apply_tag_by_name(tag, &self.buffer.iter_at_offset(s.start as i32), &self.buffer.iter_at_offset(s.end as i32));
            }
        }
        *self.highlights.borrow_mut() = (new, count);
    }

    pub fn text(&self) -> String {
        let (s, e) = self.buffer.bounds();
        self.buffer.text(&s, &e, true).to_string()
    }

    pub fn path(&self) -> PathBuf {
        self.path.borrow().clone()
    }

    pub fn is_csharp(&self) -> bool {
        self.path().extension().is_some_and(|e| e == "cs")
    }

    pub fn cursor_offset(&self) -> i32 {
        self.buffer.iter_at_mark(&self.buffer.get_insert()).offset()
    }

    /// Byte offset of the caret in [`CodeDoc::text`].
    pub fn cursor_byte(&self) -> usize {
        let it = self.buffer.iter_at_mark(&self.buffer.get_insert());
        let start = self.buffer.start_iter();
        self.buffer.text(&start, &it, true).len()
    }

    pub fn iter_at_byte(&self, byte: usize) -> gtk::TextIter {
        let text = self.text();
        self.byte_iters(&text)(byte)
    }

    /// Iterators for byte offsets of `text` (this buffer's text, taken
    /// once): many lookups without copying the buffer for each.
    pub fn byte_iters<'a>(&'a self, text: &'a str) -> impl Fn(usize) -> gtk::TextIter + 'a {
        let index = anvil_core::text::LineIndex::new(text);
        move |byte| {
            let (line, at) = index.locate(text, byte);
            self.buffer.iter_at_line_index(line as i32, at as i32).unwrap_or_else(|| self.buffer.end_iter())
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let file = TextFile { encoding: self.encoding.get(), eol: self.eol.get(), text: self.text() };
        file.write(&self.path())?;
        self.buffer.set_modified(false);
        Ok(())
    }

    /// Replaces the whole text (an external change) as one undo step.
    pub fn replace_text(&self, text: &str) {
        self.quiet.set(true);
        self.buffer.begin_user_action();
        let (mut s, mut e) = self.buffer.bounds();
        self.buffer.delete(&mut s, &mut e);
        let mut s = self.buffer.start_iter();
        self.buffer.insert(&mut s, text);
        self.buffer.end_user_action();
        self.quiet.set(false);
    }

    /// Applies byte-range edits (from the planner or the language server)
    /// as one undo step, back to front.
    pub fn apply_edits(&self, edits: &[anvil_core::edit::TextEdit]) {
        let mut sorted: Vec<&anvil_core::edit::TextEdit> = edits.iter().collect();
        sorted.sort_by_key(|e| std::cmp::Reverse(e.start));
        // Back to front, so the text before each edit is still the text the
        // index was built from.
        let text = self.text();
        let at = self.byte_iters(&text);
        self.buffer.begin_user_action();
        for e in sorted {
            let mut s = at(e.start);
            let mut end = at(e.end.max(e.start));
            self.buffer.delete(&mut s, &mut end);
            self.buffer.insert(&mut s, &e.text);
        }
        self.buffer.end_user_action();
    }

    pub fn goto_line_col(&self, line: i32, col: i32) {
        let Some(mut it) = self.buffer.iter_at_line(line.max(0)) else { return };
        it.forward_chars(col.max(0));
        self.buffer.place_cursor(&it);
        let mark = self.buffer.get_insert();
        if !self.smooth.scroll_to_iter(&it, 0.35) {
            self.view.scroll_to_mark(&mark, 0.15, true, 0.0, 0.35);
        }
        self.view.grab_focus();
    }

    pub fn selection_text(&self) -> String {
        self.buffer.selection_bounds().map(|(s, e)| self.buffer.text(&s, &e, true).to_string()).unwrap_or_default()
    }

    /// The selected lines (whole lines) as iterators.
    fn line_range(&self) -> (gtk::TextIter, gtk::TextIter) {
        let (mut s, mut e) = self.buffer.selection_bounds().unwrap_or_else(|| {
            let it = self.buffer.iter_at_mark(&self.buffer.get_insert());
            (it, it)
        });
        if e.starts_line() && e.offset() > s.offset() {
            e.backward_char();
        }
        s.set_line_offset(0);
        if !e.ends_line() {
            e.forward_to_line_end();
        }
        (s, e)
    }

    pub fn comment(&self, add: Option<bool>) {
        let (s, e) = self.line_range();
        let (first, last) = (s.line(), e.line());
        let lines: Vec<String> = (first..=last)
            .map(|l| {
                let a = self.buffer.iter_at_line(l).unwrap_or(s);
                let mut b = a;
                if !b.ends_line() {
                    b.forward_to_line_end();
                }
                self.buffer.text(&a, &b, true).to_string()
            })
            .collect();
        let all_commented = lines.iter().filter(|l| !l.trim().is_empty()).all(|l| l.trim_start().starts_with("//"));
        let add = add.unwrap_or(!all_commented);
        let indent = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
        self.buffer.begin_user_action();
        for (i, l) in lines.iter().enumerate() {
            let line = first + i as i32;
            if l.trim().is_empty() {
                continue;
            }
            let Some(mut a) = self.buffer.iter_at_line(line) else { continue };
            if add {
                a.forward_chars(indent as i32);
                self.buffer.insert(&mut a, "// ");
            } else if let Some(pos) = l.find("//") {
                let width = if l[pos..].starts_with("// ") { 3 } else { 2 };
                a.forward_chars(l[..pos].chars().count() as i32);
                let mut b = a;
                b.forward_chars(width);
                self.buffer.delete(&mut a, &mut b);
            }
        }
        self.buffer.end_user_action();
    }

    pub fn duplicate(&self) {
        let (mut s, mut e) = match self.buffer.selection_bounds() {
            Some(r) => r,
            None => self.line_range(),
        };
        let had_selection = self.buffer.has_selection();
        let text = self.buffer.text(&s, &e, true).to_string();
        self.buffer.begin_user_action();
        if had_selection {
            self.buffer.insert(&mut e, &text);
        } else {
            let mut end = e;
            self.buffer.insert(&mut end, &format!("\n{text}"));
        }
        self.buffer.end_user_action();
        s.set_line_offset(0);
    }

    pub fn delete_line(&self, cut: bool) {
        let (mut s, mut e) = self.line_range();
        e.forward_line();
        if cut {
            let text = self.buffer.text(&s, &e, true).to_string();
            self.view.clipboard().set_text(&text);
        }
        self.buffer.begin_user_action();
        self.buffer.delete(&mut s, &mut e);
        self.buffer.end_user_action();
    }

    pub fn change_case(&self, upper: bool) {
        let Some((mut s, mut e)) = self.buffer.selection_bounds() else { return };
        let text = self.buffer.text(&s, &e, true).to_string();
        let new = if upper { text.to_uppercase() } else { text.to_lowercase() };
        let (so, eo) = (s.offset(), s.offset() + new.chars().count() as i32);
        self.buffer.begin_user_action();
        self.buffer.delete(&mut s, &mut e);
        self.buffer.insert(&mut s, &new);
        self.buffer.end_user_action();
        let (a, b) = (self.buffer.iter_at_offset(so), self.buffer.iter_at_offset(eo));
        self.buffer.select_range(&a, &b);
    }

    pub fn goto_brace(&self) {
        let it = self.buffer.iter_at_mark(&self.buffer.get_insert());
        let text = self.text();
        let byte = self.cursor_byte();
        let bytes = text.as_bytes();
        let pick = |i: usize| bytes.get(i).copied().filter(|b| b"{}()[]".contains(b));
        let (pos, ch) = match pick(byte).map(|c| (byte, c)).or_else(|| byte.checked_sub(1).and_then(|b| pick(b).map(|c| (b, c)))) {
            Some(x) => x,
            None => return,
        };
        let (open, close, forward) = match ch {
            b'{' => (b'{', b'}', true),
            b'(' => (b'(', b')', true),
            b'[' => (b'[', b']', true),
            b'}' => (b'{', b'}', false),
            b')' => (b'(', b')', false),
            _ => (b'[', b']', false),
        };
        let mut depth = 0i32;
        let target = if forward {
            (pos..bytes.len()).find(|&i| {
                if bytes[i] == open {
                    depth += 1;
                } else if bytes[i] == close {
                    depth -= 1;
                }
                depth == 0
            })
        } else {
            (0..=pos).rev().find(|&i| {
                if bytes[i] == close {
                    depth += 1;
                } else if bytes[i] == open {
                    depth -= 1;
                }
                depth == 0
            })
        };
        if let Some(t) = target {
            let it2 = self.iter_at_byte(t);
            self.buffer.place_cursor(&it2);
            self.view.scroll_mark_onscreen(&self.buffer.get_insert());
        }
        let _ = it;
    }

    /// Draws breakpoints as `(line, kind)`; the marks then move with the
    /// text when lines are inserted or removed above them.
    pub fn set_breakpoint_marks(&self, marks: &[(u32, &'static str)]) {
        let (s, e) = self.buffer.bounds();
        for kind in BREAKPOINT_KINDS {
            self.buffer.remove_source_marks(&s, &e, Some(kind));
        }
        let mut kept = self.bp_marks.borrow_mut();
        kept.clear();
        for &(l, kind) in marks {
            if let Some(it) = self.buffer.iter_at_line(l as i32 - 1) {
                kept.push(self.buffer.create_source_mark(None, kind, &it));
            }
        }
    }

    /// The 1-based lines of the breakpoint marks, top to bottom.
    pub fn breakpoint_mark_lines(&self) -> Vec<u32> {
        let mut lines: Vec<u32> = self.bp_marks.borrow().iter().filter(|m| !m.is_deleted()).map(|m| self.buffer.iter_at_mark(m).line() as u32 + 1).collect();
        lines.sort_unstable();
        lines
    }

    /// `prop|` + Tab: the word before the caret, when it is a snippet's
    /// shortcut and stands alone, becomes the snippet.
    fn expand_snippet_at_caret(&self) -> bool {
        if !self.is_csharp() || self.buffer.has_selection() {
            return false;
        }
        let end = self.buffer.iter_at_mark(&self.buffer.get_insert());
        let mut start = end;
        while start.backward_char() {
            let c = start.char();
            if !(c.is_alphanumeric() || c == '~') {
                start.forward_char();
                break;
            }
        }
        let word = self.buffer.text(&start, &end, false).to_string();
        let Some(def) = anvil_core::snippets::find(&word) else { return false };
        // Only at the start of a statement: `x.for` + Tab is not a loop.
        let mut before = start;
        let prev = if before.backward_char() { before.char() } else { ' ' };
        if !(prev.is_whitespace() || prev == '{' || prev == ';' || prev == '}') {
            return false;
        }
        self.buffer.select_range(&start, &end);
        crate::ui::dialogs::expand_snippet(self, def);
        true
    }

    /// Selects the next field of the snippet; `false` when there is none.
    fn next_snippet_stop(&self) -> bool {
        let next = {
            let mut s = self.snippet.borrow_mut();
            let Some((stops, _)) = s.as_mut() else { return false };
            if stops.is_empty() { None } else { Some(stops.remove(0)) }
        };
        match next {
            Some((a, b)) => {
                self.buffer.select_range(&self.buffer.iter_at_mark(&a), &self.buffer.iter_at_mark(&b));
                self.buffer.delete_mark(&a);
                self.buffer.delete_mark(&b);
                true
            }
            None => {
                self.finish_snippet();
                true
            }
        }
    }

    /// The caret goes to the snippet's `$end$`.
    pub fn finish_snippet(&self) {
        let Some((stops, end)) = self.snippet.borrow_mut().take() else { return };
        for (a, b) in stops {
            self.buffer.delete_mark(&a);
            self.buffer.delete_mark(&b);
        }
        self.buffer.place_cursor(&self.buffer.iter_at_mark(&end));
        self.buffer.delete_mark(&end);
    }

    pub fn set_debug_line(&self, line: Option<u32>) {
        let (s, e) = self.buffer.bounds();
        self.buffer.remove_source_marks(&s, &e, Some(CURRENT_LINE));
        self.view.set_current_line(line.map(|l| l as i32 - 1));
        if let Some(l) = line
            && let Some(it) = self.buffer.iter_at_line(l as i32 - 1)
        {
            self.buffer.create_source_mark(None, CURRENT_LINE, &it);
            self.goto_line_col(l as i32 - 1, 0);
        }
    }
}

pub enum Doc {
    Code(Rc<CodeDoc>),
    Diagram(Rc<crate::ui::designer::DiagramDoc>),
}

impl Doc {
    pub fn path(&self) -> PathBuf {
        match self {
            Doc::Code(c) => c.path(),
            Doc::Diagram(d) => d.path.borrow().clone(),
        }
    }

    pub fn widget(&self) -> gtk::Widget {
        match self {
            Doc::Code(c) => c.root.clone().upcast(),
            Doc::Diagram(d) => d.widget.clone().upcast(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        match self {
            Doc::Code(c) => c.buffer.is_modified(),
            Doc::Diagram(d) => d.is_dirty(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        match self {
            Doc::Code(c) => c.save(),
            Doc::Diagram(d) => d.save(),
        }
    }
}

// ---- Tabs ----------------------------------------------------------------

pub struct Tabs {
    pub widget: gtk::Box,
    pub view: adw::TabView,
    docs: RefCell<Vec<(adw::TabPage, Rc<Doc>)>>,
    preview: RefCell<Option<adw::TabPage>>,
    /// VS' navigation history: places left by a jump, and the position in
    /// it for Navigate Forward.
    history: RefCell<(Vec<(PathBuf, i32)>, usize)>,
}

impl Tabs {
    pub fn new() -> Tabs {
        let view = adw::TabView::new();
        view.set_vexpand(true);
        let bar = adw::TabBar::new();
        bar.set_view(Some(&view));
        bar.set_autohide(false);
        bar.add_css_class("editor-tabs");
        // One card holds the tabs and the editor, as the side and bottom
        // panels hold theirs.
        bar.set_expand_tabs(false);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("card-panel");
        widget.add_css_class("editor-card");
        widget.set_overflow(gtk::Overflow::Hidden);
        widget.append(&bar);
        widget.append(&view);
        widget.set_vexpand(true);
        Tabs { widget, view, docs: RefCell::default(), preview: RefCell::default(), history: RefCell::default() }
    }

    pub fn connect(&self) {
        self.view.connect_close_page(|view, page| {
            let app = crate::ui::window::app();
            let doc = app.tabs.doc_of(page);
            let dirty = doc.as_ref().is_some_and(|d| d.is_dirty());
            if !dirty {
                app.tabs.forget(page);
                view.close_page_finish(page, true);
                return glib::Propagation::Stop;
            }
            let name = doc.as_ref().map(|d| d.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).unwrap_or_default();
            let dialog = adw::AlertDialog::new(Some(&trf("Save changes to {}?", &[&name])), None);
            dialog.add_responses(&[("cancel", &tr("Cancel")), ("discard", &tr("Don't Save")), ("save", &tr("Save"))]);
            dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("save"));
            let page = page.clone();
            let view = view.clone();
            dialog.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
                let app = crate::ui::window::app();
                match r.as_str() {
                    "save" => {
                        // A failed save keeps the tab open with its text.
                        if let Some(d) = app.tabs.doc_of(&page)
                            && let Err(e) = d.save()
                        {
                            view.close_page_finish(&page, false);
                            crate::ui::window::show_error(&trf("{} could not be saved", &[&d.path().display().to_string()]), &e.to_string());
                            return;
                        }
                        app.tabs.forget(&page);
                        view.close_page_finish(&page, true);
                    }
                    "discard" => {
                        app.tabs.forget(&page);
                        view.close_page_finish(&page, true);
                    }
                    _ => view.close_page_finish(&page, false),
                }
            });
            glib::Propagation::Stop
        });
        self.view.connect_selected_page_notify(|_| {
            let app = crate::ui::window::app();
            match app.tabs.current_code() {
                Some(c) => {
                    let it = c.buffer.iter_at_mark(&c.buffer.get_insert());
                    app.status.set_position(it.line() + 1, it.line_offset() + 1);
                    app.explorer.reveal(&c.path());
                }
                None => app.status.clear_position(),
            }
        });
        adw::StyleManager::default().connect_dark_notify(|_| {
            let app = crate::ui::window::app();
            for (_, d) in app.tabs.docs.borrow().iter() {
                if let Doc::Code(c) = &**d {
                    c.buffer.set_style_scheme(scheme().as_ref());
                    style_highlight_tags(&c.buffer);
                }
            }
        });
    }

    fn doc_of(&self, page: &adw::TabPage) -> Option<Rc<Doc>> {
        self.docs.borrow().iter().find(|(p, _)| p == page).map(|(_, d)| d.clone())
    }

    fn forget(&self, page: &adw::TabPage) {
        let doc = self.doc_of(page);
        self.docs.borrow_mut().retain(|(p, _)| p != page);
        if self.preview.borrow().as_ref() == Some(page) {
            *self.preview.borrow_mut() = None;
        }
        if let Some(d) = doc
            && let Doc::Code(c) = &*d
        {
            crate::ui::window::app().lsp.did_close(&c.path());
        }
        if self.docs.borrow().is_empty() && crate::ui::window::app().solution.borrow().is_none() {
            crate::ui::window::app().center.set_visible_child_name("start");
        }
    }

    pub fn find(&self, path: &Path) -> Option<(adw::TabPage, Rc<Doc>)> {
        self.docs.borrow().iter().find(|(_, d)| d.path() == path).cloned()
    }

    pub fn current(&self) -> Option<Rc<Doc>> {
        let page = self.view.selected_page()?;
        self.doc_of(&page)
    }

    pub fn current_code(&self) -> Option<Rc<CodeDoc>> {
        match &*self.current()? {
            Doc::Code(c) => Some(c.clone()),
            Doc::Diagram(_) => None,
        }
    }

    pub fn current_path(&self) -> Option<PathBuf> {
        self.current().map(|d| d.path())
    }

    pub fn code_docs(&self) -> Vec<Rc<CodeDoc>> {
        self.docs.borrow().iter().filter_map(|(_, d)| if let Doc::Code(c) = &**d { Some(c.clone()) } else { None }).collect()
    }

    pub fn code_doc(&self, path: &Path) -> Option<Rc<CodeDoc>> {
        self.code_docs().into_iter().find(|c| c.path() == path)
    }

    pub fn open_paths(&self) -> Vec<PathBuf> {
        self.docs.borrow().iter().map(|(_, d)| d.path()).collect()
    }

    pub fn dirty_names(&self) -> Vec<String> {
        self.docs.borrow().iter().filter(|(_, d)| d.is_dirty()).map(|(_, d)| d.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect()
    }

    /// Saves every changed tab; `false` when one could not be saved (the
    /// error is shown, the tab keeps its text).
    pub fn save_all(&self) -> bool {
        let docs: Vec<Rc<Doc>> = self.docs.borrow().iter().map(|(_, d)| d.clone()).collect();
        let mut ok = true;
        for d in docs {
            if d.is_dirty()
                && let Err(e) = d.save()
            {
                ok = false;
                crate::ui::window::show_error(&trf("{} could not be saved", &[&d.path().display().to_string()]), &e.to_string());
            }
        }
        self.refresh_titles();
        ok
    }

    pub fn save_current(&self) {
        if let Some(d) = self.current() {
            match d.save() {
                Ok(()) => {
                    if let Doc::Code(c) = &*d {
                        crate::ui::window::app().lsp.did_save(c);
                    }
                }
                Err(e) => crate::ui::window::show_error(&trf("{} could not be saved", &[&d.path().display().to_string()]), &e.to_string()),
            }
        }
        self.refresh_titles();
    }

    pub fn mark_all_clean(&self) {
        for (_, d) in self.docs.borrow().iter() {
            match &**d {
                Doc::Code(c) => c.buffer.set_modified(false),
                Doc::Diagram(g) => g.mark_clean(),
            }
        }
    }

    pub fn refresh_titles(&self) {
        for (page, d) in self.docs.borrow().iter() {
            let name = d.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            page.set_title(&if d.is_dirty() { format!("{name}*") } else { name });
            page.set_tooltip(&d.path().display().to_string());
        }
    }

    pub fn close_all(&self) {
        // Forgotten pages are not dirty for the close handler, so they
        // close without asking (the caller saved or asked already).
        let pages: Vec<adw::TabPage> = self.docs.borrow().iter().map(|(p, _)| p.clone()).collect();
        for p in pages {
            self.forget(&p);
            self.view.close_page(&p);
        }
    }

    pub fn close_current(&self) {
        if let Some(p) = self.view.selected_page() {
            self.view.close_page(&p);
        }
    }

    /// Opens (or shows) a file. A preview tab is replaced by the next
    /// preview, as VS' single-click preview.
    pub fn open(&self, path: &Path, preview: bool) -> Option<Rc<Doc>> {
        let app = crate::ui::window::app();
        app.center.set_visible_child_name("work");
        if let Some((page, doc)) = self.find(path) {
            if !preview && self.preview.borrow().as_ref() == Some(&page) {
                *self.preview.borrow_mut() = None;
                page.child().remove_css_class("preview");
            }
            self.view.set_selected_page(&page);
            return Some(doc);
        }
        let doc = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cd")) {
            match crate::ui::designer::DiagramDoc::open(path) {
                Ok(d) => Rc::new(Doc::Diagram(d)),
                Err(e) => {
                    crate::ui::window::show_error(&tr("The class diagram could not be opened"), &e);
                    return None;
                }
            }
        } else {
            match new_code_doc(path) {
                Ok(c) => Rc::new(Doc::Code(c)),
                Err(e) => {
                    crate::ui::window::show_error(&trf("{} could not be opened", &[&path.display().to_string()]), &e.to_string());
                    return None;
                }
            }
        };
        // Take the old preview first: `forget` borrows `preview` again.
        let old_preview = if preview { self.preview.borrow_mut().take() } else { None };
        if let Some(old) = old_preview {
            let dirty = self.doc_of(&old).is_some_and(|d| d.is_dirty());
            if !dirty {
                self.forget(&old);
                self.view.close_page(&old);
            }
        }
        let page = self.view.append(&doc.widget());
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        page.set_title(&name);
        page.set_tooltip(&path.display().to_string());
        if preview {
            *self.preview.borrow_mut() = Some(page.clone());
        }
        self.docs.borrow_mut().push((page.clone(), doc.clone()));
        self.view.set_selected_page(&page);
        if let Doc::Code(c) = &*doc {
            app.lsp.did_open(c);
            app.debugger.apply_marks(c);
            c.view.grab_focus();
        }
        Some(doc)
    }

    fn here(&self) -> Option<(PathBuf, i32)> {
        let c = self.current_code()?;
        Some((c.path(), c.buffer.iter_at_mark(&c.buffer.get_insert()).line()))
    }

    /// Navigate Backward (`Ctrl+-`) and Forward (`Ctrl+Shift+-`).
    pub fn navigate(&self, back: bool) {
        let here = self.here();
        let target = {
            let mut h = self.history.borrow_mut();
            let (list, pos) = &mut *h;
            if back {
                if *pos == 0 {
                    return;
                }
                if *pos == list.len()
                    && let Some(here) = here
                {
                    list.push(here);
                }
                *pos -= 1;
                list[*pos].clone()
            } else {
                if *pos + 1 >= list.len() {
                    return;
                }
                *pos += 1;
                list[*pos].clone()
            }
        };
        if let Some(d) = self.open(&target.0, false)
            && let Doc::Code(c) = &*d
        {
            c.goto_line_col(target.1, 0);
        }
    }

    /// Opens a file at a 1-based line and column, remembering where the
    /// caret was (for Navigate Backward).
    pub fn goto(&self, path: &Path, line: u32, col: u32) {
        if let Some(here) = self.here() {
            let mut h = self.history.borrow_mut();
            let pos = h.1;
            h.0.truncate(pos);
            h.0.push(here);
            h.1 = h.0.len();
        }
        if let Some(d) = self.open(path, false)
            && let Doc::Code(c) = &*d
        {
            c.goto_line_col(line as i32 - 1, col.saturating_sub(1) as i32);
        }
    }

    pub fn promote_preview(&self) {
        self.preview.borrow_mut().take();
    }
}

fn new_code_doc(path: &Path) -> std::io::Result<Rc<CodeDoc>> {
    let file = TextFile::read(path)?;
    let buffer = sourceview5::Buffer::new(None);
    buffer.set_language(language_for(path).as_ref());
    buffer.set_style_scheme(scheme().as_ref());
    buffer.set_highlight_matching_brackets(true);
    buffer.set_implicit_trailing_newline(false);
    buffer.begin_irreversible_action();
    buffer.set_text(&file.text);
    buffer.end_irreversible_action();
    buffer.set_modified(false);
    buffer.place_cursor(&buffer.start_iter());
    create_tags(&buffer);

    let view = AnvilView::new(&buffer);
    view.add_css_class("editor");
    view.set_monospace(true);
    view.set_show_line_numbers(true);
    view.set_highlight_current_line(true);
    view.set_auto_indent(true);
    view.set_indent_on_tab(true);
    view.set_tab_width(4);
    view.set_indent_width(4);
    view.set_insert_spaces_instead_of_tabs(!file.text.lines().any(|l| l.starts_with('\t')));
    view.set_smart_backspace(true);
    view.set_smart_home_end(sourceview5::SmartHomeEndType::Before);
    view.set_show_line_marks(true);
    view.set_left_margin(8);
    view.set_top_margin(6);
    view.set_bottom_margin(120);
    let settings = &crate::ui::window::app().settings;
    view.set_wrap_mode(if settings.boolean("word-wrap") { gtk::WrapMode::WordChar } else { gtk::WrapMode::None });
    set_whitespace(&view, settings.boolean("show-whitespace"));

    let tips = [
        (BREAKPOINT, tr("Breakpoint")),
        (BREAKPOINT_DISABLED, tr("Disabled breakpoint")),
        (BREAKPOINT_CONDITION, tr("Breakpoint with a condition or hit count")),
        (BREAKPOINT_PENDING, tr("The breakpoint will not currently be hit: no code was found at this line.")),
    ];
    for (kind, tip) in tips {
        let a = sourceview5::MarkAttributes::new();
        a.set_gicon(&gutter_icon(kind));
        a.connect_query_tooltip_text(move |_, _| tip.clone());
        view.set_mark_attributes(kind, &a, 10);
    }
    let cur = sourceview5::MarkAttributes::new();
    cur.set_gicon(&gutter_icon(CURRENT_LINE));
    view.set_mark_attributes(CURRENT_LINE, &cur, 20);

    let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).hexpand(true).build();
    let root = gtk::Overlay::new();
    root.set_child(Some(&scroller));
    let smooth = crate::ui::smooth::Smooth::attach(&view, &scroller, &root);
    let find = crate::ui::find::FindBar::new(&buffer, view.upcast_ref());
    root.add_overlay(&find.widget);

    let doc = Rc::new(CodeDoc {
        path: RefCell::new(path.to_owned()),
        buffer: buffer.clone(),
        view: view.clone(),
        root,
        find,
        encoding: Cell::new(file.encoding),
        eol: Cell::new(file.eol),
        version: Cell::new(1),
        quiet: Cell::new(false),
        diagnostics: RefCell::default(),
        change_serial: Cell::new(0),
        highlights: RefCell::default(),
        highlight_timer: RefCell::default(),
        bp_marks: RefCell::default(),
        snippet: RefCell::default(),
        smooth,
    });
    doc.highlight_now();

    // Typing turns a preview tab into a normal one, as in VS.
    let weak = Rc::downgrade(&doc);
    buffer.connect_changed(move |_| {
        let Some(doc) = weak.upgrade() else { return };
        let _s = crate::frame_log::span("buffer changed");
        doc.version.set(doc.version.get() + 1);
        doc.change_serial.set(doc.change_serial.get() + 1);
        doc.schedule_highlight();
        let app = crate::ui::window::app();
        if !doc.quiet.get() {
            app.tabs.promote_preview();
        }
        app.lsp.did_change(&doc);
        if app.debugger.has_breakpoints(&doc.path()) {
            app.debugger.track(&doc);
        }
        crate::ui::lsp::schedule_ai(&doc);
        crate::ui::designer::code_changed(&doc.path());
    });
    let weak = Rc::downgrade(&doc);
    buffer.connect_modified_changed(move |_| {
        if weak.upgrade().is_some() {
            crate::ui::window::app().tabs.refresh_titles();
        }
    });
    buffer.connect_cursor_position_notify(|b| {
        let it = b.iter_at_mark(&b.get_insert());
        crate::ui::window::app().status.set_position(it.line() + 1, it.line_offset() + 1);
    });
    let weak = Rc::downgrade(&doc);
    view.connect_line_mark_activated(move |_, it, button, _, _| {
        if button == gdk::BUTTON_PRIMARY
            && let Some(doc) = weak.upgrade()
        {
            crate::ui::window::app().debugger.toggle_breakpoint(&doc.path(), it.line() as u32 + 1);
        }
    });
    // Snippets as in VS: `prop` + Tab expands, Tab moves to the next field,
    // Enter or Escape ends it.
    let weak = Rc::downgrade(&doc);
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(move |_, key, _, state| {
        let Some(doc) = weak.upgrade() else { return glib::Propagation::Proceed };
        let plain = !state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SHIFT_MASK);
        match key {
            gdk::Key::Tab if plain => {
                if doc.next_snippet_stop() {
                    return glib::Propagation::Stop;
                }
                if !crate::ui::lsp::completion_visible(&doc) && doc.expand_snippet_at_caret() {
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::Escape if doc.snippet.borrow().is_some() && !crate::ui::lsp::completion_visible(&doc) => {
                doc.finish_snippet();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    view.add_controller(keys);
    // Moving the caret or clicking drops the ghost text.
    let weak = Rc::downgrade(&doc);
    let click = gtk::GestureClick::new();
    click.connect_pressed(move |_, _, _, _| {
        if let Some(d) = weak.upgrade() {
            d.view.set_ghost(None);
        }
    });
    view.add_controller(click);
    crate::ui::lsp::attach_providers(&doc);
    crate::ui::lsp::attach_context_menu(&doc);
    Ok(doc)
}

fn create_tags(buffer: &sourceview5::Buffer) {
    let p = palette::current();
    let table = buffer.tag_table();
    let error = gtk::TextTag::new(Some("diag-error"));
    error.set_underline(gtk::pango::Underline::Error);
    error.set_underline_rgba(Some(&p.down));
    table.add(&error);
    // Warnings are told apart by shape: a plain line in ink2, not amber.
    let warning = gtk::TextTag::new(Some("diag-warning"));
    warning.set_underline(gtk::pango::Underline::Single);
    warning.set_underline_rgba(Some(&p.ink2));
    table.add(&warning);
    let info = gtk::TextTag::new(Some("diag-info"));
    info.set_underline(gtk::pango::Underline::Single);
    info.set_underline_rgba(Some(&p.ink3));
    table.add(&info);
    let semantic_type = gtk::TextTag::new(Some("sem-type"));
    semantic_type.set_weight(600);
    table.add(&semantic_type);
    for (name, kind, _) in HIGHLIGHT_TAGS {
        let tag = gtk::TextTag::new(Some(name));
        // The italic comes from the scheme (`anvil:type`, `anvil:method`);
        // the cursive letter forms from the font feature.
        if kind != highlight::Kind::Property {
            tag.set_font_features(Some(NAME_FEATURES));
        }
        table.add(&tag);
    }
    style_highlight_tags(buffer);
}

/// Colours of the name tags come from the style scheme (`anvil:type` and
/// so on), so light and dark live in one place.
fn style_highlight_tags(buffer: &sourceview5::Buffer) {
    let Some(scheme) = buffer.style_scheme() else { return };
    for (name, _, style) in HIGHLIGHT_TAGS {
        if let (Some(tag), Some(style)) = (buffer.tag_table().lookup(name), scheme.style(style)) {
            style.apply(&tag);
            if name != "hl-property" {
                tag.set_font_features(Some(NAME_FEATURES));
            }
        }
    }
}

pub fn set_whitespace(view: &AnvilView, on: bool) {
    let drawer = view.space_drawer();
    drawer.set_types_for_locations(sourceview5::SpaceLocationFlags::ALL, if on { sourceview5::SpaceTypeFlags::SPACE | sourceview5::SpaceTypeFlags::TAB } else { sourceview5::SpaceTypeFlags::NONE });
    drawer.set_enable_matrix(on);
}

/// Keys the editor itself owns before the keymap: Tab accepts ghost text
/// (unless the completion list is open, where Tab belongs to IntelliSense),
/// Ctrl+Right accepts one word, Escape dismisses.
pub fn handle_editor_key(combo: &Combo) -> bool {
    let Some(doc) = crate::ui::window::app().tabs.current_code() else { return false };
    let Some((offset, text)) = doc.view.ghost() else { return false };
    let completion_open = crate::ui::lsp::completion_visible(&doc);
    match (combo.ctrl, combo.shift, combo.alt, combo.key.as_str()) {
        (false, false, false, "Tab") if !completion_open => {
            doc.view.set_ghost(None);
            let mut it = doc.buffer.iter_at_offset(offset);
            doc.buffer.begin_user_action();
            doc.buffer.insert(&mut it, &text);
            doc.buffer.end_user_action();
            true
        }
        (true, false, false, "Right") => {
            let word_end = text.char_indices().skip(1).find(|(_, c)| !c.is_alphanumeric() && *c != '_').map_or(text.len(), |(i, _)| i);
            let (word, rest) = text.split_at(word_end);
            let mut it = doc.buffer.iter_at_offset(offset);
            doc.quiet.set(true);
            doc.buffer.insert(&mut it, word);
            doc.quiet.set(false);
            let next = offset + word.chars().count() as i32;
            doc.view.set_ghost((!rest.is_empty()).then(|| (next, rest.to_owned())));
            true
        }
        (false, false, false, "Escape") => {
            doc.view.set_ghost(None);
            true
        }
        _ => {
            doc.view.set_ghost(None);
            false
        }
    }
}

/// Go To Line (`Ctrl+G`).
pub fn goto_line_dialog() {
    let app = crate::ui::window::app();
    let Some(doc) = app.tabs.current_code() else { return };
    let total = doc.buffer.line_count();
    let dialog = adw::AlertDialog::new(Some(&tr("Go To Line")), Some(&trf("Line number (1 - {}):", &[&total.to_string()])));
    let entry = gtk::Entry::new();
    entry.set_input_purpose(gtk::InputPurpose::Digits);
    let it = doc.buffer.iter_at_mark(&doc.buffer.get_insert());
    entry.set_text(&(it.line() + 1).to_string());
    entry.set_activates_default(true);
    dialog.set_extra_child(Some(&entry));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("ok", &tr("OK"))]);
    dialog.set_default_response(Some("ok"));
    dialog.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
        if r == "ok"
            && let Ok(n) = entry.text().trim().parse::<i32>()
        {
            doc.goto_line_col(n.clamp(1, total) - 1, 0);
        }
    });
}
