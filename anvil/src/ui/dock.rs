//! Tool windows docked left, right or below the editor, as in VS: a panel
//! moves by dragging its tab onto another area or through its context
//! menu (Move to …), an area hides and comes back with the panel's
//! shortcut, and where everything is, how large and whether it is open is
//! part of the [`Layout`] kept per solution.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
pub use anvil_core::session::DockArea;
use anvil_core::session::{AreaState, Layout};
use gtk::{gdk, gio, glib};

use crate::i18n::tr;

/// Sizes an area gets before the user drags its edge.
fn default_size(area: DockArea) -> i32 {
    match area {
        DockArea::Left => 280,
        DockArea::Right => 320,
        DockArea::Bottom => 240,
    }
}

/// The smallest size a drag or a stored layout leaves an area.
const MIN_SIZE: i32 = 160;

/// Drag payloads carry this prefix, so a text dropped from elsewhere is
/// never taken for a panel.
const DRAG_PREFIX: &str = "anvil-panel:";

struct Panel {
    name: String,
    title: String,
    widget: gtk::Widget,
    default: DockArea,
    area: DockArea,
    available: bool,
}

struct Area {
    kind: DockArea,
    stack: adw::ViewStack,
    /// Kept alive: a dropped pages model stops emitting `items-changed`.
    pages: gtk::SelectionModel,
    card: gtk::Box,
    tabs: gtk::Box,
    placeholder: gtk::Label,
    open: Cell<bool>,
    size: Cell<i32>,
}

pub struct Dock {
    /// The whole work area: left | (editor and bottom | right).
    pub widget: gtk::Paned,
    inner: gtk::Paned,
    vpaned: gtk::Paned,
    areas: Vec<Area>,
    panels: RefCell<Vec<Panel>>,
    workspace: Cell<bool>,
    dragging: Cell<bool>,
}

fn paned(orientation: gtk::Orientation) -> gtk::Paned {
    let p = gtk::Paned::new(orientation);
    p.set_wide_handle(true);
    p.add_css_class("split");
    p
}

impl Dock {
    pub fn new(center: &impl IsA<gtk::Widget>) -> Rc<Dock> {
        let areas: Vec<Area> = DockArea::ALL.into_iter().map(Area::new).collect();
        let area = |k: DockArea| &areas[k as usize];

        let vpaned = paned(gtk::Orientation::Vertical);
        vpaned.set_start_child(Some(center));
        vpaned.set_end_child(Some(&area(DockArea::Bottom).card));
        vpaned.set_resize_end_child(false);
        vpaned.set_shrink_end_child(false);

        let inner = paned(gtk::Orientation::Horizontal);
        inner.set_start_child(Some(&vpaned));
        inner.set_end_child(Some(&area(DockArea::Right).card));
        inner.set_resize_end_child(false);
        inner.set_shrink_end_child(false);

        let widget = paned(gtk::Orientation::Horizontal);
        widget.add_css_class("workspace");
        widget.set_start_child(Some(&area(DockArea::Left).card));
        widget.set_end_child(Some(&inner));
        widget.set_resize_start_child(false);
        widget.set_shrink_start_child(false);
        widget.set_vexpand(true);

        let dock = Rc::new(Dock { widget, inner, vpaned, areas, panels: RefCell::default(), workspace: Cell::new(false), dragging: Cell::new(false) });
        for a in &dock.areas {
            dock.connect_area(a);
        }
        dock
    }

    fn area(&self, k: DockArea) -> &Area {
        &self.areas[k as usize]
    }

