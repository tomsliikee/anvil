//! The variable tree of Locals, Autos, Watch and the DataTip: Name, Value
//! and Type columns as in Visual Studio. Objects expand into their members
//! (fetched from the debugger when a row is first shown), collections show
//! `Count = n`, and values that changed since the last stop are red.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use anvil_dap::client::{Client, Variable};
use anvil_dap::display::{self, Snapshot};
use gtk::{gio, glib};

use crate::i18n::tr;

#[derive(Clone)]
pub struct Item {
    pub var: Variable,
    /// The text in the Value column.
    pub shown: String,
    pub changed: bool,
    client: Option<Client>,
    snapshot: Rc<RefCell<Snapshot>>,
}

impl Item {
    /// A row without a debugger behind it (a watch while not stopped).
    pub fn plain(name: &str, value: &str) -> Item {
        Item {
            var: Variable { name: name.into(), value: value.into(), ty: None, variables_reference: 0, evaluate_name: None },
            shown: value.into(),
            changed: false,
            client: None,
            snapshot: Rc::default(),
        }
    }
}

/// Rows for `vars`: collection counts are fetched, changes recorded.
pub async fn items(client: &Client, frame: Option<i64>, vars: Vec<Variable>, snapshot: &Rc<RefCell<Snapshot>>) -> Vec<Item> {
    let mut out = Vec::with_capacity(vars.len());
    for v in vars {
        let mut shown = v.value.clone();
        if let Some(expr) = display::count_expression(&v)
            && let Ok(c) = client.evaluate(&expr, frame, "watch").await
            && c.value.chars().all(|ch| ch.is_ascii_digit())
        {
            shown = display::with_count(&c.value);
        }
        let key = v.evaluate_name.clone().unwrap_or_else(|| v.name.clone());
        // What the user sees is compared, so a list that grew is marked.
        let changed = snapshot.borrow_mut().update(&key, &shown);
        out.push(Item { var: v, shown, changed, client: Some(client.clone()), snapshot: snapshot.clone() });
    }
    out
}

pub struct VarTree {
    pub widget: gtk::ScrolledWindow,
    pub view: gtk::ColumnView,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
}

fn item_of(obj: &glib::Object) -> Option<Item> {
    let row = obj.downcast_ref::<gtk::TreeListRow>()?;
    let boxed = row.item()?.downcast::<glib::BoxedAnyObject>().ok()?;
    let item = boxed.borrow::<Item>().clone();
    Some(item)
}

fn label_column(title: &str, expand: bool, fill: impl Fn(&gtk::Label, &Item) + 'static) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, li| {
        let li = li.downcast_ref::<gtk::ListItem>().expect("list item");
        li.set_child(Some(&gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["mono"]).build()));
    });
    factory.connect_bind(move |_, li| {
        let li = li.downcast_ref::<gtk::ListItem>().expect("list item");
        let (Some(label), Some(item)) = (li.child().and_downcast::<gtk::Label>(), li.item().as_ref().and_then(item_of)) else { return };
        fill(&label, &item);
    });
    let col = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    col.set_expand(expand);
    col.set_resizable(true);
    col
}

impl VarTree {
    pub fn new() -> VarTree {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let tree = gtk::TreeListModel::new(store.clone(), false, false, |obj| {
            let boxed = obj.downcast_ref::<glib::BoxedAnyObject>()?;
            let item = boxed.borrow::<Item>().clone();
            let client = item.client.clone()?;
            if item.var.variables_reference <= 0 {
                return None;
            }
            let children = gio::ListStore::new::<glib::BoxedAnyObject>();
            let c2 = children.clone();
            glib::spawn_future_local(async move {
                let vars = client.variables(item.var.variables_reference).await.unwrap_or_default();
                let frame = crate::ui::window::app().debugger.frame();
                for it in items(&client, frame, vars, &item.snapshot).await {
                    c2.append(&glib::BoxedAnyObject::new(it));
                }
            });
            Some(children.upcast())
        });
        let selection = gtk::SingleSelection::new(Some(tree));
        selection.set_autoselect(false);
        let view = gtk::ColumnView::new(Some(selection.clone()));
        view.add_css_class("vars");
        view.set_reorderable(false);

        // Name, with the expander.
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, li| {
            let li = li.downcast_ref::<gtk::ListItem>().expect("list item");
            let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["mono"]).build();
            let exp = gtk::TreeExpander::new();
            exp.set_child(Some(&label));
            li.set_child(Some(&exp));
        });
        factory.connect_bind(|_, li| {
            let li = li.downcast_ref::<gtk::ListItem>().expect("list item");
            let Some(exp) = li.child().and_downcast::<gtk::TreeExpander>() else { return };
            let row = li.item().and_downcast::<gtk::TreeListRow>();
            exp.set_list_row(row.as_ref());
            if let (Some(label), Some(item)) = (exp.child().and_downcast::<gtk::Label>(), li.item().as_ref().and_then(item_of)) {
                label.set_label(&item.var.name);
            }
        });
        let name = gtk::ColumnViewColumn::new(Some(&tr("Name")), Some(factory));
        name.set_resizable(true);
        name.set_fixed_width(220);
        view.append_column(&name);
        view.append_column(&label_column(&tr("Value"), true, |l, it| {
            l.set_label(&it.shown);
            l.set_tooltip_text(Some(&it.var.value));
            l.remove_css_class("changed");
            l.remove_css_class("dim");
            if it.changed {
                l.add_css_class("changed");
            } else if it.var.value == "null" {
                l.add_css_class("dim");
            }
        }));
        let ty = label_column(&tr("Type"), false, |l, it| {
            l.set_label(it.var.ty.as_deref().unwrap_or(""));
            l.add_css_class("dim");
        });
        ty.set_fixed_width(220);
        view.append_column(&ty);

        let widget = gtk::ScrolledWindow::builder().child(&view).vexpand(true).hexpand(true).build();
        VarTree { widget, view, store, selection }
    }

    pub fn set(&self, items: Vec<Item>) {
        let objs: Vec<glib::BoxedAnyObject> = items.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &objs);
    }

    pub fn clear(&self) {
        self.store.remove_all();
    }

    /// The top-level row's name under the selection (Watch: the
    /// expression).
    pub fn selected_root(&self) -> Option<String> {
        let row = self.selection.selected_item().and_downcast::<gtk::TreeListRow>()?;
        if row.depth() != 0 {
            return None;
        }
        item_of(row.upcast_ref()).map(|i| i.var.name)
    }
}
