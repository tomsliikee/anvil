//! The Class Designer: a VS `.cd` file drawn as Insights Mono cards,
//! wired both ways to the code. Moving a card changes only its
//! `<Position>`; adding a member, a type or an inheritance line becomes a
//! text edit planned by `anvil_core::planner`, applied as one undo step.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use anvil_core::cd::{self, Diagram, ShapeKind};
use anvil_core::csharp::{self, Access, MemberKind, MembersFormat, TypeDecl, TypeKind};
use anvil_core::hashcode;
use anvil_core::planner::{self, NewMember};
use anvil_core::spring::{Params, Spring};
use anvil_core::text::TextFile;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib, graphene, gsk};

use crate::i18n::{tr, trf};
use crate::palette::{self, Palette};
use crate::ui::menus::Item;
use crate::ui::window::{self};

const HEADER: f32 = 44.0;
const SECTION: f32 = 24.0;
const ROW: f32 = 21.0;
const PAD_X: f32 = 12.0;
const RADIUS: f32 = 12.0;

/// A type of the project as the diagram sees it.
#[derive(Clone, Debug)]
struct TypeInfo {
    decl: TypeDecl,
    full_name: String,
    file: PathBuf,
}

#[derive(Clone, Debug)]
enum RowKind {
    Section(String),
    Member(usize),
}

#[derive(Clone, Debug)]
struct Card {
    name: String,
    kind: ShapeKind,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    /// Index into `types`, `None` for a comment or a type not in the code.
    ty: Option<usize>,
    rows: Vec<(RowKind, String)>,
    collapsed: bool,
    comment: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Inheritance,
    Association,
}

#[derive(Default)]
pub struct State {
    cards: Vec<Card>,
    types: Vec<TypeInfo>,
    format: MembersFormat,
    zoom: Option<Spring>,
    pan: (f32, f32),
    selected: Option<usize>,
    selected_row: Option<usize>,
    drag: Option<(usize, f32, f32, f32, f32, bool)>,
    panning: Option<(f32, f32, f32, f32)>,
    /// A right press waits for its release: without movement it opens the
    /// context menu there (x, y), with movement it was a pan.
    menu_pending: Option<(f64, f64)>,
    tool: Option<(Tool, Option<usize>)>,
    tick: Option<gtk::TickCallbackId>,
    last_frame: Option<i64>,
    /// Fit the diagram into the view at the first frame with a real size.
    fit_pending: bool,
    /// Lines between cards: (from, to, inheritance?, label).
    edges: Vec<(usize, usize, bool, String)>,
}

impl State {
    fn zoom(&self) -> f32 {
        self.zoom.map_or(1.0, |z| z.value as f32)
    }

    fn to_world(&self, x: f64, y: f64) -> (f32, f32) {
        let z = self.zoom();
        ((x as f32 - self.pan.0) / z, (y as f32 - self.pan.1) / z)
    }

    fn hit(&self, wx: f32, wy: f32) -> Option<usize> {
        self.cards.iter().rposition(|c| wx >= c.x && wx <= c.x + c.w && wy >= c.y && wy <= c.y + c.h)
    }