    fn connect_area(self: &Rc<Self>, a: &Area) {
        let weak = Rc::downgrade(self);
        let kind = a.kind;
        let rebuild = move || {
            if let Some(d) = weak.upgrade() {
                d.rebuild_tabs(kind);
            }
        };
        let r = rebuild.clone();
        a.pages.connect_items_changed(move |_, _, _, _| r());
        a.stack.connect_visible_child_name_notify(move |_| rebuild());

        // Dropping a panel's tab on an area moves the panel there.
        let drop = gtk::DropTarget::new(glib::types::Type::STRING, gdk::DragAction::MOVE);
        let weak = Rc::downgrade(self);
        drop.connect_drop(move |_, value, _, _| {
            let Some(d) = weak.upgrade() else { return false };
            let Some(name) = value.get::<String>().ok().and_then(|s| s.strip_prefix(DRAG_PREFIX).map(str::to_owned)) else { return false };
            d.move_to(&name, kind);
            true
        });
        a.card.add_controller(drop);

        let hide = gtk::Button::from_icon_name("window-close-symbolic");
        hide.add_css_class("panel-hide");
        hide.set_valign(gtk::Align::Center);
        hide.set_tooltip_text(Some(&crate::commands::tooltip("window.close-tool-window")));
        let weak = Rc::downgrade(self);
        hide.connect_clicked(move |_| {
            if let Some(d) = weak.upgrade() {
                d.close_area(kind);
            }
        });
        if let Some(header) = a.card.first_child().and_downcast::<gtk::Box>() {
            header.append(&hide);
        }
    }

    /// Registers a panel in its default area; [`Dock::apply`] moves it
    /// where the layout says.
    pub fn add(&self, widget: &impl IsA<gtk::Widget>, name: &str, title: &str, default: DockArea) {
        let widget = widget.as_ref().clone();
        self.area(default).stack.add_titled(&widget, Some(name), title);
        self.panels.borrow_mut().push(Panel { name: name.to_owned(), title: title.to_owned(), widget, default, area: default, available: true });
        self.refresh(default);
    }

    /// Debugger windows exist only during a session, the chat only with AI
    /// on: their tabs come and go without losing their place.
    pub fn set_available(&self, name: &str, on: bool) {
        let area = {
            let mut panels = self.panels.borrow_mut();
            let Some(p) = panels.iter_mut().find(|p| p.name == name) else { return };
            p.available = on;
            self.area(p.area).stack.page(&p.widget).set_visible(on);
            p.area
        };
        self.rebuild_tabs(area);
        self.refresh(area);
    }

    pub fn area_of(&self, name: &str) -> Option<DockArea> {
        self.panels.borrow().iter().find(|p| p.name == name).map(|p| p.area)
    }

    fn widget_of(&self, name: &str) -> Option<gtk::Widget> {
        self.panels.borrow().iter().find(|p| p.name == name).map(|p| p.widget.clone())
    }

    /// Whether one of `names` is the panel in front of an open area.
    pub fn is_showing(&self, names: &[&str]) -> bool {
        self.areas.iter().any(|a| a.card.is_visible() && a.open.get() && a.stack.visible_child_name().is_some_and(|n| names.contains(&n.as_str())))
    }

    /// Brings a panel to the front of its area and opens the area.
    pub fn show(&self, name: &str) {
        let Some(area) = self.area_of(name) else { return };
        let a = self.area(area);
        if a.stack.child_by_name(name).is_some_and(|w| a.stack.page(&w).is_visible()) {
            a.stack.set_visible_child_name(name);
        }
        self.open_area(area);
    }

    /// The panel's shortcut: shows and focuses it; pressed again while it
    /// has the focus, hides its area and returns to the editor.
    pub fn toggle(&self, name: &str) {
        let Some(area) = self.area_of(name) else { return };
        let a = self.area(area);
        let in_front = a.card.is_visible() && a.open.get() && a.stack.visible_child_name().as_deref() == Some(name);
        if in_front && self.focus_area() == Some(area) {
            self.close_area(area);
            return;
        }
        self.show(name);
        self.focus(name);
    }

    pub fn focus(&self, name: &str) {
        if let Some(w) = self.widget_of(name)
            && !w.child_focus(gtk::DirectionType::TabForward)
        {
            w.grab_focus();
        }
    }

    /// The area holding the keyboard focus, if any.
    pub fn focus_area(&self) -> Option<DockArea> {
        let root = self.widget.root()?;
        let mut w = root.focus();
        while let Some(widget) = w {
            if let Some(a) = self.areas.iter().find(|a| a.card == widget) {
                return Some(a.kind);
            }
            w = widget.parent();
        }
        None
    }

