//! VS-Code-like motion in the code editor: the mouse wheel and Page Up/Down
//! glide (150 ms, ease-out) instead of jumping, jumps to a line glide to it,
//! and the caret is drawn by Anvil on a layer above the text so it can fade
//! when it blinks and slide when it moves. Touchpads keep GTK's own kinetic
//! scrolling. Settings → Editor → "Smooth scrolling and caret" (and the
//! system's "reduce animations") turn all of it off.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene};

use crate::motion::{self, Blink};

pub const KEY: &str = "smooth-motion";

/// Smooth motion is on: Anvil's setting and the system's animations.
pub fn enabled() -> bool {
    let system = gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations());
    system && crate::ui::window::app().settings.boolean(KEY)
}

fn now_ms() -> f64 {
    glib::monotonic_time() as f64 / 1000.0
}

/// GTK's caret blink as the user set it, read before Anvil turns GTK's own
/// blinking off (Anvil's caret blinks instead; GTK's, now transparent,
/// would still repaint the whole text every half second).
fn system_blink() -> Blink {
    thread_local! {
        static BLINK: Blink = {
            let s = gtk::Settings::default();
            Blink {
                on: s.as_ref().is_none_or(|s| s.is_gtk_cursor_blink()),
                cycle_ms: s.as_ref().map_or(1200, |s| s.gtk_cursor_blink_time()) as f64,
                timeout_ms: s.as_ref().map_or(10, |s| s.gtk_cursor_blink_timeout()) as f64 * 1000.0,
            }
        };
    }
    BLINK.with(|b| *b)
}

/// Hides GTK's caret in views drawn with Anvil's (once per display).
fn install_css() {
    thread_local! {
        static DONE: Cell<bool> = const { Cell::new(false) };
    }
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let css = gtk::CssProvider::new();
    css.load_from_string("textview.anvil-smooth-caret > text { caret-color: transparent; -gtk-secondary-caret-color: transparent; }");
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
    }
}

// ---- Scrolling ---------------------------------------------------------------

#[derive(Default)]
struct Glide {
    from: f64,
    to: f64,
    start: f64,
    /// The value the last frame set; anything else moved the view meanwhile
    /// (a scrollbar drag, GTK following the caret) and ends the glide.
    last: f64,
    /// After the glide, let GTK bring the caret fully into view (columns,
    /// and line heights GTK had only estimated).
    settle: bool,
    tick: Option<gtk::TickCallbackId>,
}

pub struct Smooth {
    view: gtk::TextView,
    adj: gtk::Adjustment,
    glide: Rc<RefCell<Glide>>,
    caret: Caret,
}