    fn row_at(&self, card: usize, wy: f32) -> Option<usize> {
        let c = &self.cards[card];
        if c.collapsed || wy < c.y + HEADER {
            return None;
        }
        let mut y = c.y + HEADER;
        for (i, (k, _)) in c.rows.iter().enumerate() {
            let h = if matches!(k, RowKind::Section(_)) { SECTION } else { ROW };
            if wy >= y && wy < y + h {
                return Some(i);
            }
            y += h;
        }
        None
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Canvas {
        pub state: RefCell<State>,
        pub doc: RefCell<std::rc::Weak<DiagramDoc>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "AnvilDiagramCanvas";
        type Type = super::Canvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Canvas {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    impl WidgetImpl for Canvas {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            if obj.width() > 100 && std::mem::take(&mut self.state.borrow_mut().fit_pending) {
                obj.fit();
            }
            obj.draw(snapshot, false);
        }
    }
}

glib::wrapper! {
    pub struct Canvas(ObjectSubclass<imp::Canvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

fn spring_params() -> Params {
    Params { damping: crate::motion::CAMERA.damping, stiffness: crate::motion::CAMERA.stiffness }
}

pub struct DiagramDoc {
    pub path: RefCell<PathBuf>,
    pub widget: gtk::Box,
    pub canvas: Canvas,
    pub project: String,
    diagram: RefCell<Diagram>,
    saved: RefCell<Vec<u8>>,
    details: gtk::ListBox,
    details_title: gtk::Label,
    undo: RefCell<Vec<UndoStep>>,
    redo: RefCell<Vec<UndoStep>>,
    learned: RefCell<hashcode::Learned>,
    zoom_label: gtk::Label,
}

/// One diagram action: the `.cd` text before it and the code files it
/// changed with their text before.
#[derive(Clone)]
struct UndoStep {
    cd: String,
    files: Vec<(PathBuf, Option<String>)>,
}

impl Canvas {
    fn st(&self) -> std::cell::RefMut<'_, State> {
        self.imp().state.borrow_mut()
    }

    fn doc(&self) -> Option<Rc<DiagramDoc>> {
        self.imp().doc.borrow().upgrade()
    }

    fn setup(&self) {
        self.set_focusable(true);
        self.set_overflow(gtk::Overflow::Hidden);
        self.set_hexpand(true);
        self.set_vexpand(true);
        self.add_css_class("designer");
        self.st().zoom = Some(Spring::new(1.0, spring_params()).with_epsilon(0.0005));
        self.st().pan = (40.0, 30.0);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        scroll.connect_scroll(glib::clone!(
            #[weak(rename_to = c)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |ctl, dx, dy| {
                if ctl.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                    let factor = 1.15_f64.powf(-dy);
                    let target = c.st().zoom.map_or(1.0, |z| z.target) * factor;
                    c.zoom_to(target, None);
                } else {
                    let step = if ctl.unit() == gdk::ScrollUnit::Wheel { 40.0 } else { 1.0 };
                    let mut st = c.st();
                    st.pan.0 -= (dx * step) as f32;
                    st.pan.1 -= (dy * step) as f32;
                    drop(st);
                    c.queue_draw();
                }
                glib::Propagation::Stop
            }
        ));
        self.add_controller(scroll);

        let drag = gtk::GestureDrag::new();
        drag.set_button(0);
        drag.connect_drag_begin(glib::clone!(
            #[weak(rename_to = c)]
            self,
            move |g, x, y| c.drag_begin(g, x, y)
        ));
        drag.connect_drag_update(glib::clone!(
            #[weak(rename_to = c)]
            self,
            move |_, ox, oy| c.drag_update(ox, oy)
        ));
        drag.connect_drag_end(glib::clone!(
            #[weak(rename_to = c)]
            self,
            move |_, _, _| c.drag_end()
        ));
        self.add_controller(drag);

        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.connect_pressed(glib::clone!(
            #[weak(rename_to = c)]
            self,
            move |g, n, x, y| c.click(g.current_button(), n, x, y)
        ));
        self.add_controller(click);

        // Drag a .cs file from the Solution Explorer: its types appear.
        let drop = gtk::DropTarget::new(glib::types::Type::STRING, gdk::DragAction::COPY);
        drop.connect_drop(glib::clone!(
            #[weak(rename_to = c)]
            self,
            #[upgrade_or]
            false,
            move |_, v, x, y| {
                let Ok(path) = v.get::<String>() else { return false };
                let (wx, wy) = c.imp().state.borrow().to_world(x, y);
                if let Some(doc) = c.doc() {
                    doc.show_types_of_file(Path::new(&path), wx, wy);
                }
                true
            }
        ));
        self.add_controller(drop);

        adw::StyleManager::default().connect_dark_notify(glib::clone!(
            #[weak(rename_to = c)]
            self,
            move |_| c.queue_draw()
        ));
    }

    fn zoom_to(&self, target: f64, around: Option<(f32, f32)>) {
        let target = target.clamp(0.15, 3.0);
        let (w, h) = (self.width() as f32, self.height() as f32);
        let (cx, cy) = around.unwrap_or((w / 2.0, h / 2.0));
        let (wx, wy) = self.imp().state.borrow().to_world(cx as f64, cy as f64);
        if let Some(z) = self.st().zoom.as_mut() {
            z.retarget(target);
        }
        let id = self.add_tick_callback(move |c, clock| {
            let now = clock.frame_time();
            let still = {
                let mut st = c.st();
                let dt = st.last_frame.map_or(1.0 / 60.0, |t| (now - t) as f64 / 1e6);
                st.last_frame = Some(now);
                let z = st.zoom.as_mut().map(|z| {
                    z.step(dt);
                    (z.value as f32, z.is_resting())
                });
                let (zv, rest) = z.unwrap_or((1.0, true));
                st.pan = (cx - wx * zv, cy - wy * zv);
                rest
            };
            c.queue_draw();
            if let Some(d) = c.doc() {
                d.zoom_label.set_label(&format!("{:.0} %", c.imp().state.borrow().zoom() * 100.0));
            }
            if still {
                let mut st = c.st();
                st.tick = None;
                st.last_frame = None;
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        if let Some(old) = self.st().tick.replace(id) {
            old.remove();
        }
    }

    fn fit(&self) {
        let st = self.imp().state.borrow();
        if st.cards.is_empty() {
            return;
        }
        let x0 = st.cards.iter().map(|c| c.x).fold(f32::MAX, f32::min) - 40.0;
        let y0 = st.cards.iter().map(|c| c.y).fold(f32::MAX, f32::min) - 40.0;
        let x1 = st.cards.iter().map(|c| c.x + c.w).fold(f32::MIN, f32::max) + 40.0;
        let y1 = st.cards.iter().map(|c| c.y + c.h).fold(f32::MIN, f32::max) + 40.0;
        let (w, h) = (self.width().max(400) as f32, self.height().max(300) as f32);
        // Leave room for the toolbox on the left.
        let w = w - 150.0;
        // Readable first: VS opens at 100 %; below 60 % the members are
        // too small to read below 85 %, so the view starts at the top left then.
        let z = (w / (x1 - x0)).min(h / (y1 - y0)).clamp(0.85, 1.0);
        drop(st);
        let mut st = self.st();
        if let Some(s) = st.zoom.as_mut() {
            s.set(z as f64);
        }
        if let Some(d) = self.doc() {
            d.zoom_label.set_label(&format!("{:.0} %", z * 100.0));
        }
        let fits = (x1 - x0) * z <= w && (y1 - y0) * z <= h;
        st.pan = if fits { (150.0 - x0 * z + (w - (x1 - x0) * z) / 2.0, -y0 * z + (h - (y1 - y0) * z) / 2.0) } else { (150.0 - x0 * z, -y0 * z) };
        drop(st);
        self.queue_draw();
    }

    fn drag_begin(&self, g: &gtk::GestureDrag, x: f64, y: f64) {
        self.grab_focus();
        let button = g.current_button();
        let (wx, wy) = self.imp().state.borrow().to_world(x, y);
        let hit = self.imp().state.borrow().hit(wx, wy);
        let tool_active = self.imp().state.borrow().tool.is_some();
        let mut st = self.st();
        match (button, hit) {
            (b, Some(i)) if b == gdk::BUTTON_PRIMARY && !tool_active => {
                let c = &st.cards[i];
                let (cx, cy) = (c.x, c.y);
                st.drag = Some((i, cx, cy, wx, wy, false));
                // Raise the card.
                let card = st.cards.remove(i);
                st.cards.push(card);
                let last = st.cards.len() - 1;
                st.drag = Some((last, cx, cy, wx, wy, false));
                st.selected = Some(last);
            }
            (b, None) if b == gdk::BUTTON_PRIMARY || b == gdk::BUTTON_MIDDLE => {
                let (px, py) = st.pan;
                st.panning = Some((px, py, x as f32, y as f32));
            }
            // The right button pans anywhere, over cards too.
            (b, _) if b == gdk::BUTTON_SECONDARY => {
                let (px, py) = st.pan;
                st.panning = Some((px, py, x as f32, y as f32));
                st.menu_pending = Some((x, y));
            }
            _ => {}
        }
    }

    fn drag_update(&self, ox: f64, oy: f64) {
        let mut st = self.st();
        let z = st.zoom();
        if let Some((i, cx, cy, _, _, moved)) = st.drag {
            if !moved && ox.abs() + oy.abs() < 3.0 {
                return;
            }
            st.cards[i].x = cx + ox as f32 / z;
            st.cards[i].y = cy + oy as f32 / z;
            st.drag = Some((i, cx, cy, 0.0, 0.0, true));
        } else if let Some((px, py, _, _)) = st.panning {
            if st.menu_pending.is_some() {
                if ox.abs() + oy.abs() < 4.0 {
                    return;
                }
                st.menu_pending = None;
                drop(st);
                self.set_cursor_from_name(Some("grabbing"));
                st = self.st();
            }
            st.pan = (px + ox as f32, py + oy as f32);
        }
        drop(st);
        self.queue_draw();
    }

    fn drag_end(&self) {
        let (drag, _, menu) = {
            let mut st = self.st();
            (st.drag.take(), st.panning.take(), st.menu_pending.take())
        };
        self.set_cursor(None);
        if let Some((x, y)) = menu {
            self.context_menu(x, y);
            return;
        }
        if let Some((i, ..)) = drag.filter(|d| d.5)
            && let Some(doc) = self.doc()
        {
            // VS snaps to an eighth of an inch.
            let (name, x, y) = {
                let mut st = self.st();
                let c = &mut st.cards[i];
                let snap = |v: f32| (v / 12.0).round() * 12.0;
                c.x = snap(c.x);
                c.y = snap(c.y);
                (c.name.clone(), c.x, c.y)
            };
            doc.edit_cd(|d| d.move_shape(&name, cd::px_to_inches(x as f64), cd::px_to_inches(y as f64)));
        }
        self.queue_draw();
    }

    fn click(&self, button: u32, n: i32, x: f64, y: f64) {
        let Some(doc) = self.doc() else { return };
        let (wx, wy) = self.imp().state.borrow().to_world(x, y);
        let hit = self.imp().state.borrow().hit(wx, wy);
        let row = hit.and_then(|i| self.imp().state.borrow().row_at(i, wy));
        {
            let mut st = self.st();
            st.selected = hit;
            st.selected_row = row;
        }
        doc.update_details();
        self.queue_draw();
        // A tool picks two cards.
        let tool = self.imp().state.borrow().tool;
        if let (Some((tool, first)), Some(i)) = (tool, hit) {
            match first {
                None => self.st().tool = Some((tool, Some(i))),
                Some(a) => {
                    self.st().tool = None;
                    doc.connect_cards(tool, a, i);
                }
            }
            return;
        }
        // The right button's menu opens on release (drag_end), so a
        // right drag can pan.
        if button == gdk::BUTTON_SECONDARY {
            return;
        }
        // A click on a compartment's title folds it, as in VS.
        if let (Some(i), Some(r)) = (hit, row) {
            let section = match self.imp().state.borrow().cards[i].rows.get(r) {
                Some((RowKind::Section(key), _)) if !key.is_empty() => Some(key.clone()),
                _ => None,
            };
            if let Some(key) = section {
                let name = self.imp().state.borrow().cards[i].name.clone();
                let collapsed = doc.diagram.borrow().find(&name).is_some_and(|s| s.collapsed_compartments.contains(&key));
                doc.edit_cd(|d| d.set_compartment_collapsed(&name, &key, !collapsed));
                return;
            }
        }
        if n == 2
            && let Some(i) = hit
        {
            doc.open_code(i, row);
        }
    }

    fn context_menu(&self, x: f64, y: f64) {
        let Some(doc) = self.doc() else { return };
        let (wx, wy) = self.imp().state.borrow().to_world(x, y);
        let hit = self.imp().state.borrow().hit(wx, wy);
        let row = hit.and_then(|i| self.imp().state.borrow().row_at(i, wy));
        match hit {
            Some(i) => doc.card_menu(self.upcast_ref(), x, y, i, row),
            None => doc.canvas_menu(self.upcast_ref(), x, y, wx, wy),
        }
    }

    /// Draws everything; with `export` at zoom 1 without grid and minimap.
    fn draw(&self, s: &gtk::Snapshot, export: bool) {
        let p = palette::current();
        let st = self.imp().state.borrow();
        let (w, h) = (self.width() as f32, self.height() as f32);
        // The canvas is the editor card's ground; cards sit one step up
        // (white in light, fill in dark). An export keeps page and card.
        let (ground, face) = if export {
            (&p.bg, &p.card)
        } else if adw::StyleManager::default().is_dark() {
            (&p.card, &p.fill)
        } else {
            (&p.card, &p.bar)
        };
        if !export {
            s.append_color(ground, &graphene::Rect::new(0.0, 0.0, w, h));
            draw_grid(s, &st, w, h, p);
            s.save();
            s.translate(&graphene::Point::new(st.pan.0, st.pan.1));
            let z = st.zoom();
            s.scale(z, z);
        }
        let ctx = self.pango_context();
        draw_edges(s, &st, p, &ctx, ground);
        for (i, c) in st.cards.iter().enumerate() {
            let selected = !export && st.selected == Some(i);
            let picked = matches!(st.tool, Some((_, Some(a))) if a == i);
            draw_card(s, c, &st, &ctx, (p, face), selected || picked, if selected { st.selected_row } else { None });
        }
        if !export {
            s.restore();
            if !st.cards.is_empty() {
                draw_minimap(s, &st, w, h, p);
            }
            if st.cards.is_empty() {
                let layout = gtk::pango::Layout::new(&ctx);
                layout.set_font_description(Some(&gtk::pango::FontDescription::from_string("Geist Serif 18")));
                layout.set_text(&tr("Right-click to add a class, or drag a .cs file here."));
                let (lw, _) = layout.pixel_size();
                s.save();
                s.translate(&graphene::Point::new((w - lw as f32) / 2.0, h / 2.0 - 20.0));
                s.append_layout(&layout, &p.ink2);
                s.restore();
            }
        }
    }

    /// The whole diagram at zoom 1, for export: (node, bounds).
    fn export_node(&self) -> Option<(gsk::RenderNode, graphene::Rect)> {
        let p = palette::current();
        let st = self.imp().state.borrow();
        if st.cards.is_empty() {
            return None;
        }
        let x0 = st.cards.iter().map(|c| c.x).fold(f32::MAX, f32::min) - 32.0;
        let y0 = st.cards.iter().map(|c| c.y).fold(f32::MAX, f32::min) - 32.0;
        let x1 = st.cards.iter().map(|c| c.x + c.w).fold(f32::MIN, f32::max) + 32.0;
        let y1 = st.cards.iter().map(|c| c.y + c.h).fold(f32::MIN, f32::max) + 32.0;
        drop(st);
        let bounds = graphene::Rect::new(x0, y0, x1 - x0, y1 - y0);
        let s = gtk::Snapshot::new();
        s.append_color(&p.bg, &bounds);
        self.draw(&s, true);
        s.to_node().map(|n| (n, bounds))
    }
}

/// Members in the code face, so a card reads like its source.
const MEMBER_FACE: &str = "Cascadia Mono 8.5";
const CHIP_FACE: &str = "Cascadia Mono 7";

fn font(desc: &str) -> gtk::pango::FontDescription {
    gtk::pango::FontDescription::from_string(desc)
}

/// Draws `text` at (x, y), cut with an ellipsis at `max_w`.
fn text_at(s: &gtk::Snapshot, ctx: &gtk::pango::Context, text: &str, desc: &str, (x, y, max_w): (f32, f32, f32), color: &gdk::RGBA) {
    let layout = gtk::pango::Layout::new(ctx);
    layout.set_font_description(Some(&font(desc)));
    layout.set_text(text);
    layout.set_width((max_w.max(10.0) * gtk::pango::SCALE as f32) as i32);
    layout.set_ellipsize(gtk::pango::EllipsizeMode::End);
    s.save();
    s.translate(&graphene::Point::new(x, y));
    s.append_layout(&layout, color);
    s.restore();
}

fn text_width(ctx: &gtk::pango::Context, text: &str, desc: &str) -> f32 {
    let layout = gtk::pango::Layout::new(ctx);
    layout.set_font_description(Some(&font(desc)));
    layout.set_text(text);
    layout.pixel_size().0 as f32
}

fn rounded(r: graphene::Rect, radius: f32) -> gsk::RoundedRect {
    gsk::RoundedRect::from_rect(r, radius)
}

fn fill_rounded(s: &gtk::Snapshot, r: gsk::RoundedRect, color: &gdk::RGBA) {
    s.push_rounded_clip(&r);
    s.append_color(color, r.bounds());
    s.pop();
}

fn kind_chip(c: &Card, st: &State) -> String {
    match c.kind {
        ShapeKind::Comment => String::new(),
        ShapeKind::Enum => "enum".into(),
        ShapeKind::Interface => "interface".into(),
        ShapeKind::Struct => "struct".into(),
        ShapeKind::Delegate => "delegate".into(),
        ShapeKind::Class => match c.ty.map(|t| &st.types[t].decl) {
            Some(d) if d.kind == TypeKind::Record => "record".into(),
            Some(d) if d.is_static() => "static class".into(),
            Some(d) if d.is_abstract() => "abstract class".into(),
            Some(d) if d.modifiers.iter().any(|m| m == "sealed") => "sealed class".into(),
            _ => "class".into(),
        },
    }
}

fn draw_card(s: &gtk::Snapshot, c: &Card, st: &State, ctx: &gtk::pango::Context, (p, face): (&Palette, &gdk::RGBA), selected: bool, selected_row: Option<usize>) {
    let rect = graphene::Rect::new(c.x, c.y, c.w, c.h);
    let rr = rounded(rect, RADIUS);
    if c.kind == ShapeKind::Comment {
        fill_rounded(s, rr, &p.fill);
        let layout = gtk::pango::Layout::new(ctx);
        layout.set_font_description(Some(&font("Inter 9.5")));
        layout.set_text(&c.comment);
        layout.set_width(((c.w - 2.0 * PAD_X) * gtk::pango::SCALE as f32) as i32);
        layout.set_wrap(gtk::pango::WrapMode::WordChar);
        s.save();
        s.translate(&graphene::Point::new(c.x + PAD_X, c.y + 10.0));
        s.append_layout(&layout, &p.ink2);
        s.restore();
        return;
    }
    fill_rounded(s, rr, face);
    if selected {
        s.append_border(&rr, &[1.5; 4], &[p.ink2; 4]);
    }
    // The type name in Geist Serif, its kind as a mono chip.
    let chip = kind_chip(c, st);
    let chip_w = text_width(ctx, &chip, CHIP_FACE) + 10.0;
    let short = c.name.rsplit('.').next().unwrap_or(&c.name).to_owned();
    text_at(s, ctx, &short, "Geist Serif 12", (c.x + PAD_X, c.y + 7.0, c.w - 2.0 * PAD_X - chip_w - 6.0), &p.ink);
    let missing = c.ty.is_none();
    let chip_rect = graphene::Rect::new(c.x + c.w - PAD_X - chip_w, c.y + 10.0, chip_w, 15.0);
    fill_rounded(s, rounded(chip_rect, 4.0), if face == &p.fill { &p.card } else { &p.fill });
    text_at(s, ctx, &chip, CHIP_FACE, (chip_rect.x() + 5.0, chip_rect.y() + 1.0, chip_w), &p.ink2);
    let sub = if missing {
        tr("not found in the code")
    } else {
        c.ty.map(|t| st.types[t].decl.bases.join(", ")).unwrap_or_default()
    };
    if !sub.is_empty() {
        text_at(s, ctx, &sub, "Inter 8", (c.x + PAD_X, c.y + 26.0, c.w - 2.0 * PAD_X), if missing { &p.down } else { &p.ink3 });
    }
    if c.collapsed {
        return;
    }
    let mut y = c.y + HEADER;
    for (i, (k, text)) in c.rows.iter().enumerate() {
        match k {
            RowKind::Section(_) => {
                s.append_color(&p.line, &graphene::Rect::new(c.x + PAD_X, y + 2.0, c.w - 2.0 * PAD_X, 1.0));
                text_at(s, ctx, text, "Inter Semi-Bold 8", (c.x + PAD_X, y + 6.0, c.w - 2.0 * PAD_X), &p.ink3);
                y += SECTION;
            }
            RowKind::Member(_) => {
                if selected_row == Some(i) {
                    fill_rounded(s, rounded(graphene::Rect::new(c.x + 4.0, y, c.w - 8.0, ROW), 6.0), &palette::fade(&p.ink3, 0.18));
                }
                text_at(s, ctx, text, MEMBER_FACE, (c.x + PAD_X, y + 3.0, c.w - 2.0 * PAD_X), &p.ink);
                y += ROW;
            }
        }
    }
}

fn card_anchor(c: &Card, toward: (f32, f32)) -> (f32, f32) {
    let (cx, cy) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
    let (dx, dy) = (toward.0 - cx, toward.1 - cy);
    if dx.abs() * c.h > dy.abs() * c.w {
        (if dx > 0.0 { c.x + c.w } else { c.x }, cy + dy * (c.w / 2.0) / dx.abs().max(1.0))
    } else {
        (cx + dx * (c.h / 2.0) / dy.abs().max(1.0), if dy > 0.0 { c.y + c.h } else { c.y })
    }
}

fn draw_edges(s: &gtk::Snapshot, st: &State, p: &Palette, ctx: &gtk::pango::Context, ground: &gdk::RGBA) {
    for (a, b, inherit, label) in &st.edges {
        let (Some(ca), Some(cb)) = (st.cards.get(*a), st.cards.get(*b)) else { continue };
        let to = card_anchor(cb, (ca.x + ca.w / 2.0, ca.y + ca.h / 2.0));
        let from = card_anchor(ca, to);
        let builder = gsk::PathBuilder::new();
        builder.move_to(from.0, from.1);
        builder.line_to(to.0, to.1);
        let stroke = gsk::Stroke::new(1.2);
        s.append_stroke(&builder.to_path(), &stroke, &p.ink2);
        // Head: a hollow triangle for inheritance, an open arrow for an
        // association.
        let ang = (from.1 - to.1).atan2(from.0 - to.0);
        let (l, wing) = (12.0_f32, 0.45_f32);
        let p1 = (to.0 + l * (ang + wing).cos(), to.1 + l * (ang + wing).sin());
        let p2 = (to.0 + l * (ang - wing).cos(), to.1 + l * (ang - wing).sin());
        let head = gsk::PathBuilder::new();
        head.move_to(p1.0, p1.1);
        head.line_to(to.0, to.1);
        head.line_to(p2.0, p2.1);
        if *inherit {
            head.close();
            s.append_fill(&head.to_path(), gsk::FillRule::Winding, ground);
        }
        s.append_stroke(&head.to_path(), &stroke, &p.ink2);
        if !label.is_empty() {
            let (mx, my) = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
            text_at(s, ctx, label, "Cascadia Mono 8", (mx + 4.0, my - 14.0, 200.0), &p.ink2);
        }
    }
    // Lollipops: implemented interfaces not on the diagram, a small ring
    // above the card with the names.
    for c in &st.cards {
        let Some(t) = c.ty else { continue };
        let ifaces: Vec<&String> = st.types[t].decl.bases.iter().filter(|b| is_interface_name(b) && !st.cards.iter().any(|o| short_name(&o.name) == b.as_str())).collect();
        if ifaces.is_empty() {
            continue;
        }
        let (x, y) = (c.x + 20.0, c.y);
        let stem = gsk::PathBuilder::new();
        stem.move_to(x, y);
        stem.line_to(x, y - 14.0);
        s.append_stroke(&stem.to_path(), &gsk::Stroke::new(1.2), &p.ink2);
        let ring = gsk::PathBuilder::new();
        ring.add_circle(&graphene::Point::new(x, y - 19.0), 5.0);
        s.append_stroke(&ring.to_path(), &gsk::Stroke::new(1.2), &p.ink2);
        let names = ifaces.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
        text_at(s, ctx, &names, "Cascadia Mono 8", (x + 9.0, y - 27.0, 260.0), &p.ink2);
    }
}

fn draw_grid(s: &gtk::Snapshot, st: &State, w: f32, h: f32, p: &Palette) {
    let z = st.zoom();
    if z < 0.35 {
        return;
    }
    let step = 24.0 * z;
    let ox = st.pan.0.rem_euclid(step);
    let oy = st.pan.1.rem_euclid(step);
    let tile = graphene::Rect::new(ox, oy, step, step);
    s.push_repeat(&graphene::Rect::new(0.0, 0.0, w, h), Some(&tile));
    let r = 1.1 * z.sqrt();
    fill_rounded(s, rounded(graphene::Rect::new(ox - r, oy - r, 2.0 * r, 2.0 * r), r), &p.line);
    s.pop();
}

fn draw_minimap(s: &gtk::Snapshot, st: &State, w: f32, h: f32, p: &Palette) {
    let (mw, mh) = (150.0_f32, 100.0_f32);
    let (mx, my) = (w - mw - 14.0, h - mh - 14.0);
    let x0 = st.cards.iter().map(|c| c.x).fold(f32::MAX, f32::min);
    let y0 = st.cards.iter().map(|c| c.y).fold(f32::MAX, f32::min);
    let x1 = st.cards.iter().map(|c| c.x + c.w).fold(f32::MIN, f32::max);
    let y1 = st.cards.iter().map(|c| c.y + c.h).fold(f32::MIN, f32::max);
    let scale = ((mw - 16.0) / (x1 - x0).max(1.0)).min((mh - 16.0) / (y1 - y0).max(1.0));
    let box_r = rounded(graphene::Rect::new(mx, my, mw, mh), 12.0);
    s.append_outset_shadow(&box_r, &p.shadow, 0.0, 6.0, 0.0, 16.0);
    fill_rounded(s, box_r, &p.bar);
    for c in &st.cards {
        let r = graphene::Rect::new(mx + 8.0 + (c.x - x0) * scale, my + 8.0 + (c.y - y0) * scale, (c.w * scale).max(2.0), (c.h * scale).max(2.0));
        fill_rounded(s, rounded(r, 2.0), &p.fill);
    }
    let z = st.zoom();
    let (vx0, vy0) = (-st.pan.0 / z, -st.pan.1 / z);
    let view = graphene::Rect::new(mx + 8.0 + (vx0 - x0) * scale, my + 8.0 + (vy0 - y0) * scale, w / z * scale, h / z * scale);
    let clip = graphene::Rect::new(mx, my, mw, mh);
    s.push_clip(&clip);
    let b = gsk::PathBuilder::new();
    b.add_rect(&view);
    s.append_stroke(&b.to_path(), &gsk::Stroke::new(1.0), &p.ink3);
    s.pop();
}

fn is_interface_name(n: &str) -> bool {
    n.starts_with('I') && n.chars().nth(1).is_some_and(char::is_uppercase)
}

fn short_name(full: &str) -> &str {
    full.rsplit('.').next().unwrap_or(full)
}

/// Rows of a card in VS' compartments (Fields, Properties, Methods,
/// Events, Nested Types), or one list for enum values.
fn rows_for(decl: &TypeDecl, format: MembersFormat, collapsed: &[String], grouping: &str, hide_assoc: &[String]) -> Vec<(RowKind, String)> {
    let mut out = Vec::new();
    if decl.kind == TypeKind::Enum {
        for (i, m) in decl.members.iter().enumerate() {
            out.push((RowKind::Member(i), m.name.clone()));
        }
        return out;
    }
    let visible: Vec<(usize, &csharp::Member)> = decl.members.iter().enumerate().filter(|(_, m)| !hide_assoc.contains(&m.name)).collect();
    let groups: Vec<(String, Vec<(usize, &csharp::Member)>)> = match grouping {
        "Access" => [Access::Public, Access::Internal, Access::Protected, Access::ProtectedInternal, Access::PrivateProtected, Access::Private]
            .into_iter()
            .map(|a| (a.keyword().to_owned(), visible.iter().copied().filter(|(_, m)| m.access == a).collect()))
            .collect(),
        "Alphabetical" => {
            let mut all = visible.clone();
            all.sort_by_key(|(_, m)| m.name.to_lowercase());
            vec![(String::new(), all)]
        }
        _ => {
            let pick = |kinds: &[MemberKind]| -> Vec<(usize, &csharp::Member)> { visible.iter().copied().filter(|(_, m)| kinds.contains(&m.kind)).collect() };
            vec![
                (tr("Fields"), pick(&[MemberKind::Field, MemberKind::Constant])),
                (tr("Properties"), pick(&[MemberKind::Property, MemberKind::Indexer])),
                (tr("Methods"), pick(&[MemberKind::Method, MemberKind::Constructor, MemberKind::Destructor, MemberKind::Operator])),
                (tr("Events"), pick(&[MemberKind::Event])),
            ]
        }
    };
    let compartment_key = |title: &str| match title {
        t if t == tr("Fields") => "Fields".to_owned(),
        t if t == tr("Properties") => "Properties".to_owned(),
        t if t == tr("Methods") => "Methods".to_owned(),
        t if t == tr("Events") => "Events".to_owned(),
        t => t.to_owned(),
    };
    for (title, members) in groups {
        if members.is_empty() {
            continue;
        }
        if !title.is_empty() {
            out.push((RowKind::Section(compartment_key(&title)), title.clone()));
        }
        if collapsed.contains(&compartment_key(&title)) {
            continue;
        }
        for (i, m) in members {
            out.push((RowKind::Member(i), m.display(format)));
        }
    }
    if !decl.nested.is_empty() {
        out.push((RowKind::Section("Nested Types".into()), tr("Nested Types")));
        if !collapsed.iter().any(|c| c == "Nested Types") {
            for n in &decl.nested {
                out.push((RowKind::Section(String::new()), format!("{} {}", n.kind.keyword(), n.name)));
            }
        }
    }
    out
}

/// Opens a `.cd` as a tab.
impl DiagramDoc {
    pub fn open(path: &Path) -> Result<Rc<DiagramDoc>, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let diagram = Diagram::parse(&bytes).map_err(|e| e.to_string())?;
        let app = crate::ui::window::app();
        let project = app.solution.borrow().as_ref().and_then(|s| s.project_of(path).map(|p| p.name.clone())).unwrap_or_default();
        let canvas: Canvas = glib::Object::new();
        let (widget, details, details_title, zoom_label) = build_frame(&canvas);
        let doc = Rc::new(DiagramDoc {
            path: RefCell::new(path.to_owned()),
            widget,
            canvas: canvas.clone(),
            project,
            diagram: RefCell::new(diagram),
            saved: RefCell::new(bytes),
            details,
            details_title,
            undo: RefCell::default(),
            redo: RefCell::default(),
            learned: RefCell::default(),
            zoom_label,
        });
        *canvas.imp().doc.borrow_mut() = Rc::downgrade(&doc);
        doc.reload_types();
        doc.learn_hashes();
        doc.rebuild();
        // Restore zoom and scroll from .vs/anvil/diagrams/<name>.json.
        if let Some(dir) = window::solution_dir()
            && let Ok(t) = std::fs::read_to_string(anvil_core::session::diagram_view_path(&dir, path))
            && let Ok(v) = serde_json::from_str::<anvil_core::session::DiagramView>(&t)
        {
            let mut st = canvas.st();
            if let Some(z) = st.zoom.as_mut() {
                z.set(v.zoom.clamp(0.15, 3.0));
            }
            st.pan = (v.x as f32, v.y as f32);
        } else {
            canvas.st().fit_pending = true;
        }
        doc.zoom_label.set_label(&format!("{:.0} %", canvas.imp().state.borrow().zoom() * 100.0));
        let d = Rc::downgrade(&doc);
        doc.widget.connect_unmap(move |_| {
            if let Some(d) = d.upgrade() {
                d.save_view();
            }
        });
        Ok(doc)
    }

    fn save_view(&self) {
        let Some(dir) = window::solution_dir() else { return };
        let st = self.canvas.imp().state.borrow();
        let v = anvil_core::session::DiagramView { zoom: st.zoom() as f64, x: st.pan.0 as f64, y: st.pan.1 as f64 };
        let p = anvil_core::session::diagram_view_path(&dir, &self.path.borrow());
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(p, serde_json::to_string(&v).unwrap_or_default());
    }

    pub fn is_dirty(&self) -> bool {
        self.diagram.borrow().to_bytes() != *self.saved.borrow()
    }

    pub fn mark_clean(&self) {
        *self.saved.borrow_mut() = self.diagram.borrow().to_bytes();
    }

    pub fn save(&self) -> std::io::Result<()> {
        let bytes = self.diagram.borrow().to_bytes();
        anvil_core::text::write_atomic(&self.path.borrow(), &bytes)?;
        *self.saved.borrow_mut() = bytes;
        crate::ui::window::app().tabs.refresh_titles();
        Ok(())
    }

    /// Parses every `.cs` of the project (open buffers first).
    fn reload_types(&self) {
        self.canvas.st().types = parse_types(self.type_sources());
    }

    /// The project's source files, with the text of those open in a tab
    /// (unsaved edits count); the rest is read from disk by [`parse_types`].
    fn type_sources(&self) -> Vec<(PathBuf, Option<String>)> {
        let app = crate::ui::window::app();
        let sol = app.solution.borrow().clone();
        let files: Vec<PathBuf> = sol.as_ref().and_then(|s| s.project_named(&self.project)).map(|p| p.source_files()).unwrap_or_else(|| {
            // A diagram outside a project: the .cs files next to it.
            let dir = self.path.borrow().parent().map(Path::to_path_buf).unwrap_or_default();
            std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "cs")).collect()).unwrap_or_default()
        });
        files.into_iter().map(|f| (f.clone(), app.tabs.code_doc(&f).map(|d| d.text()))).collect()
    }