    /// Shift+Esc: hides the focused area, else the bottom one.
    pub fn close_focused(&self) {
        self.close_area(self.focus_area().unwrap_or(DockArea::Bottom));
    }

    pub fn open_area(&self, area: DockArea) {
        let a = self.area(area);
        if a.open.get() && a.card.is_visible() {
            return;
        }
        a.open.set(true);
        self.refresh(area);
        if a.card.is_visible() {
            self.apply_size(area);
            crate::motion::fade_in(&a.card, 0.0, crate::motion::STATE_MS);
        }
    }

    pub fn close_area(&self, area: DockArea) {
        let a = self.area(area);
        let had_focus = self.focus_area() == Some(area);
        self.capture_size(area);
        a.open.set(false);
        self.refresh(area);
        if had_focus && let Some(app) = crate::ui::window::try_app() {
            app.tabs.focus_current();
        }
    }

    /// Moves a panel to another area and shows it there.
    pub fn move_to(&self, name: &str, to: DockArea) {
        let from = {
            let mut panels = self.panels.borrow_mut();
            let Some(p) = panels.iter_mut().find(|p| p.name == name) else { return };
            if p.area == to {
                drop(panels);
                self.show(name);
                return;
            }
            let from = p.area;
            self.capture_size(from);
            self.area(from).stack.remove(&p.widget);
            let page = self.area(to).stack.add_titled(&p.widget, Some(&p.name), &p.title);
            page.set_visible(p.available);
            p.area = to;
            from
        };
        self.refresh(from);
        self.show(name);
        self.focus(name);
    }

    /// Empty areas show while a tab is dragged, so a panel can go where
    /// no panel is yet.
    fn set_dragging(&self, on: bool) {
        self.dragging.set(on);
        for k in DockArea::ALL {
            self.refresh(k);
        }
        if on {
            for k in DockArea::ALL {
                if self.area(k).card.is_visible() {
                    self.apply_size(k);
                }
            }
        }
    }

    fn has_pages(&self, area: DockArea) -> bool {
        self.panels.borrow().iter().any(|p| p.area == area && p.available)
    }

    fn refresh(&self, area: DockArea) {
        let a = self.area(area);
        let pages = self.has_pages(area);
        let was = a.card.is_visible();
        let show = self.workspace.get() && (self.dragging.get() || (a.open.get() && pages));
        a.placeholder.set_visible(!pages);
        a.stack.set_visible(pages);
        a.card.set_visible(show);
        if show && !was {
            self.apply_size(area);
        }
    }

    fn rebuild_tabs(&self, area: DockArea) {
        let a = self.area(area);
        while let Some(c) = a.tabs.first_child() {
            a.tabs.remove(&c);
        }
        let mut group: Option<gtk::ToggleButton> = None;
        let pages = a.stack.pages();
        for i in 0..pages.n_items() {
            let Some(page) = pages.item(i).and_downcast::<adw::ViewStackPage>() else { continue };
            if !page.is_visible() {
                continue;
            }
            let name = page.name().map(|n| n.to_string()).unwrap_or_default();
            let b = gtk::ToggleButton::with_label(&page.title().map(|t| t.to_string()).unwrap_or_default());
            if let Some(g) = &group {
                b.set_group(Some(g));
            } else {
                group = Some(b.clone());
            }
            b.set_active(a.stack.visible_child_name().as_deref() == Some(name.as_str()));
            let stack = a.stack.clone();
            let n = name.clone();
            b.connect_toggled(move |b| {
                if b.is_active() {
                    stack.set_visible_child_name(&n);
                }
            });
            tab_controllers(&b, &name, area);
            a.tabs.append(&b);
        }
    }

    // ---- Sizes -----------------------------------------------------------

    fn paned_of(&self, area: DockArea) -> &gtk::Paned {
        match area {
            DockArea::Left => &self.widget,
            DockArea::Right => &self.inner,
            DockArea::Bottom => &self.vpaned,
        }
    }

    /// The area's size as shown, kept for when it comes back.
    fn capture_size(&self, area: DockArea) {
        let a = self.area(area);
        if !a.card.is_visible() || !self.has_pages(area) {
            return;
        }
        let size = if area == DockArea::Bottom { a.card.height() } else { a.card.width() };
        if size > 0 {
            a.size.set(size);
        }
    }

