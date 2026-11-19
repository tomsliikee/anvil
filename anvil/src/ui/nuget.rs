//! NuGet Package Manager: Browse (nuget.org search), Installed, Updates;
//! install, update and uninstall through `dotnet add/remove package`,
//! with the project file's BOM and line endings kept.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use anvil_core::nuget::{self, Package};
use gtk::{gio, glib};

use crate::i18n::{tr, trf};
use crate::ui::build::dotnet_on_file;
use crate::ui::window::{self};

async fn http_get(url: String) -> Result<String, String> {
    crate::runtime::run(async move { reqwest::get(&url).await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string()) }).await
}

/// The package sources of the solution's `NuGet.Config`, else nuget.org.
fn search_endpoint_sources() -> Vec<(String, String)> {
    let dir = window::solution_dir();
    let cfg = dir.and_then(|d| ["NuGet.Config", "nuget.config", "NuGet.config"].iter().find_map(|n| std::fs::read_to_string(d.join(n)).ok()));
    match cfg {
        Some(c) => nuget::sources(&c, &nuget::default_sources()),
        None => nuget::default_sources(),
    }
}

pub fn open(project: Option<String>) {
    let app = crate::ui::window::app();
    let Some(project) = project.or_else(crate::ui::build::current_project).or_else(|| app.solution.borrow().as_ref().and_then(|s| s.first_project().map(|p| p.name.clone()))) else {
        app.status.flash(&tr("Open a solution first."));
        return;
    };
    let Some(p) = app.solution.borrow().as_ref().and_then(|s| s.project_named(&project).cloned()) else { return };

    let stack = adw::ViewStack::new();
    let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build();
    let search = gtk::SearchEntry::builder().placeholder_text(tr("Search (nuget.org)")).hexpand(true).build();
    let prerelease = gtk::CheckButton::with_label(&tr("Include prerelease"));
    let browse_list = gtk::ListBox::new();
    browse_list.add_css_class("results");
    let installed_list = gtk::ListBox::new();
    installed_list.add_css_class("results");
    let updates_list = gtk::ListBox::new();
    updates_list.add_css_class("results");
    let browse = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.append(&search);
    top.append(&prerelease);
    browse.append(&top);
    browse.append(&gtk::ScrolledWindow::builder().child(&browse_list).vexpand(true).build());
    stack.add_titled(&browse, Some("browse"), &tr("Browse"));
    stack.add_titled(&gtk::ScrolledWindow::builder().child(&installed_list).vexpand(true).build(), Some("installed"), &tr("Installed"));
    stack.add_titled(&gtk::ScrolledWindow::builder().child(&updates_list).vexpand(true).build(), Some("updates"), &tr("Updates"));

    // The details side: id, description, version picker, action.
    let title = gtk::Label::builder().xalign(0.0).css_classes(["panel-title"]).wrap(true).build();
    let desc = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["hint"]).build();
    let versions = gtk::DropDown::from_strings(&[]);
    let install = gtk::Button::with_label(&tr("Install"));
    install.add_css_class("solid");
    let uninstall = gtk::Button::with_label(&tr("Uninstall"));
    uninstall.add_css_class("quiet");
    let side = gtk::Box::new(gtk::Orientation::Vertical, 10);
    side.set_width_request(300);
    side.append(&title);
    side.append(&desc);
    side.append(&versions);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.append(&install);
    buttons.append(&uninstall);
    side.append(&buttons);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    stack.set_hexpand(true);
    body.append(&stack);
    body.append(&side);
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 10);
    outer.set_margin_top(12);
    outer.set_margin_bottom(12);
    outer.set_margin_start(12);
    outer.set_margin_end(12);
    outer.append(&switcher);
    outer.append(&body);
    let dialog = adw::Dialog::builder().title(trf("NuGet - {}", &[&project])).content_width(980).content_height(640).child(&outer).build();

    let current: Rc<RefCell<Option<String>>> = Rc::default();
    let select = {
        let (title, desc, versions, current, install, prerelease) = (title.clone(), desc.clone(), versions.clone(), current.clone(), install.clone(), prerelease.clone());
        move |id: String, description: String, installed: Option<String>| {
            title.set_label(&id);
            desc.set_label(&description);
            install.set_label(&if installed.is_some() { tr("Update") } else { tr("Install") });
            *current.borrow_mut() = Some(id.clone());
            // No install with the previous package's versions while this
            // one's are loading.
            versions.set_model(None::<&gio::ListModel>);
            install.set_sensitive(false);
            let (versions, install, current) = (versions.clone(), install.clone(), current.clone());
            let pre = prerelease.is_active();
            glib::spawn_future_local(async move {
                let list = http_get(nuget::versions_url(&id)).await.map(|j| nuget::parse_versions(&j)).unwrap_or_default();
                if current.borrow().as_deref() != Some(id.as_str()) {
                    return;
                }
                let refs: Vec<&str> = list.iter().map(String::as_str).collect();
                versions.set_model(Some(&gtk::StringList::new(&refs)));
                versions.set_selected(nuget::default_version(&list, pre) as u32);
                install.set_sensitive(true);
            });
        }
    };
    let row_for = |pkg: &Package, installed: Option<&str>| {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        head.append(&gtk::Label::builder().label(&pkg.id).xalign(0.0).hexpand(true).css_classes(["strong"]).build());
        head.append(&gtk::Label::builder().label(installed.map(|i| format!("{i} → {}", pkg.version)).unwrap_or_else(|| pkg.version.clone())).css_classes(["count"]).build());
        b.append(&head);
        b.append(&gtk::Label::builder().label(pkg.description.lines().next().unwrap_or("")).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["hint"]).build());
        b
    };
    let results: Rc<RefCell<Vec<Package>>> = Rc::default();
    {
        let (browse_list, results, prerelease) = (browse_list.clone(), results.clone(), prerelease.clone());
        search.connect_search_changed(move |e| {
            let q = e.text().to_string();
            let (list, results, pre) = (browse_list.clone(), results.clone(), prerelease.is_active());
            glib::spawn_future_local(async move {
                let endpoint = http_get(nuget::NUGET_ORG.into()).await.ok().and_then(|i| nuget::search_service(&i)).unwrap_or_else(|| nuget::NUGET_ORG_SEARCH.to_owned());
                let Ok(json) = http_get(nuget::search_url(&endpoint, &q, pre, 0)).await else { return };
                let pkgs = nuget::parse_search(&json).unwrap_or_default();
                while let Some(c) = list.first_child() {
                    list.remove(&c);
                }
                for p in &pkgs {
                    list.append(&row_for(p, None));
                }
                *results.borrow_mut() = pkgs;
            });
        });
    }
    {
        let (results, select) = (results.clone(), select.clone());
        browse_list.connect_row_activated(move |_, row| {
            if let Some(p) = results.borrow().get(row.index() as usize) {
                select(p.id.clone(), p.description.clone(), None);
            }
        });
    }
    let installed: Vec<anvil_core::csproj::PackageRef> = p.csproj.as_ref().map(|c| c.package_refs()).unwrap_or_default();
    for r in &installed {
        installed_list.append(&gtk::Label::builder().label(format!("{}  {}", r.id, r.version.clone().unwrap_or_default())).xalign(0.0).build());
    }
    {
        let (installed, select) = (installed.clone(), select.clone());
        installed_list.connect_row_activated(move |_, row| {
            if let Some(r) = installed.get(row.index() as usize) {
                select(r.id.clone(), String::new(), r.version.clone());
            }
        });
    }
    // Updates: the newest stable version of each installed package.
    {
        let (installed, updates_list, select) = (installed.clone(), updates_list.clone(), select.clone());
        glib::spawn_future_local(async move {
            let mut ups = Vec::new();
            for r in &installed {
                let newest = http_get(nuget::versions_url(&r.id)).await.map(|j| nuget::parse_versions(&j)).unwrap_or_default().into_iter().find(|v| !v.contains('-'));
                if let (Some(n), Some(cur)) = (newest, r.version.clone())
                    && n != cur
                {
                    updates_list.append(&gtk::Label::builder().label(format!("{}  {cur} → {n}", r.id)).xalign(0.0).build());
                    ups.push(r.clone());
                }
            }
            updates_list.connect_row_activated(move |_, row| {
                if let Some(r) = ups.get(row.index() as usize) {
                    select(r.id.clone(), String::new(), r.version.clone());
                }
            });
        });
    }
    let csproj = p.path.clone();
    {
        let (current, versions, csproj, dialog) = (current.clone(), versions.clone(), csproj.clone(), dialog.clone());
        install.connect_clicked(move |_| {
            let Some(id) = current.borrow().clone() else { return };
            let version = versions.selected_item().and_downcast::<gtk::StringObject>().map(|s| s.string().to_string());
            let (csproj, dialog) = (csproj.clone(), dialog.clone());
            glib::spawn_future_local(async move {
                let mut args = vec!["add".into(), csproj.to_string_lossy().into_owned(), "package".into(), id.clone()];
                if let Some(v) = version {
                    args.extend(["--version".into(), v]);
                }
                crate::ui::window::app().output.show("Package Manager");
                match dotnet_on_file(&csproj, args).await {
                    Ok(()) => {
                        window::toast(&trf("{} installed.", &[&id]));
                        dialog.close();
                        crate::ui::explorer::reload_solution_model();
                    }
                    Err(e) => window::show_error(&tr("NuGet"), &e),
                }
            });
        });
    }
    {
        let (current, csproj, dialog) = (current.clone(), csproj.clone(), dialog.clone());
        uninstall.connect_clicked(move |_| {
            let Some(id) = current.borrow().clone() else { return };
            let (csproj, dialog) = (csproj.clone(), dialog.clone());
            glib::spawn_future_local(async move {
                match dotnet_on_file(&csproj, vec!["remove".into(), csproj.to_string_lossy().into_owned(), "package".into(), id.clone()]).await {
                    Ok(()) => {
                        window::toast(&trf("{} uninstalled.", &[&id]));
                        dialog.close();
                        crate::ui::explorer::reload_solution_model();
                    }
                    Err(e) => window::show_error(&tr("NuGet"), &e),
                }
            });
        });
    }
    let sources = search_endpoint_sources();
    desc.set_label(&trf("Package sources: {}", &[&sources.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ")]));
    dialog.present(Some(&app.window));
    search.grab_focus();
}