    fn learn_hashes(&self) {
        let st = self.canvas.imp().state.borrow();
        let samples: Vec<(BTreeSet<String>, hashcode::Bits)> = self
            .diagram
            .borrow()
            .shapes()
            .iter()
            .filter_map(|s| {
                let t = st.types.iter().find(|t| t.full_name == s.name)?;
                let names = hashcode::hashed_names(&t.decl.members).into_iter().map(str::to_owned).collect();
                Some((names, hashcode::parse(s.hash.as_deref()?)?))
            })
            .collect();
        self.learned.borrow_mut().learn(&samples);
    }

    /// Cards from the `.cd` shapes and the structure model.
    fn rebuild(&self) {
        let d = self.diagram.borrow();
        let format = d.members_format();
        let grouping = d.doc.attr(0, "GroupingSetting").unwrap_or("Kind").to_owned();
        let shapes = d.shapes();
        let ctx = self.canvas.pango_context();
        let mut st = self.canvas.st();
        let selected_name = st.selected.and_then(|i| st.cards.get(i).map(|c| c.name.clone()));
        let mut cards = Vec::new();
        for sh in &shapes {
            let ty = st.types.iter().position(|t| t.full_name == sh.name);
            let hide: Vec<String> = sh.show_as_association.iter().chain(&sh.show_as_collection).cloned().collect();
            let rows = ty.map(|t| rows_for(&st.types[t].decl, format, &sh.collapsed_compartments, &grouping, &hide)).unwrap_or_default();
            let w = cd::inches_to_px(sh.width) as f32;
            let body: f32 = rows.iter().map(|(k, _)| if matches!(k, RowKind::Section(_)) { SECTION } else { ROW }).sum();
            let h = if sh.kind == ShapeKind::Comment { sh.height.map_or(72.0, |h| cd::inches_to_px(h) as f32) } else if sh.collapsed { HEADER } else { HEADER + body + 8.0 };
            cards.push(Card { name: sh.name.clone(), kind: sh.kind, x: cd::inches_to_px(sh.x) as f32, y: cd::inches_to_px(sh.y) as f32, w, h, ty, rows, collapsed: sh.collapsed, comment: if sh.kind == ShapeKind::Comment { sh.name.clone() } else { String::new() } });
        }
        // Edges: inheritance (base on the diagram) and associations.
        let mut edges = Vec::new();
        for (i, c) in cards.iter().enumerate() {
            let Some(t) = c.ty else { continue };
            for b in &st.types[t].decl.bases {
                let base = b.split('<').next().unwrap_or(b);
                if let Some(j) = cards.iter().position(|o| short_name(&o.name) == base) {
                    edges.push((i, j, true, String::new()));
                }
            }
            let sh = shapes.iter().find(|s| s.name == c.name);
            for m in sh.map(|s| s.show_as_association.iter().chain(&s.show_as_collection).cloned().collect::<Vec<_>>()).unwrap_or_default() {
                let Some(member) = st.types[t].decl.members.iter().find(|x| x.name == m) else { continue };
                let ty = member.ty.clone().unwrap_or_default();
                let target = ty.trim_end_matches("[]").trim_end_matches('?').split(['<', '>']).rfind(|x| !x.is_empty()).unwrap_or(&ty).to_owned();
                if let Some(j) = cards.iter().position(|o| short_name(&o.name) == target) {
                    edges.push((i, j, false, m.clone()));
                }
            }
        }
        st.edges = edges;
        st.format = format;
        st.cards = cards;
        st.selected = selected_name.and_then(|n| st.cards.iter().position(|c| c.name == n));
        let _ = ctx;
        drop(st);
        self.canvas.queue_draw();
        self.update_details();
    }