    fn apply_size(&self, area: DockArea) {
        let a = self.area(area);
        let size = if a.size.get() > 0 { a.size.get() } else { default_size(area) };
        let p = self.paned_of(area);
        let total = if area == DockArea::Bottom { p.height() } else { p.width() };
        if total <= 0 {
            // Not laid out yet (the window is still opening): once it is.
            let weak = p.downgrade();
            glib::idle_add_local_once(move || {
                if let (Some(_), Some(app)) = (weak.upgrade(), crate::ui::window::try_app()) {
                    let p = app.dock.paned_of(area);
                    let total = if area == DockArea::Bottom { p.height() } else { p.width() };
                    if total > 0 {
                        app.dock.apply_size(area);
                    }
                }
            });
            return;
        }
        // Leave the editor at least as much as an area may take.
        let size = size.clamp(MIN_SIZE, (total - MIN_SIZE * 2).max(MIN_SIZE));
        let handle = handle_size(p);
        p.set_position(if area == DockArea::Left { size } else { total - size - handle });
    }

    // ---- Layout ----------------------------------------------------------

    pub fn layout(&self) -> Layout {
        let mut l = Layout::default();
        for p in self.panels.borrow().iter() {
            l.move_panel(&p.name, p.area, p.default);
        }
        for a in &self.areas {
            self.capture_size(a.kind);
            l.areas.insert(a.kind, AreaState { open: a.open.get(), size: a.size.get(), active: a.stack.visible_child_name().map(|n| n.to_string()) });
        }
        l
    }

    /// Puts a stored layout into effect: panels into their areas, areas
    /// open or hidden at their sizes, the front panel of each.
    pub fn apply(&self, layout: &Layout) {
        let names: Vec<(String, DockArea)> = self.panels.borrow().iter().map(|p| (p.name.clone(), layout.area_of(&p.name, p.default))).collect();
        for (name, area) in names {
            let mut panels = self.panels.borrow_mut();
            let Some(p) = panels.iter_mut().find(|p| p.name == name) else { continue };
            if p.area != area {
                self.area(p.area).stack.remove(&p.widget);
                let page = self.area(area).stack.add_titled(&p.widget, Some(&p.name), &p.title);
                page.set_visible(p.available);
                p.area = area;
            }
        }
        for a in &self.areas {
            let s = layout.area(a.kind);
            a.open.set(s.open);
            a.size.set(s.size);
            if let Some(name) = s.active.as_deref()
                && a.stack.child_by_name(name).is_some_and(|w| a.stack.page(&w).is_visible())
            {
                a.stack.set_visible_child_name(name);
            }
            self.rebuild_tabs(a.kind);
            self.refresh(a.kind);
            if a.card.is_visible() {
                self.apply_size(a.kind);
            }
        }
    }

    /// The start page stands alone; the areas come with a solution.
    pub fn set_workspace(&self, on: bool) {
        self.workspace.set(on);
        for k in DockArea::ALL {
            if !on {
                self.capture_size(k);
            }
            self.refresh(k);
        }
    }
}

/// The handle between a paned's children (wider in comfortable density).
fn handle_size(p: &gtk::Paned) -> i32 {
    let (Some(s), Some(e)) = (p.start_child(), p.end_child()) else { return 10 };
    if !s.is_visible() || !e.is_visible() {
        return 10;
    }
    let (total, a, b) = if p.orientation() == gtk::Orientation::Vertical { (p.height(), s.height(), e.height()) } else { (p.width(), s.width(), e.width()) };
    if a > 0 && b > 0 { (total - a - b).max(0) } else { 10 }
}