impl Smooth {
    /// Adds smooth scrolling to `scroller` (holding `view`) and the caret
    /// layer to `overlay` (whose child is `scroller`).
    pub fn attach(view: &impl IsA<gtk::TextView>, scroller: &gtk::ScrolledWindow, overlay: &gtk::Overlay) -> Rc<Smooth> {
        let view: gtk::TextView = view.clone().upcast();
        install_css();
        let caret = Caret::new(&view);
        overlay.add_overlay(&caret);
        let smooth = Rc::new(Smooth { view: view.clone(), adj: scroller.vadjustment(), glide: Rc::default(), caret });

        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&smooth);
        wheel.connect_scroll(move |c, _, dy| {
            let Some(s) = weak.upgrade() else { return glib::Propagation::Proceed };
            let mods = c.current_event_state();
            // Touchpads (surface units) scroll kinetically already; Ctrl
            // and Shift change what the wheel means.
            if c.unit() != gdk::ScrollUnit::Wheel || dy == 0.0 || mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK) || !enabled() {
                return glib::Propagation::Proceed;
            }
            // GTK's own wheel step, so the distance is the one users know.
            let step = s.adj.page_size().powf(2.0 / 3.0);
            s.glide_to(s.target() + dy * step, false);
            glib::Propagation::Stop
        });
        scroller.add_controller(wheel);

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&smooth);
        keys.connect_key_pressed(move |_, key, _, state| {
            let down = match key {
                gdk::Key::Page_Down | gdk::Key::KP_Page_Down => true,
                gdk::Key::Page_Up | gdk::Key::KP_Page_Up => false,
                _ => return glib::Propagation::Proceed,
            };
            let Some(s) = weak.upgrade() else { return glib::Propagation::Proceed };
            if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) || !enabled() || crate::ui::lsp::completion_open() {
                return glib::Propagation::Proceed;
            }
            s.page(down, state.contains(gdk::ModifierType::SHIFT_MASK));
            glib::Propagation::Stop
        });
        view.add_controller(keys);

        let weak = Rc::downgrade(&smooth);
        crate::ui::window::app().settings.connect_changed(Some(KEY), move |_, _| {
            if let Some(s) = weak.upgrade() {
                s.caret.set_enabled(enabled());
            }
        });
        smooth.caret.set_enabled(enabled());
        smooth
    }

    /// Where the view is going: the glide's end, or where it is.
    fn target(&self) -> f64 {
        let g = self.glide.borrow();
        if g.tick.is_some() { g.to } else { self.adj.value() }
    }

    fn clamp(&self, v: f64) -> f64 {
        v.clamp(self.adj.lower(), (self.adj.upper() - self.adj.page_size()).max(self.adj.lower()))
    }

    fn glide_to(&self, target: f64, settle: bool) {
        let to = self.clamp(target);
        let mut g = self.glide.borrow_mut();
        let value = self.adj.value();
        g.from = value;
        g.to = to;
        g.start = now_ms();
        g.last = value;
        g.settle = settle;
        if g.tick.is_some() {
            return;
        }
        let glide = Rc::downgrade(&self.glide);
        let adj = self.adj.clone();
        let view = self.view.clone();
        g.tick = Some(self.view.add_tick_callback(move |_, _| {
            let Some(glide) = glide.upgrade() else { return glib::ControlFlow::Break };
            let mut g = glide.borrow_mut();
            if (adj.value() - g.last).abs() > 1.0 {
                g.tick = None;
                return glib::ControlFlow::Break;
            }
            let t = (now_ms() - g.start) / motion::SCROLL_MS;
            let v = g.from + (g.to - g.from) * motion::ease_out(t);
            g.last = v;
            adj.set_value(v);
            if t < 1.0 {
                return glib::ControlFlow::Continue;
            }
            g.tick = None;
            if g.settle {
                view.scroll_mark_onscreen(&view.buffer().get_insert());
            }
            glib::ControlFlow::Break
        }));
    }

    /// Page Up/Down as GTK does it (the caret keeps its place on screen,
    /// at the ends it goes to the first or last line), but gliding.
    fn page(&self, down: bool, extend: bool) {
        let buffer = self.view.buffer();
        let base = self.target();
        let page = self.adj.page_size();
        let target = self.clamp(if down { base + page } else { base - page });
        let insert = buffer.iter_at_mark(&buffer.get_insert());
        let r = self.view.iter_location(&insert);
        let it = if (target - base).abs() < 1.0 {
            if down { buffer.end_iter() } else { buffer.start_iter() }
        } else {
            self.view.iter_at_location(r.x(), r.y() + (target - base) as i32).unwrap_or(insert)
        };
        if extend {
            buffer.move_mark(&buffer.get_insert(), &it);
        } else {
            buffer.place_cursor(&it);
        }
        self.glide_to(target, false);
    }

    /// Glides so that `it` shows `yalign` down the view, unless its line is
    /// on screen already (away from the edges). `false` when smooth motion
    /// is off or the view has no size yet: the caller jumps instead.
    pub fn scroll_to_iter(&self, it: &gtk::TextIter, yalign: f64) -> bool {
        let page = self.adj.page_size();
        if !enabled() || !self.view.is_mapped() || page <= 0.0 {
            return false;
        }
        let (y, h) = self.view.line_yrange(it);
        let (y, h) = (y as f64, h as f64);
        let margin = page * 0.15;
        let base = self.target();
        if y >= base + margin && y + h <= base + page - margin {
            self.view.scroll_mark_onscreen(&self.view.buffer().get_insert());
            return true;
        }
        self.glide_to(y - yalign * (page - h), true);
        true
    }
}