    /// Changes the `.cd` as one undo step.
    fn edit_cd(&self, f: impl FnOnce(&mut Diagram)) {
        let before = self.diagram.borrow().file.text.clone();
        f(&mut self.diagram.borrow_mut());
        if self.diagram.borrow().file.text != before {
            self.undo.borrow_mut().push(UndoStep { cd: before, files: Vec::new() });
            self.redo.borrow_mut().clear();
        }
        self.rebuild();
        crate::ui::window::app().tabs.refresh_titles();
    }

    /// Applies planned code edits (and an optional `.cd` change) as one
    /// undo step shared by diagram and editor.
    fn apply_code(&self, file_edits: Vec<(PathBuf, Vec<anvil_core::edit::TextEdit>)>, cd_change: impl FnOnce(&mut Diagram)) {
        let app = crate::ui::window::app();
        let mut step = UndoStep { cd: self.diagram.borrow().file.text.clone(), files: Vec::new() };
        for (path, edits) in file_edits {
            match app.tabs.code_doc(&path) {
                Some(doc) => {
                    step.files.push((path.clone(), None));
                    doc.apply_edits(&edits);
                }
                None => {
                    let Ok(mut f) = TextFile::read(&path) else { continue };
                    step.files.push((path.clone(), Some(f.text.clone())));
                    f.text = anvil_core::edit::apply(&f.text, &edits);
                    if let Err(e) = f.write(&path) {
                        window::show_error(&tr("The code could not be changed"), &e.to_string());
                    }
                }
            }
        }
        cd_change(&mut self.diagram.borrow_mut());
        self.undo.borrow_mut().push(step);
        self.redo.borrow_mut().clear();
        self.reload_types();
        self.rebuild();
        app.tabs.refresh_titles();
    }

