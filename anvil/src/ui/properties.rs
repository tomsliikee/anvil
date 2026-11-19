//! Project Properties (`Alt+Enter`): the settings an assignment needs,
//! written into the `.csproj` element by element, and the command-line
//! arguments in `Properties/launchSettings.json`.

use adw::prelude::*;
use anvil_core::csproj::Csproj;
use anvil_core::text::TextFile;
use gtk::gio;

use crate::i18n::{tr, trf};
use crate::ui::window::{self};

fn entry_row(title: &str, value: &str) -> adw::EntryRow {
    let r = adw::EntryRow::new();
    r.set_title(title);
    r.set_text(value);
    r
}

/// A choice whose first entry is "(Default)": the property is absent and
/// MSBuild's default applies. A value Anvil does not list is kept as an
/// extra entry, so opening and saving never changes it.
fn combo_row(title: &str, options: &[&str], value: &str) -> (adw::ComboRow, Vec<String>) {
    let mut values: Vec<String> = std::iter::once(String::new()).chain(options.iter().map(|o| (*o).to_owned())).collect();
    if !value.is_empty() && !values.iter().any(|v| v.eq_ignore_ascii_case(value)) {
        values.push(value.to_owned());
    }
    let labels: Vec<String> = values.iter().map(|v| if v.is_empty() { tr("(Default)") } else { v.clone() }).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let r = adw::ComboRow::new();
    r.set_title(title);
    r.set_model(Some(&gtk::StringList::new(&refs)));
    r.set_selected(values.iter().position(|v| v.eq_ignore_ascii_case(value)).unwrap_or(0) as u32);
    (r, values)
}

fn selected(r: &adw::ComboRow, values: &[String], before: &str) -> String {
    let v = values.get(r.selected() as usize).cloned().unwrap_or_default();
    // Same value in other letter case: keep the file's spelling.
    if v.eq_ignore_ascii_case(before) { before.to_owned() } else { v }
}

pub fn show(project: Option<String>) {
    let app = crate::ui::window::app();
    let Some(name) = project else {
        app.status.flash(&tr("Select a project first."));
        return;
    };
    let Some(p) = app.solution.borrow().as_ref().and_then(|s| s.project_named(&name).cloned()) else { return };
    let Ok(file) = TextFile::read(&p.path) else { return };
    let Ok(c) = Csproj::parse(&file.text) else {
        window::show_error(&tr("Properties"), &tr("The project file could not be read."));
        return;
    };
    let get = |k: &str| c.property(k).unwrap_or_default();
    let outputs = ["Exe", "Library", "WinExe"];
    let nullables = ["enable", "disable", "warnings", "annotations"];
    let toggles = ["enable", "disable"];

    let app_group = adw::PreferencesGroup::new();
    app_group.set_title(&tr("Application"));
    let tfm = entry_row(&tr("Target framework"), &c.target_framework().unwrap_or_default());
    let (output, output_values) = combo_row(&tr("Output type"), &outputs, &get("OutputType"));
    let root_ns = entry_row(&tr("Default namespace"), &get("RootNamespace"));
    let assembly = entry_row(&tr("Assembly name"), &get("AssemblyName"));
    for r in [tfm.upcast_ref::<gtk::Widget>(), output.upcast_ref(), root_ns.upcast_ref(), assembly.upcast_ref()] {
        app_group.add(r);
    }
    let build_group = adw::PreferencesGroup::new();
    build_group.set_title(&tr("Build"));
    let (nullable, nullable_values) = combo_row(&tr("Nullable"), &nullables, &get("Nullable"));
    let (implicit, implicit_values) = combo_row(&tr("Implicit global usings"), &toggles, &get("ImplicitUsings"));
    let errors = adw::SwitchRow::new();
    errors.set_title(&tr("Treat warnings as errors"));
    errors.set_active(get("TreatWarningsAsErrors").eq_ignore_ascii_case("true"));
    let nowarn = entry_row(&tr("Suppress specific warnings"), &get("NoWarn"));
    for r in [nullable.upcast_ref::<gtk::Widget>(), implicit.upcast_ref(), errors.upcast_ref(), nowarn.upcast_ref()] {
        build_group.add(r);
    }
    let debug_group = adw::PreferencesGroup::new();
    debug_group.set_title(&tr("Debug"));
    let launch_path = p.dir.join("Properties/launchSettings.json");
    let launch = TextFile::read(&launch_path).ok();
    let args = entry_row(&tr("Command line arguments"), &launch.as_ref().and_then(|l| anvil_core::launch::args(&l.text)).unwrap_or_default());
    debug_group.add(&args);

    let page = adw::PreferencesPage::new();
    page.add(&app_group);
    page.add(&build_group);
    page.add(&debug_group);
    let dialog = adw::AlertDialog::new(Some(&trf("{} Properties", &[&name])), None);
    dialog.set_extra_child(Some(&gtk::ScrolledWindow::builder().child(&page).min_content_height(480).min_content_width(520).build()));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("save", &tr("Save"))]);
    dialog.set_default_response(Some("save"));
    let mut before: Vec<(&'static str, String)> = ["TargetFramework", "OutputType", "RootNamespace", "AssemblyName", "Nullable", "ImplicitUsings", "TreatWarningsAsErrors", "NoWarn"].iter().map(|k| (*k, get(k))).collect();
    // The field shows the framework even when it comes from
    // `TargetFrameworks`; untouched, nothing is written for it.
    before[0].1 = c.target_framework().unwrap_or_default();
    let warn_before = get("TreatWarningsAsErrors");
    let old_args = args.text().to_string();
    dialog.choose(Some(&app.window), gio::Cancellable::NONE, move |r| {
        if r != "save" {
            return;
        }
        let after: Vec<(&'static str, String)> = vec![
            ("TargetFramework", tfm.text().to_string()),
            ("OutputType", selected(&output, &output_values, &before[1].1)),
            ("RootNamespace", root_ns.text().to_string()),
            ("AssemblyName", assembly.text().to_string()),
            ("Nullable", selected(&nullable, &nullable_values, &before[4].1)),
            ("ImplicitUsings", selected(&implicit, &implicit_values, &before[5].1)),
            ("TreatWarningsAsErrors", if errors.is_active() == warn_before.eq_ignore_ascii_case("true") { warn_before.clone() } else if errors.is_active() { "true".into() } else { String::new() }),
            ("NoWarn", nowarn.text().to_string()),
        ];
        // Only what changed is written; one property at a time on the
        // original text, so nothing else in the file moves.
        let Ok(mut file) = TextFile::read(&p.path) else { return };
        let mut changed = false;
        for ((k, old), (_, new)) in before.iter().zip(&after) {
            if old == new {
                continue;
            }
            let Ok(c) = Csproj::parse(&file.text) else { break };
            let edits = if new.is_empty() { c.remove_property(k) } else { vec![c.set_property(k, new)] };
            file.text = anvil_core::edit::apply(&file.text, &edits);
            changed = true;
        }
        if changed && let Err(e) = file.write(&p.path) {
            window::show_error(&tr("Properties"), &e.to_string());
        }
        if args.text() != old_args {
            let f = anvil_core::launch::set_args(launch.as_ref(), &name, &args.text());
            let _ = std::fs::create_dir_all(launch_path.parent().expect("Properties folder"));
            if let Err(e) = f.write(&launch_path) {
                window::show_error(&tr("Properties"), &e.to_string());
            }
        }
        crate::ui::explorer::reload_solution_model();
    });
}