// ---- The caret ---------------------------------------------------------------

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Caret {
        pub view: RefCell<Option<gtk::TextView>>,
        pub enabled: Cell<bool>,
        /// Where the caret slides from and to (x, y, width, height in this
        /// widget), and when the slide started (0: no slide).
        pub from: Cell<[f32; 4]>,
        pub to: Cell<[f32; 4]>,
        pub move_start: Cell<f64>,
        /// The text area (not the gutter), so the caret never paints over
        /// line numbers when scrolled sideways.
        pub clip: Cell<[f32; 4]>,
        /// The last move or keystroke: blinking starts over from it.
        pub blink_start: Cell<f64>,
        pub tick: RefCell<Option<gtk::TickCallbackId>>,
        pub timer: RefCell<Option<glib::SourceId>>,
        /// A position update is queued; `true` if the caret moved (slide and
        /// restart the blink) rather than the text under it scrolled.
        pub pending: Cell<Option<bool>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Caret {
        const NAME: &'static str = "AnvilCaret";
        type Type = super::Caret;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Caret {}

    impl WidgetImpl for Caret {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(view) = self.view.borrow().clone() else { return };
            if !self.enabled.get() || !view.has_focus() {
                return;
            }
            let alpha = system_blink().alpha(now_ms() - self.blink_start.get());
            if alpha <= 0.0 {
                return;
            }
            let [x, y, w, h] = self.current();
            let [cx, cy, cw, ch] = self.clip.get();
            let color = view.color();
            snapshot.push_clip(&graphene::Rect::new(cx, cy, cw, ch));
            snapshot.append_color(&crate::palette::fade(&color, alpha as f32), &graphene::Rect::new(x, y, w, h));
            snapshot.pop();
        }
    }

    impl Caret {
        /// The caret's place now, partway through a slide.
        pub fn current(&self) -> [f32; 4] {
            let (from, to) = (self.from.get(), self.to.get());
            let start = self.move_start.get();
            if start == 0.0 {
                return to;
            }
            let t = motion::ease_out((now_ms() - start) / motion::CARET_MOVE_MS) as f32;
            [0, 1, 2, 3].map(|i| from[i] + (to[i] - from[i]) * t)
        }
    }
}