    pub fn undo(&self, redo: bool) {
        let step = if redo { self.redo.borrow_mut().pop() } else { self.undo.borrow_mut().pop() };
        let Some(step) = step else { return };
        let app = crate::ui::window::app();
        let mut inverse = UndoStep { cd: self.diagram.borrow().file.text.clone(), files: Vec::new() };
        for (path, text) in &step.files {
            match (text, app.tabs.code_doc(path)) {
                (None, Some(doc)) => {
                    inverse.files.push((path.clone(), None));
                    if redo {
                        doc.buffer.redo();
                    } else {
                        doc.buffer.undo();
                    }
                }
                (Some(old), _) => {
                    if let Ok(mut f) = TextFile::read(path) {
                        inverse.files.push((path.clone(), Some(f.text.clone())));
                        f.text = old.clone();
                        let _ = f.write(path);
                    }
                }
                _ => {}
            }
        }
        {
            let mut d = self.diagram.borrow_mut();
            let text = step.cd.clone();
            d.file.text = text;
            if let Ok(doc) = anvil_core::xml::Doc::parse(&d.file.text) {
                d.doc = doc;
            }
        }
        if redo { self.undo.borrow_mut().push(inverse) } else { self.redo.borrow_mut().push(inverse) }
        self.reload_types();
        self.rebuild();
        app.tabs.refresh_titles();
    }

    fn type_of_card(&self, i: usize) -> Option<TypeInfo> {
        let st = self.canvas.imp().state.borrow();
        st.cards.get(i).and_then(|c| c.ty).map(|t| st.types[t].clone())
    }

    fn file_text(path: &Path) -> Option<String> {
        crate::ui::window::app().tabs.code_doc(path).map(|d| d.text()).or_else(|| TextFile::read(path).ok().map(|f| f.text))
    }

    pub fn open_code(&self, card: usize, row: Option<usize>) {
        let Some(t) = self.type_of_card(card) else { return };
        let text = Self::file_text(&t.file).unwrap_or_default();
        let member = row.and_then(|r| match self.canvas.imp().state.borrow().cards[card].rows.get(r) {
            Some((RowKind::Member(m), _)) => Some(*m),
            _ => None,
        });
        let byte = member.and_then(|m| t.decl.members.get(m)).map_or(t.decl.name_range.start, |m| m.name_range.start);
        let line = text[..byte.min(text.len())].matches('\n').count() as u32 + 1;
        crate::ui::window::app().tabs.goto(&t.file, line, 1);
    }