impl Area {
    fn new(kind: DockArea) -> Area {
        let stack = adw::ViewStack::new();
        stack.set_vexpand(true);
        // The panel tabs cross-fade like the start page and the editor.
        stack.set_enable_transitions(true);
        stack.set_transition_duration(crate::motion::STATE_MS);
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("card-panel");
        card.add_css_class("dock-area");
        card.add_css_class(kind.nick());
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        header.add_css_class("panel-header");
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        tabs.add_css_class("panel-tabs");
        // Long (German) titles scroll instead of widening the window.
        let tabs_scroller = gtk::ScrolledWindow::builder().child(&tabs).hscrollbar_policy(gtk::PolicyType::External).vscrollbar_policy(gtk::PolicyType::Never).hexpand(true).propagate_natural_height(true).build();
        header.append(&tabs_scroller);
        card.append(&header);
        card.append(&stack);
        let placeholder = gtk::Label::builder().label(tr("Drop a panel here")).css_classes(["hint", "dock-placeholder"]).vexpand(true).hexpand(true).wrap(true).justify(gtk::Justification::Center).build();
        placeholder.set_visible(false);
        card.append(&placeholder);
        if kind != DockArea::Bottom {
            card.set_width_request(MIN_SIZE + 60);
        }
        card.set_visible(false);
        let pages = stack.pages();
        Area { kind, stack, pages, card, tabs, placeholder, open: Cell::new(true), size: Cell::new(0) }
    }
}

/// Drag to move, right click (or the menu key) for Move to … and Hide.
fn tab_controllers(b: &gtk::ToggleButton, name: &str, area: DockArea) {
    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::MOVE);
    let payload = format!("{DRAG_PREFIX}{name}");
    source.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&payload.to_value())));
    let button = b.clone();
    source.connect_drag_begin(move |s, _| {
        s.set_icon(Some(&gtk::WidgetPaintable::new(Some(&button))), 0, 0);
        if let Some(app) = crate::ui::window::try_app() {
            app.dock.set_dragging(true);
        }
    });
    source.connect_drag_end(|_, _, _| {
        if let Some(app) = crate::ui::window::try_app() {
            app.dock.set_dragging(false);
        }
    });
    b.add_controller(source);

    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    let (n, button) = (name.to_owned(), b.clone());
    click.connect_pressed(move |g, _, _, _| {
        g.set_state(gtk::EventSequenceState::Claimed);
        panel_menu(&button, &n, area);
    });
    b.add_controller(click);

    let keys = gtk::EventControllerKey::new();
    let (n, button) = (name.to_owned(), b.clone());
    keys.connect_key_pressed(move |_, key, _, state| {
        if key == gdk::Key::Menu || (key == gdk::Key::F10 && state.contains(gdk::ModifierType::SHIFT_MASK)) {
            panel_menu(&button, &n, area);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    b.add_controller(keys);
}

fn area_label(area: DockArea) -> String {
    match area {
        DockArea::Left => tr("Move to Left"),
        DockArea::Right => tr("Move to Right"),
        DockArea::Bottom => tr("Move to Bottom"),
    }
}

fn panel_menu(button: &gtk::ToggleButton, name: &str, area: DockArea) {
    let menu = gio::Menu::new();
    let moves = gio::Menu::new();
    for k in DockArea::ALL.into_iter().filter(|k| *k != area) {
        moves.append(Some(&area_label(k)), Some(&format!("panel.move::{}", k.nick())));
    }
    menu.append_section(None, &moves);
    menu.append(Some(&crate::commands::title("window.close-tool-window")), Some("panel.hide"));

    let group = gio::SimpleActionGroup::new();
    let mv = gio::SimpleAction::new("move", Some(glib::VariantTy::STRING));
    let n = name.to_owned();
    mv.connect_activate(move |_, v| {
        let Some(to) = v.and_then(|v| v.str()).and_then(DockArea::from_nick) else { return };
        let n = n.clone();
        // After the popover has closed: moving reparents its button.
        glib::idle_add_local_once(move || crate::ui::window::app().dock.move_to(&n, to));
    });
    group.add_action(&mv);
    let hide = gio::SimpleAction::new("hide", None);
    hide.connect_activate(move |_, _| {
        glib::idle_add_local_once(move || crate::ui::window::app().dock.close_area(area));
    });
    group.add_action(&hide);
    button.insert_action_group("panel", Some(&group));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("anvil");
    popover.set_parent(button);
    popover.set_has_arrow(false);
    // The tabs are rebuilt often; a popover left on a dropped button warns.
    popover.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
}