glib::wrapper! {
    pub struct Caret(ObjectSubclass<imp::Caret>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Caret {
    fn new(view: &gtk::TextView) -> Caret {
        let caret: Caret = glib::Object::new();
        caret.set_can_target(false);
        caret.set_can_focus(false);
        *caret.imp().view.borrow_mut() = Some(view.clone());

        let buffer = view.buffer();
        let weak = caret.downgrade();
        buffer.connect_mark_set(move |b, _, mark| {
            if let Some(c) = weak.upgrade()
                && *mark == b.get_insert()
            {
                c.schedule(true);
            }
        });
        let weak = caret.downgrade();
        buffer.connect_changed(move |_| {
            if let Some(c) = weak.upgrade() {
                c.schedule(true);
            }
        });
        for adj in [view.vadjustment(), view.hadjustment()].into_iter().flatten() {
            let weak = caret.downgrade();
            adj.connect_value_changed(move |_| {
                if let Some(c) = weak.upgrade() {
                    c.schedule(false);
                }
            });
            // A new page size: the view was resized.
            let weak = caret.downgrade();
            adj.connect_changed(move |_| {
                if let Some(c) = weak.upgrade() {
                    c.schedule(false);
                }
            });
        }
        let focus = gtk::EventControllerFocus::new();
        let weak = caret.downgrade();
        focus.connect_enter(move |_| {
            if let Some(c) = weak.upgrade() {
                c.imp().blink_start.set(now_ms());
                c.schedule(false);
            }
        });
        let weak = caret.downgrade();
        focus.connect_leave(move |_| {
            if let Some(c) = weak.upgrade() {
                c.queue_draw();
            }
        });
        view.add_controller(focus);
        caret
    }

    fn set_enabled(&self, on: bool) {
        self.imp().enabled.set(on);
        self.set_visible(on);
        let Some(view) = self.imp().view.borrow().clone() else { return };
        if on {
            view.add_css_class("anvil-smooth-caret");
            // GTK's blink would repaint the whole (transparent-caret) view;
            // Anvil's caret blinks on its own small layer instead.
            let _ = system_blink();
            if let Some(s) = gtk::Settings::default() {
                s.set_gtk_cursor_blink(false);
            }
            self.schedule(false);
        } else {
            view.remove_css_class("anvil-smooth-caret");
            if let Some(s) = gtk::Settings::default() {
                s.set_gtk_cursor_blink(system_blink().on);
            }
        }
    }

    /// Queues a position update before the next frame (many signals of one
    /// keystroke make one update).
    fn schedule(&self, moved: bool) {
        let imp = self.imp();
        if !imp.enabled.get() {
            return;
        }
        let queued = imp.pending.get();
        imp.pending.set(Some(moved || queued == Some(true)));
        if queued.is_some() {
            return;
        }
        let weak = self.downgrade();
        // Before GTK's redraw (HIGH_IDLE + 20), after the text view saw
        // the change.
        glib::idle_add_local_full(glib::Priority::HIGH_IDLE, move || {
            if let Some(c) = weak.upgrade() {
                let moved = c.imp().pending.take().unwrap_or(false);
                c.update(moved);
            }
            glib::ControlFlow::Break
        });
    }

    fn update(&self, moved: bool) {
        let imp = self.imp();
        let Some(view) = imp.view.borrow().clone() else { return };
        let buffer = view.buffer();
        let r = view.iter_location(&buffer.iter_at_mark(&buffer.get_insert()));
        let (wx, wy) = view.buffer_to_window_coords(gtk::TextWindowType::Widget, r.x(), r.y());
        let Some(p) = view.compute_point(self, &graphene::Point::new(wx as f32, wy as f32)) else { return };
        let gutter = view.gutter(gtk::TextWindowType::Left).map_or(0, |g| g.width()) as f32;
        let Some(origin) = view.compute_point(self, &graphene::Point::new(gutter, 0.0)) else { return };
        let clip = [origin.x(), origin.y(), view.width() as f32 - gutter, view.height() as f32];
        let width = if view.overwrites() { (r.width() as f32).max(4.0) } else { 2.0 };
        let next = [p.x() - if view.overwrites() { 0.0 } else { 1.0 }, p.y(), width, r.height() as f32];
        let now = now_ms();
        let shown = imp.current();
        let inside = |c: [f32; 4]| c[1] + c[3] > clip[1] && c[1] < clip[1] + clip[3];
        // A slide only for short moves on screen (typing, arrows, a click
        // nearby); a jump across the file lands at once.
        let near = (next[1] - shown[1]).abs() <= next[3] * 4.0 && (next[0] - shown[0]).abs() <= 600.0;
        if moved && near && inside(shown) && inside(next) && shown != next {
            imp.from.set(shown);
            imp.move_start.set(now);
        } else {
            imp.from.set(next);
            imp.move_start.set(0.0);
        }
        imp.to.set(next);
        imp.clip.set(clip);
        if moved {
            imp.blink_start.set(now);
        }
        self.queue_draw();
        self.animate();
    }

    /// Repaints every frame while the caret slides or fades; between
    /// fades one timer waits for the next, and at rest nothing runs.
    fn animate(&self) {
        let imp = self.imp();
        if let Some(t) = imp.timer.borrow_mut().take() {
            t.remove();
        }
        if imp.tick.borrow().is_some() {
            return;
        }
        let tick = self.add_tick_callback(|c, _| {
            c.queue_draw();
            let imp = c.imp();
            let now = now_ms();
            let sliding = imp.move_start.get() != 0.0 && now - imp.move_start.get() < motion::CARET_MOVE_MS;
            if !sliding {
                imp.move_start.set(0.0);
            }
            let elapsed = now - imp.blink_start.get();
            let next = system_blink().next_change(elapsed);
            if sliding || next.is_some_and(|n| n <= elapsed) {
                return glib::ControlFlow::Continue;
            }
            imp.tick.borrow_mut().take();
            if let Some(n) = next {
                let weak = c.downgrade();
                let id = glib::timeout_add_local_once(std::time::Duration::from_millis((n - elapsed).max(1.0) as u64), move || {
                    if let Some(c) = weak.upgrade() {
                        c.imp().timer.borrow_mut().take();
                        c.animate();
                    }
                });
                *imp.timer.borrow_mut() = Some(id);
            }
            glib::ControlFlow::Break
        });
        *imp.tick.borrow_mut() = Some(tick);
    }
}