    pub fn update_details(&self) {
        while let Some(c) = self.details.first_child() {
            self.details.remove(&c);
        }
        let sel = self.canvas.imp().state.borrow().selected;
        let Some(t) = sel.and_then(|i| self.type_of_card(i)) else {
            self.details_title.set_label(&tr("Class Details"));
            let hint = gtk::Label::builder().label(tr("Select a type to see and edit its members.")).xalign(0.0).css_classes(["hint"]).margin_start(14).margin_top(4).build();
            self.details.append(&hint);
            return;
        };
        self.details_title.set_label(&trf("Class Details - {}", &[&t.decl.name]));
        for (mi, m) in t.decl.members.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let name = gtk::Entry::builder().text(m.name.as_str()).width_chars(16).build();
            let ty = gtk::Entry::builder().text(m.ty.clone().unwrap_or_default()).width_chars(12).sensitive(m.ty.is_some() && m.kind != MemberKind::EnumValue).build();
            let access = gtk::DropDown::from_strings(&["public", "internal", "protected", "private"]);
            access.set_selected(match m.access {
                Access::Public => 0,
                Access::Internal => 1,
                Access::Protected | Access::ProtectedInternal => 2,
                _ => 3,
            });
            access.set_sensitive(m.kind != MemberKind::EnumValue);
            let kind = gtk::Label::builder().label(format!("{:?}", m.kind)).css_classes(["chip"]).build();
            row.append(&kind);
            row.append(&name);
            row.append(&ty);
            row.append(&access);
            self.details.append(&row);
            let doc_weak = Rc::downgrade(&self.self_rc());
            let (tname, file, mname, mkind) = (t.full_name.clone(), t.file.clone(), m.name.clone(), m.kind);
            let commit_name = {
                let doc_weak = doc_weak.clone();
                let (tname, file, mname) = (tname.clone(), file.clone(), mname.clone());
                move |e: &gtk::Entry| {
                    let new = e.text().trim().to_owned();
                    if new.is_empty() || new == mname {
                        return;
                    }
                    let Some(doc) = doc_weak.upgrade() else { return };
                    let Some(text) = Self::file_text(&file) else { return };
                    let st = csharp::parse(&text);
                    if let Ok(edit) = planner::rename_member_decl(&st, &tname, &mname, mkind, &new) {
                        doc.apply_code(vec![(file.clone(), vec![edit])], |_| {});
                    }
                }
            };
            name.connect_activate(commit_name);
            let commit_type = {
                let doc_weak = doc_weak.clone();
                let (tname, file, mname) = (tname.clone(), file.clone(), mname.clone());
                move |e: &gtk::Entry| {
                    let Some(doc) = doc_weak.upgrade() else { return };
                    let Some(text) = Self::file_text(&file) else { return };
                    let st = csharp::parse(&text);
                    if let Ok(edit) = planner::set_member_type(&text, &st, &tname, &mname, mkind, e.text().trim()) {
                        doc.apply_code(vec![(file.clone(), vec![edit])], |_| {});
                    }
                }
            };
            ty.connect_activate(commit_type);
            access.connect_selected_notify(move |dd| {
                let Some(doc) = doc_weak.upgrade() else { return };
                let Some(text) = Self::file_text(&file) else { return };
                let st = csharp::parse(&text);
                let Some(t) = st.find(&tname) else { return };
                let Some(m) = t.members.get(mi) else { return };
                let a = [Access::Public, Access::Internal, Access::Protected, Access::Private][dd.selected() as usize % 4];
                if m.access == a {
                    return;
                }
                let edit = planner::set_access(&text, m.range.start, m.range.end, &m.modifiers, a);
                doc.apply_code(vec![(file.clone(), vec![edit])], |_| {});
            });
        }
    }

    fn self_rc(&self) -> Rc<DiagramDoc> {
        crate::ui::window::app()
            .tabs
            .find(&self.path.borrow())
            .and_then(|(_, d)| match &*d {
                crate::ui::editor::Doc::Diagram(x) => Some(x.clone()),
                _ => None,
            })
            .expect("the diagram is open")
    }

    /// Adds a member through the planner (card context menu, toolbox).
    fn add_member(&self, card: usize, kind: NewMember) {
        let Some(t) = self.type_of_card(card) else { return };
        let Some(text) = Self::file_text(&t.file) else { return };
        let st = csharp::parse(&text);
        let (mut name, ty) = kind.defaults(&t.decl.name);
        // Next free name: Method, Method1, … as VS numbers them.
        if !matches!(kind, NewMember::Constructor | NewMember::Destructor) {
            let base = name.clone();
            let mut n = 1;
            while t.decl.members.iter().any(|m| m.name == name) {
                name = format!("{base}{n}");
                n += 1;
            }
        }
        // VS' Class Designer adds public members.
        let access = Access::Public;
        match planner::add_member(&text, &st, &t.full_name, kind, &name, ty, access) {
            Ok(edit) => self.apply_code(vec![(t.file.clone(), vec![edit])], |_| {}),
            Err(e) => window::show_error(&tr("Add member"), &e.to_string()),
        }
    }

    /// Adds a new type: a new file like VS' Class Designer writes it and a
    /// shape at (wx, wy).
    fn add_type(&self, kind: TypeKind, abstract_: bool, wx: f32, wy: f32) {
        let app = crate::ui::window::app();
        let Some(p) = app.solution.borrow().as_ref().and_then(|s| s.project_named(&self.project).cloned()) else { return };
        let base = match kind {
            TypeKind::Interface => "IInterface",
            TypeKind::Enum => "Enum",
            TypeKind::Struct => "Struct",
            TypeKind::Record => "Record",
            TypeKind::Delegate => "Delegate",
            _ => "Class",
        };
        let name = (1..).map(|n| format!("{base}{n}")).find(|n| !p.dir.join(format!("{n}.cs")).exists()).unwrap_or_default();
        let dialog = adw::AlertDialog::new(Some(&trf("New {}", &[kind.keyword()])), None);
        let grid = gtk::Grid::builder().row_spacing(8).column_spacing(8).build();
        let name_e = gtk::Entry::builder().text(name.as_str()).activates_default(true).build();
        let access = gtk::DropDown::from_strings(&["public", "internal"]);
        let file_e = gtk::Entry::builder().text(format!("{name}.cs")).build();
        let n2 = file_e.clone();
        name_e.connect_changed(move |e| n2.set_text(&format!("{}.cs", e.text())));
        grid.attach(&gtk::Label::new(Some(&tr("Name"))), 0, 0, 1, 1);
        grid.attach(&name_e, 1, 0, 1, 1);
        grid.attach(&gtk::Label::new(Some(&tr("Access"))), 0, 1, 1, 1);
        grid.attach(&access, 1, 1, 1, 1);
        grid.attach(&gtk::Label::new(Some(&tr("File name"))), 0, 2, 1, 1);
        grid.attach(&file_e, 1, 2, 1, 1);
        dialog.set_extra_child(Some(&grid));
        dialog.add_responses(&[("cancel", &tr("Cancel")), ("ok", &tr("OK"))]);
        dialog.set_default_response(Some("ok"));
        let me = self.self_rc();
        dialog.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
            if r != "ok" {
                return;
            }
            let tname = anvil_core::templates::safe_identifier(name_e.text().trim());
            let file = p.dir.join(file_e.text().trim());
            if file.exists() {
                window::show_error(&tr("New type"), &trf("{} already exists.", &[&file.display().to_string()]));
                return;
            }
            let acc = if access.selected() == 0 { Access::Public } else { Access::Internal };
            let ns = p.root_namespace();
            let mut text = planner::new_type_file(kind, &tname, &ns, acc);
            if abstract_ {
                text = text.replacen(&format!("{} class", acc.keyword()), &format!("{} abstract class", acc.keyword()), 1);
            }
            if let Err(e) = TextFile::vs_new(&text).write(&file) {
                window::show_error(&tr("New type"), &e.to_string());
                return;
            }
            me.undo.borrow_mut().push(UndoStep { cd: me.diagram.borrow().file.text.clone(), files: Vec::new() });
            let hash = hashcode::to_base64(&hashcode::EMPTY);
            let fname = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            me.diagram.borrow_mut().add_shape(ShapeKind::for_type(kind), &format!("{ns}.{tname}"), cd::px_to_inches(wx as f64).max(0.0), cd::px_to_inches(wy as f64).max(0.0), &hash, &fname);
            me.reload_types();
            me.rebuild();
            crate::ui::window::app().explorer.reload();
            crate::ui::window::app().tabs.refresh_titles();
        });
    }

    /// Shows the types declared in a `.cs` file (dragged from the explorer).
    pub fn show_types_of_file(&self, file: &Path, wx: f32, wy: f32) {
        self.reload_types();
        let types: Vec<TypeInfo> = self.canvas.imp().state.borrow().types.iter().filter(|t| t.file == file).cloned().collect();
        self.add_shapes(&types, wx, wy);
    }

    fn add_shapes(&self, types: &[TypeInfo], wx: f32, wy: f32) {
        let existing: Vec<String> = self.diagram.borrow().shapes().iter().map(|s| s.name.clone()).collect();
        let learned = self.learned.borrow().clone();
        let mut x = wx;
        self.edit_cd(|d| {
            for t in types.iter().filter(|t| !existing.contains(&t.full_name)) {
                let names = hashcode::hashed_names(&t.decl.members);
                let hash = learned.compute(names.iter().copied()).map(|b| hashcode::to_base64(&b)).unwrap_or_else(|| hashcode::to_base64(&hashcode::EMPTY));
                let fname = t.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                d.add_shape(ShapeKind::for_type(t.decl.kind), &t.full_name, cd::px_to_inches(x as f64).max(0.0), cd::px_to_inches(wy as f64).max(0.0), &hash, &fname);
                x += 192.0;
            }
        });
    }

    fn connect_cards(&self, tool: Tool, a: usize, b: usize) {
        let (Some(ta), Some(tb)) = (self.type_of_card(a), self.type_of_card(b)) else { return };
        match tool {
            Tool::Inheritance => {
                let Some(text) = Self::file_text(&ta.file) else { return };
                let st = csharp::parse(&text);
                let res = if tb.decl.kind == TypeKind::Interface { planner::add_interface(&text, &st, &ta.full_name, &tb.decl.name) } else { planner::set_base(&text, &st, &ta.full_name, &tb.decl.name) };
                match res {
                    Ok(e) => {
                        self.apply_code(vec![(ta.file.clone(), vec![e])], |_| {});
                        if tb.decl.kind == TypeKind::Interface {
                            // VS stubs the members through Roslyn's
                            // "Implement interface"; the quick action on
                            // the base list does the same here.
                            window::toast(&tr("Use Quick Actions (Ctrl+.) on the interface name to implement its members."));
                        }
                    }
                    Err(e) => window::show_error(&tr("Inheritance"), &e.to_string()),
                }
            }
            Tool::Association => {
                let target = tb.decl.name.clone();
                let member = ta.decl.members.iter().find(|m| m.ty.as_deref().is_some_and(|t| t.trim_end_matches('?') == target)).map(|m| (m.name.clone(), false)).or_else(|| {
                    ta.decl.members.iter().find(|m| m.ty.as_deref().is_some_and(|t| t.contains(&format!("<{target}>")) || t == format!("{target}[]"))).map(|m| (m.name.clone(), true))
                });
                match member {
                    Some((m, coll)) => self.edit_cd(|d| d.show_as_association(&ta.full_name, &m, coll)),
                    None => window::toast(&trf("{} has no field or property of type {}.", &[&ta.decl.name, &target])),
                }
            }
        }
    }

    fn delete_card(&self, i: usize, delete_code: bool) {
        let name = self.canvas.imp().state.borrow().cards[i].name.clone();
        if delete_code && let Some(t) = self.type_of_card(i) {
            let Some(text) = Self::file_text(&t.file) else { return };
            let st = csharp::parse(&text);
            if let Ok(e) = planner::delete_type(&text, &st, &t.full_name) {
                self.apply_code(vec![(t.file.clone(), vec![e])], |d| d.remove_shape(&name));
            }
        } else {
            self.edit_cd(|d| d.remove_shape(&name));
        }
        self.canvas.st().selected = None;
    }

    fn delete_member(&self, card: usize, row: usize) {
        let Some(t) = self.type_of_card(card) else { return };
        let member = match self.canvas.imp().state.borrow().cards[card].rows.get(row) {
            Some((RowKind::Member(m), _)) => *m,
            _ => return,
        };
        let Some(m) = t.decl.members.get(member) else { return };
        let Some(text) = Self::file_text(&t.file) else { return };
        let st = csharp::parse(&text);
        if let Ok(e) = planner::delete_member(&text, &st, &t.full_name, &m.name, Some(m.kind)) {
            self.apply_code(vec![(t.file.clone(), vec![e])], |_| {});
        }
    }

    /// Delete key: VS asks whether to remove from the diagram only or the
    /// code too.
    fn ask_delete(&self) {
        let (sel, row) = {
            let st = self.canvas.imp().state.borrow();
            (st.selected, st.selected_row)
        };
        let Some(i) = sel else { return };
        if let Some(r) = row {
            self.delete_member(i, r);
            return;
        }
        let d = adw::AlertDialog::new(Some(&tr("Delete")), Some(&tr("Remove the shape from the diagram only, or delete the type from the code too?")));
        d.add_responses(&[("cancel", &tr("Cancel")), ("code", &tr("Delete Code")), ("diagram", &tr("Remove from Diagram"))]);
        d.set_response_appearance("code", adw::ResponseAppearance::Destructive);
        d.set_default_response(Some("diagram"));
        let me = self.self_rc();
        d.choose(Some(&crate::ui::window::app().window), gio::Cancellable::NONE, move |r| match r.as_str() {
            "diagram" => me.delete_card(i, false),
            "code" => me.delete_card(i, true),
            _ => {}
        });
    }

    pub fn rename_selected(&self) {
        let Some(i) = self.canvas.imp().state.borrow().selected else { return };
        let Some(t) = self.type_of_card(i) else { return };
        // A type rename is Roslyn's job; open the code at the name and run
        // Rename there so every reference follows.
        self.open_code(i, None);
        glib::idle_add_local_once(crate::ui::lsp::rename);
        let _ = t;
    }

    fn layout(&self) {
        let st = self.canvas.imp().state.borrow();
        let sizes: Vec<anvil_core::layout::Size> = st.cards.iter().map(|c| anvil_core::layout::Size { w: c.h as f64, h: c.w as f64 }).collect();
        // Base classes above derived ones: the layered layout runs left to
        // right, so it gets transposed sizes and its result is transposed.
        let edges: Vec<(usize, usize)> = st.edges.iter().filter(|e| e.2).map(|e| (e.1, e.0)).collect();
        let names: Vec<String> = st.cards.iter().map(|c| c.name.clone()).collect();
        drop(st);
        let pts = anvil_core::layout::layered(&sizes, &edges);
        self.edit_cd(|d| {
            for (n, p) in names.iter().zip(pts) {
                d.move_shape(n, cd::px_to_inches(p.y + 24.0), cd::px_to_inches(p.x + 24.0));
            }
        });
        self.canvas.fit();
    }

    fn adjust_widths(&self) {
        let ctx = self.canvas.pango_context();
        let st = self.canvas.imp().state.borrow();
        let widths: Vec<(String, f64)> = st
            .cards
            .iter()
            .map(|c| {
                let w = c.rows.iter().map(|(_, t)| text_width(&ctx, t, MEMBER_FACE)).fold(text_width(&ctx, short_name(&c.name), "Inter Semi-Bold 10.5") + 90.0, f32::max) + 2.0 * PAD_X;
                // VS widths are in quarter inches.
                (c.name.clone(), ((cd::px_to_inches(w as f64) * 4.0).ceil() / 4.0).max(1.5))
            })
            .collect();
        drop(st);
        self.edit_cd(|d| {
            for (n, w) in widths {
                d.set_width(&n, w);
            }
        });
    }

    fn export(&self) {
        let Some((node, bounds)) = self.canvas.export_node() else { return };
        let stem = self.path.borrow().file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let fd = gtk::FileDialog::builder().title(tr("Export Diagram as Image")).initial_name(format!("{stem}.png")).build();
        fd.save(Some(&crate::ui::window::app().window), gio::Cancellable::NONE, move |r| {
            let Ok(f) = r else { return };
            let Some(path) = f.path() else { return };
            if let Err(e) = export_to(&node, &bounds, &path) {
                window::show_error(&tr("Export Diagram as Image"), &e);
            } else {
                window::toast(&trf("{} written.", &[&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()]));
            }
        });
    }

    pub fn command(&self, id: &str) {
        match id {
            "designer.delete" => self.ask_delete(),
            "designer.delete-code" => {
                if let Some(i) = self.canvas.imp().state.borrow().selected {
                    let me = self.self_rc();
                    glib::idle_add_local_once(move || me.delete_card(i, true));
                }
            }
            "designer.layout" => self.layout(),
            // The zoom is read first: `zoom_to` borrows the state mutably.
            "designer.zoom-in" | "designer.zoom-out" => {
                let z = self.canvas.imp().state.borrow().zoom() as f64;
                self.canvas.zoom_to(if id == "designer.zoom-in" { z * 1.25 } else { z / 1.25 }, None);
            }
            "designer.export" => self.export(),
            _ => {}
        }
    }

    fn card_menu(&self, w: &gtk::Widget, x: f64, y: f64, i: usize, row: Option<usize>) {
        let me = self.self_rc();
        let run = |label: String, f: Rc<dyn Fn()>| Item::Run(label, f);
        let add = |kind: NewMember, label: String| {
            let me = me.clone();
            run(label, Rc::new(move || me.add_member(i, kind)))
        };
        let card = self.canvas.imp().state.borrow().cards[i].clone();
        let is_enum = card.kind == ShapeKind::Enum;
        let mut items = vec![];
        if card.ty.is_some() && !is_enum {
            items.push(Item::Submenu(
                tr("Add"),
                vec![
                    add(NewMember::Method, tr("Method")),
                    add(NewMember::Property, tr("Property")),
                    add(NewMember::Field, tr("Field")),
                    add(NewMember::Event, tr("Event")),
                    add(NewMember::Constructor, tr("Constructor")),
                    add(NewMember::Destructor, tr("Destructor")),
                    add(NewMember::Constant, tr("Constant")),
                ],
            ));
            items.push(Item::Separator);
        }
        if let Some(r) = row
            && matches!(card.rows.get(r), Some((RowKind::Member(_), _)))
            && let Some(t) = self.type_of_card(i)
            && let Some(RowKind::Member(m)) = card.rows.get(r).map(|x| x.0.clone())
        {
            let mname = t.decl.members[m].name.clone();
            let tname = t.full_name.clone();
            let (me1, me2, me3) = (me.clone(), me.clone(), me.clone());
            let (n1, n2, t1, t2) = (mname.clone(), mname.clone(), tname.clone(), tname.clone());
            items.push(run(tr("Show as Association"), Rc::new(move || me1.edit_cd(|d| d.show_as_association(&t1, &n1, false)))));
            items.push(run(tr("Show as Collection Association"), Rc::new(move || me2.edit_cd(|d| d.show_as_association(&t2, &n2, true)))));
            items.push(run(tr("Delete Code"), Rc::new(move || me3.delete_member(i, r))));
            items.push(Item::Separator);
        }
        let (m1, m2, m3, m4, m5) = (me.clone(), me.clone(), me.clone(), me.clone(), me.clone());
        let name = card.name.clone();
        let n2 = name.clone();
        items.push(run(tr("View Code"), Rc::new(move || m1.open_code(i, row))));
        items.push(run(tr("Show Base Class"), Rc::new(move || m2.show_related(i, true))));
        items.push(run(tr("Show Derived Classes"), Rc::new(move || m3.show_related(i, false))));
        items.push(run(if card.collapsed { tr("Expand") } else { tr("Collapse") }, Rc::new(move || {
            let c = m4.canvas.imp().state.borrow().cards[i].collapsed;
            m4.edit_cd(|d| d.set_collapsed(&name, !c));
        })));
        items.push(Item::Separator);
        items.push(run(tr("Remove from Diagram"), Rc::new(move || m5.edit_cd(|d| d.remove_shape(&n2)))));
        items.push(Item::Command("designer.delete-code"));
        crate::ui::menus::popup(w, x, y, &items);
    }

    fn canvas_menu(&self, w: &gtk::Widget, x: f64, y: f64, wx: f32, wy: f32) {
        let me = self.self_rc();
        let add = |label: String, kind: TypeKind, abs: bool| {
            let me = me.clone();
            Item::Run(label, Rc::new(move || me.add_type(kind, abs, wx, wy)))
        };
        let comment = {
            let me = me.clone();
            Item::Run(tr("Comment"), Rc::new(move || me.edit_cd(|d| d.add_comment(&tr("Comment"), cd::px_to_inches(wx as f64), cd::px_to_inches(wy as f64)))))
        };
        let format = |label: String, f: MembersFormat| {
            let me = me.clone();
            Item::Run(label, Rc::new(move || me.edit_cd(|d| d.set_members_format(f))))
        };
        let group = |label: String, g: &'static str| {
            let me = me.clone();
            Item::Run(
                label,
                Rc::new(move || {
                    me.edit_cd(|d| {
                        let e = if g == "Kind" {
                            d.doc.el(0).attrs.iter().find(|a| a.name == "GroupingSetting").map(|a| {
                                let start = d.doc.text[..a.value_span.start].rfind(" GroupingSetting=").unwrap_or(a.value_span.start);
                                anvil_core::edit::TextEdit::new(start, a.value_span.end + 1, "")
                            })
                        } else {
                            Some(d.doc.set_attr(0, "GroupingSetting", g))
                        };
                        if let Some(e) = e {
                            d.apply(&[e]);
                        }
                    })
                }),
            )
        };
        let (l, a, e) = (me.clone(), me.clone(), me.clone());
        let items = vec![
            Item::Submenu(
                tr("Add"),
                vec![
                    add(tr("Class"), TypeKind::Class, false),
                    add(tr("Enum"), TypeKind::Enum, false),
                    add(tr("Interface"), TypeKind::Interface, false),
                    add(tr("Abstract Class"), TypeKind::Class, true),
                    add(tr("Struct"), TypeKind::Struct, false),
                    add(tr("Record"), TypeKind::Record, false),
                    add(tr("Delegate"), TypeKind::Delegate, false),
                    comment,
                ],
            ),
            Item::Separator,
            Item::Submenu(tr("Change Members Format"), vec![format(tr("Display Name"), MembersFormat::Name), format(tr("Display Name and Type"), MembersFormat::NameAndType), format(tr("Display Full Signature"), MembersFormat::FullSignature)]),
            Item::Submenu(tr("Group Members"), vec![group(tr("By Kind"), "Kind"), group(tr("By Access"), "Access"), group(tr("Sort Alphabetically"), "Alphabetical")]),
            Item::Run(tr("Layout Diagram"), Rc::new(move || l.layout())),
            Item::Run(tr("Adjust Shapes Width"), Rc::new(move || a.adjust_widths())),
            Item::Separator,
            Item::Run(tr("Export Diagram as Image…"), Rc::new(move || e.export())),
        ];
        crate::ui::menus::popup(w, x, y, &items);
    }

    fn show_related(&self, i: usize, base: bool) {
        let Some(t) = self.type_of_card(i) else { return };
        let all = self.canvas.imp().state.borrow().types.clone();
        let related: Vec<TypeInfo> = if base {
            all.iter().filter(|o| t.decl.bases.iter().any(|b| b.split('<').next() == Some(o.decl.name.as_str()))).cloned().collect()
        } else {
            all.iter().filter(|o| o.decl.bases.iter().any(|b| b.split('<').next() == Some(t.decl.name.as_str()))).cloned().collect()
        };
        if related.is_empty() {
            window::toast(&if base { tr("The base class is not in this project.") } else { tr("No derived classes in this project.") });
            return;
        }
        let c = self.canvas.imp().state.borrow().cards[i].clone();
        self.add_shapes(&related, c.x, if base { (c.y - 220.0).max(0.0) } else { c.y + c.h + 80.0 });
    }
}

fn export_to(node: &gsk::RenderNode, bounds: &graphene::Rect, path: &Path) -> Result<(), String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
    let (w, h) = (bounds.width() as f64, bounds.height() as f64);
    let draw = |ctx: &cairo::Context| {
        ctx.translate(-bounds.x() as f64, -bounds.y() as f64);
        node.draw(ctx);
    };
    match ext.as_str() {
        "svg" => {
            let s = cairo::SvgSurface::new(w, h, Some(path)).map_err(|e| e.to_string())?;
            draw(&cairo::Context::new(&s).map_err(|e| e.to_string())?);
            s.finish();
        }
        "pdf" => {
            let s = cairo::PdfSurface::new(w, h, path).map_err(|e| e.to_string())?;
            draw(&cairo::Context::new(&s).map_err(|e| e.to_string())?);
            s.finish();
        }
        _ => {
            let scale = 2.0;
            let s = cairo::ImageSurface::create(cairo::Format::ARgb32, (w * scale) as i32, (h * scale) as i32).map_err(|e| e.to_string())?;
            {
                let ctx = cairo::Context::new(&s).map_err(|e| e.to_string())?;
                ctx.scale(scale, scale);
                draw(&ctx);
            }
            let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
            s.write_to_png(&mut f).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn build_frame(canvas: &Canvas) -> (gtk::Box, gtk::ListBox, gtk::Label, gtk::Label) {
    // The Class Designer toolbox: types on the left, tools below.
    let toolbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    toolbox.add_css_class("toolbox");
    toolbox.set_valign(gtk::Align::Start);
    toolbox.set_halign(gtk::Align::Start);
    toolbox.set_margin_start(12);
    toolbox.set_margin_top(12);
    let tool_button = |label: &str| {
        let b = gtk::Button::with_label(label);
        b.add_css_class("flat");
        if let Some(l) = b.child().and_downcast::<gtk::Label>() {
            l.set_xalign(0.0);
        }
        b
    };
    for (label, kind, abs) in [
        (tr("Class"), TypeKind::Class, false),
        (tr("Enum"), TypeKind::Enum, false),
        (tr("Interface"), TypeKind::Interface, false),
        (tr("Abstract Class"), TypeKind::Class, true),
        (tr("Struct"), TypeKind::Struct, false),
        (tr("Record"), TypeKind::Record, false),
        (tr("Delegate"), TypeKind::Delegate, false),
    ] {
        let b = tool_button(&label);
        let c = canvas.downgrade();
        b.connect_clicked(move |_| {
            if let Some(c) = c.upgrade()
                && let Some(d) = c.doc()
            {
                let (wx, wy) = c.imp().state.borrow().to_world(c.width() as f64 / 2.0, c.height() as f64 / 3.0);
                d.add_type(kind, abs, wx, wy);
            }
        });
        toolbox.append(&b);
    }
    for (label, tool) in [(tr("Inheritance"), Tool::Inheritance), (tr("Association"), Tool::Association)] {
        let b = tool_button(&label);
        let c = canvas.downgrade();
        b.connect_clicked(move |_| {
            if let Some(c) = c.upgrade() {
                c.st().tool = Some((tool, None));
                crate::ui::window::app().status.flash(&tr("Click the first type, then the second."));
            }
        });
        toolbox.append(&b);
    }
    let zoom_label = gtk::Label::builder().label("100 %").css_classes(["count"]).build();
    toolbox.append(&zoom_label);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(canvas));
    overlay.add_overlay(&toolbox);

    let details_title = gtk::Label::builder().label(tr("Class Details")).xalign(0.0).css_classes(["panel-title"]).margin_start(14).margin_top(8).build();
    let details = gtk::ListBox::new();
    details.set_selection_mode(gtk::SelectionMode::None);
    details.add_css_class("results");
    let details_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    details_box.add_css_class("details-panel");
    details_box.append(&details_title);
    details_box.append(&gtk::ScrolledWindow::builder().child(&details).vexpand(true).build());
    details_box.set_size_request(-1, 150);

    // Class Details sits below the canvas at a fixed height, as VS docks
    // it; the canvas takes the rest.
    overlay.set_vexpand(true);
    details_box.set_size_request(-1, 170);
    details_box.set_vexpand(false);
    let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
    widget.add_css_class("designer");
    widget.append(&overlay);
    widget.append(&details_box);
    (widget, details, details_title, zoom_label)
}

/// The types declared in `sources` (path, open text); files without an
/// open text are read from disk. Pure, so it runs off the main thread.
fn parse_types(sources: Vec<(PathBuf, Option<String>)>) -> Vec<TypeInfo> {
    let mut types = Vec::new();
    for (f, text) in sources {
        let text = text.or_else(|| TextFile::read(&f).ok().map(|t| t.text)).unwrap_or_default();
        let st = csharp::parse(&text);
        for (outer, t) in st.all_types() {
            types.push(TypeInfo { full_name: t.full_name(&outer), decl: t.clone(), file: f.clone() });
        }
    }
    types
}

/// Called when a code buffer changed: open diagrams of that project
/// follow within one parse (150 ms after typing stops).
pub fn code_changed(path: &Path) {
    thread_local! {
        static TIMER: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
        static PENDING: RefCell<BTreeSet<PathBuf>> = RefCell::default();
        static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
    if path.extension().is_none_or(|e| e != "cs") {
        return;
    }
    PENDING.with(|p| p.borrow_mut().insert(path.to_owned()));
    TIMER.with(|t| {
        if let Some(id) = t.borrow_mut().take() {
            id.remove();
        }
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(150), || {
            TIMER.with(|t| t.borrow_mut().take());
            PENDING.with(|p| p.borrow_mut().clear());
            let generation = GENERATION.with(|g| {
                g.set(g.get() + 1);
                g.get()
            });
            // Reading and parsing every file of the project takes longer
            // than a frame in a medium solution: a worker does it, and only
            // the newest pass reaches the canvas.
            for d in open_diagrams() {
                let sources = d.type_sources();
                let weak = Rc::downgrade(&d);
                glib::spawn_future_local(async move {
                    let Ok(types) = gio::spawn_blocking(move || parse_types(sources)).await else { return };
                    if GENERATION.with(|g| g.get()) != generation {
                        return;
                    }
                    if let Some(d) = weak.upgrade() {
                        let _s = crate::frame_log::span("designer rebuild");
                        d.canvas.st().types = types;
                        d.rebuild();
                    }
                });
            }
        });
        *t.borrow_mut() = Some(id);
    });
}

fn open_diagrams() -> Vec<Rc<DiagramDoc>> {
    let app = crate::ui::window::app();
    app.tabs
        .open_paths()
        .into_iter()
        .filter_map(|p| app.tabs.find(&p))
        .filter_map(|(_, d)| match &*d {
            crate::ui::editor::Doc::Diagram(x) => Some(x.clone()),
            _ => None,
        })
        .collect()
}

/// Project → Add → Class Diagram: a new empty `ClassDiagramN.cd`.
pub fn add_class_diagram() {
    let Some((_, dir)) = crate::ui::explorer::target_folder() else { return };
    let name = anvil_core::templates::next_free_name(&dir, "ClassDiagram", ".cd");
    let path = dir.join(&name);
    if let Err(e) = anvil_core::text::write_atomic(&path, &Diagram::new_empty().to_bytes()) {
        window::show_error(&tr("Class Diagram"), &e.to_string());
        return;
    }
    crate::ui::window::app().explorer.reload();
    crate::ui::window::app().tabs.open(&path, false);
}

/// Project → View Class Diagram: a new diagram with every type of the
/// project, laid out.
pub fn view_class_diagram() {
    let app = crate::ui::window::app();
    let Some(project) = crate::ui::explorer::selected_project().or_else(crate::ui::build::current_project) else {
        app.status.flash(&tr("Select a project first."));
        return;
    };
    let Some(p) = app.solution.borrow().as_ref().and_then(|s| s.project_named(&project).cloned()) else { return };
    let name = anvil_core::templates::next_free_name(&p.dir, "ClassDiagram", ".cd");
    let path = p.dir.join(&name);
    if anvil_core::text::write_atomic(&path, &Diagram::new_empty().to_bytes()).is_err() {
        return;
    }
    app.explorer.reload();
    if let Some(d) = app.tabs.open(&path, false)
        && let crate::ui::editor::Doc::Diagram(doc) = &*d
    {
        let types = doc.canvas.imp().state.borrow().types.clone();
        doc.add_shapes(&types, 24.0, 24.0);
        doc.layout();
        let _ = doc.save();
    }
    let _ = HashMap::<(), ()>::new();
    let _ = Cell::new(0);
}
