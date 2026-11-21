//! Test Explorer: tests found in the code of test projects, run with
//! `dotnet test` (TRX results), shown as project → namespace → class →
//! test with pass (`up`), fail (`down`) and skip marks.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use adw::prelude::*;
use anvil_core::host::Command;
use anvil_core::testing::{self, Outcome, TestId, TestResult};
use gtk::glib;

use crate::host;
use crate::i18n::{tr, trf};
use crate::ui::window::{self};

/// The tests of the last run and whether it was debugged.
type LastRun = (Vec<(String, TestId)>, bool);

pub struct TestExplorer {
    pub widget: gtk::Box,
    list: gtk::ListBox,
    detail: gtk::Label,
    summary: gtk::Label,
    /// project → tests
    tests: RefCell<BTreeMap<String, Vec<TestId>>>,
    results: RefCell<HashMap<String, TestResult>>,
    rows: RefCell<Vec<Option<(String, TestId)>>>,
    last: RefCell<Option<LastRun>>,
}

impl TestExplorer {
    pub fn new() -> TestExplorer {
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bar.set_margin_start(10);
        for (label, cmd) in [(tr("Run All"), "test.run-all"), (tr("Run Failed"), "test.run-failed"), (tr("Repeat Last Run"), "test.repeat-last"), (tr("Debug All"), "test.debug-all")] {
            let b = gtk::Button::with_label(&label);
            b.add_css_class("flat");
            b.set_action_name(Some(&format!("win.{cmd}")));
            b.set_tooltip_text(Some(&crate::commands::tooltip(cmd)));
            bar.append(&b);
        }
        let run_sel = gtk::Button::with_label(&tr("Run"));
        run_sel.add_css_class("flat");
        run_sel.connect_clicked(|_| crate::ui::window::app().tests.run_selected(false));
        bar.append(&run_sel);
        let summary = gtk::Label::builder().hexpand(true).xalign(1.0).css_classes(["count"]).margin_end(10).build();
        bar.append(&summary);
        let list = gtk::ListBox::new();
        list.add_css_class("results");
        let detail = gtk::Label::builder().xalign(0.0).yalign(0.0).wrap(true).selectable(true).css_classes(["mono"]).margin_start(12).margin_end(12).build();
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_start_child(Some(&gtk::ScrolledWindow::builder().child(&list).vexpand(true).hexpand(true).build()));
        paned.set_end_child(Some(&gtk::ScrolledWindow::builder().child(&detail).build()));
        paned.set_position(520);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 4);
        widget.append(&gtk::ScrolledWindow::builder().child(&bar).hscrollbar_policy(gtk::PolicyType::External).vscrollbar_policy(gtk::PolicyType::Never).propagate_natural_height(true).build());
        widget.append(&paned);
        TestExplorer { widget, list, detail, summary, tests: RefCell::default(), results: RefCell::default(), rows: RefCell::default(), last: RefCell::default() }
    }

    pub fn connect(&self) {
        self.list.connect_row_selected(|_, row| {
            let app = crate::ui::window::app();
            let me = &app.tests;
            let Some(row) = row else { return };
            let entry = me.rows.borrow().get(row.index() as usize).cloned().flatten();
            let text = entry
                .and_then(|(_, id)| me.results.borrow().get(&id.full_name()).cloned())
                .map(|r| format!("{}\n{} ms\n\n{}\n\n{}", r.display, r.duration_ms.round(), r.message.unwrap_or_default(), r.stack.unwrap_or_default()))
                .unwrap_or_default();
            me.detail.set_label(text.trim());
        });
        self.list.connect_row_activated(|_, row| {
            let app = crate::ui::window::app();
            let entry = app.tests.rows.borrow().get(row.index() as usize).cloned().flatten();
            if let Some((_, id)) = entry {
                goto_test(&id);
            }
        });
    }

    /// Finds tests in the code of every test project (no build needed).
    pub fn discover(&self) {
        let app = crate::ui::window::app();
        let Some(sol) = app.solution.borrow().clone() else { return };
        let mut map = BTreeMap::new();
        for p in sol.projects.iter().filter(|p| p.csproj.as_ref().is_some_and(|c| c.is_test_project())) {
            let mut tests = Vec::new();
            for f in p.source_files() {
                let text = app.tabs.code_doc(&f).map(|d| d.text()).or_else(|| anvil_core::text::TextFile::read(&f).ok().map(|t| t.text)).unwrap_or_default();
                tests.extend(testing::discover(&anvil_core::csharp::parse(&text)).into_iter().map(|(t, _)| t));
            }
            map.insert(p.name.clone(), tests);
        }
        *self.tests.borrow_mut() = map;
        self.render();
    }

    fn render(&self) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        let mut rows = Vec::new();
        let results = self.results.borrow();
        let (mut passed, mut failed, mut total) = (0, 0, 0);
        for (project, tests) in self.tests.borrow().iter() {
            self.list.append(&gtk::Label::builder().label(project).xalign(0.0).css_classes(["strong"]).build());
            rows.push(None);
            let mut by_ns: BTreeMap<String, BTreeMap<String, Vec<&TestId>>> = BTreeMap::new();
            for t in tests {
                by_ns.entry(t.namespace.clone()).or_default().entry(t.class.clone()).or_default().push(t);
            }
            for (ns, classes) in by_ns {
                self.list.append(&gtk::Label::builder().label(&ns).xalign(0.0).margin_start(16).css_classes(["dim"]).build());
                rows.push(None);
                for (class, ts) in classes {
                    self.list.append(&gtk::Label::builder().label(&class).xalign(0.0).margin_start(32).build());
                    rows.push(None);
                    for t in ts {
                        total += 1;
                        let r = results.get(&t.full_name());
                        let (mark, css) = match r.map(|r| r.outcome) {
                            Some(Outcome::Passed) => {
                                passed += 1;
                                ("\u{2713}", "passed")
                            }
                            Some(Outcome::Failed) => {
                                failed += 1;
                                ("\u{2715}", "failed")
                            }
                            Some(Outcome::Skipped) => ("\u{2013}", "skipped"),
                            _ => ("\u{25cb}", "skipped"),
                        };
                        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                        row.set_margin_start(48);
                        row.append(&gtk::Label::builder().label(mark).css_classes([css]).build());
                        row.append(&gtk::Label::builder().label(&t.method).xalign(0.0).hexpand(true).build());
                        if let Some(r) = r {
                            row.append(&gtk::Label::builder().label(format!("{} ms", r.duration_ms.round())).css_classes(["count"]).build());
                        }
                        self.list.append(&row);
                        rows.push(Some((project.clone(), t.clone())));
                    }
                }
            }
        }
        *self.rows.borrow_mut() = rows;
        self.summary.set_label(&if total == 0 { tr("No tests found. Add an MSTest, xUnit or NUnit project.") } else { trf("{} tests · {} passed · {} failed", &[&total.to_string(), &passed.to_string(), &failed.to_string()]) });
    }

    fn all(&self) -> Vec<(String, TestId)> {
        self.tests.borrow().iter().flat_map(|(p, ts)| ts.iter().map(move |t| (p.clone(), t.clone()))).collect()
    }

    pub fn run_all(&self, debug: bool) {
        self.discover();
        let all = self.all();
        if all.is_empty() {
            window::toast(&tr("No tests found."));
            return;
        }
        // Whole projects: no filter.
        run(all, debug, true);
    }

    pub fn run_selected(&self, debug: bool) {
        let sel = self.list.selected_row().and_then(|r| self.rows.borrow().get(r.index() as usize).cloned().flatten());
        if let Some(t) = sel {
            run(vec![t], debug, false);
        }
    }

    pub fn run_failed(&self) {
        let failed: Vec<(String, TestId)> = self.all().into_iter().filter(|(_, t)| self.results.borrow().get(&t.full_name()).is_some_and(|r| r.outcome == Outcome::Failed)).collect();
        if failed.is_empty() {
            window::toast(&tr("No failed tests."));
        } else {
            run(failed, false, false);
        }
    }

    pub fn repeat_last(&self) {
        let last = self.last.borrow().clone();
        if let Some((t, debug)) = last {
            run(t, debug, false);
        }
    }

    /// Ctrl+R, T in a test file: the test (or class) under the caret.
    pub fn run_at_caret(&self, debug: bool) {
        let app = crate::ui::window::app();
        let Some(doc) = app.tabs.current_code() else { return };
        let text = doc.text();
        let byte = doc.cursor_byte();
        let st = anvil_core::csharp::parse(&text);
        let found = testing::discover(&st);
        let in_method: Vec<TestId> = st
            .all_types()
            .iter()
            .flat_map(|(_, t)| t.members.iter().filter(|m| m.range.contains(&byte)).map(|m| m.name.clone()).collect::<Vec<_>>())
            .filter_map(|name| found.iter().find(|(t, _)| t.method == name).map(|(t, _)| t.clone()))
            .collect();
        let chosen = if in_method.is_empty() { found.into_iter().map(|(t, _)| t).collect() } else { in_method };
        let project = app.solution.borrow().as_ref().and_then(|s| s.project_of(&doc.path()).map(|p| p.name.clone())).unwrap_or_default();
        if chosen.is_empty() {
            app.status.flash(&tr("No test here."));
        } else {
            run(chosen.into_iter().map(|t| (project.clone(), t)).collect(), debug, false);
        }
    }
}

fn goto_test(id: &TestId) {
    let app = crate::ui::window::app();
    let Some(sol) = app.solution.borrow().clone() else { return };
    for p in &sol.projects {
        for f in p.source_files() {
            let text = anvil_core::text::TextFile::read(&f).map(|t| t.text).unwrap_or_default();
            for (t, byte) in testing::discover(&anvil_core::csharp::parse(&text)) {
                if &t == id {
                    let line = text[..byte].matches('\n').count() as u32 + 1;
                    app.tabs.goto(&f, line, 1);
                    return;
                }
            }
        }
    }
}

/// Runs tests per project; with `whole` no filter is passed.
fn run(tests: Vec<(String, TestId)>, debug: bool, whole: bool) {
    let app = crate::ui::window::app();
    *app.tests.last.borrow_mut() = Some((tests.clone(), debug));
    app.tabs.save_all();
    let mut by_project: BTreeMap<String, Vec<TestId>> = BTreeMap::new();
    for (p, t) in tests {
        by_project.entry(p).or_default().push(t);
    }
    window::show_bottom("tests");
    app.output.clear("Tests");
    let job = crate::ui::jobs::start("test", &tr("Running tests…"));
    glib::spawn_future_local(async move {
        for (project, ids) in by_project {
            let app = crate::ui::window::app();
            let Some(p) = app.solution.borrow().as_ref().and_then(|s| s.project_named(&project).cloned()) else { continue };
            // In .vs/anvil: the host's dotnet and the sandbox both see it
            // (the sandbox has its own /tmp), and the hand-in leaves it out.
            let results_dir: PathBuf = anvil_core::session::dir(&window::solution_dir().unwrap_or_default()).join("TestResults").join(&project);
            let _ = std::fs::remove_dir_all(&results_dir);
            let mtp = p.csproj.as_ref().is_some_and(testing::uses_testing_platform);
            let filter = if whole { Vec::new() } else { ids };
            let mut cmd = Command::dotnet().args(testing::test_args(&p.path.to_string_lossy(), &filter, &results_dir.to_string_lossy(), mtp)).cwd(&p.dir);
            if debug {
                cmd = cmd.env("VSTEST_HOST_DEBUG", "1");
            }
            app.status.set_message(&trf("Running tests of {}…", &[&project]));
            job.set_text(&trf("Running tests of {}…", &[&project]));
            // Old results of what runs now go, so a failed build never
            // leaves green ticks behind.
            {
                let mut res = app.tests.results.borrow_mut();
                let prefix_ids: Vec<String> = res.keys().filter(|k| whole || filter.iter().any(|f| **k == f.full_name())).cloned().collect();
                for k in prefix_ids {
                    res.remove(&k);
                }
            }
            app.tests.render();
            let mut stream = host::stream(cmd);
            let mut attached = false;
            let mut log = String::new();
            while let Some(line) = stream.lines.recv().await {
                match line {
                    host::Line::Out(l) | host::Line::Err(l) => {
                        app.output.append("Tests", &format!("{l}\n"));
                        log.push_str(&l);
                        log.push('\n');
                        // VSTest prints the testhost's id and waits for a
                        // debugger.
                        if debug
                            && !attached
                            && let Some(pid) = l.split("Process Id:").nth(1).and_then(|r| r.trim().split(|c: char| !c.is_ascii_digit()).next()).and_then(|n| n.parse::<u32>().ok())
                        {
                            attached = true;
                            app.debugger.attach(pid);
                        }
                    }
                    host::Line::Done(_) => {}
                }
            }
            // Read every TRX written.
            let mut results = Vec::new();
            if let Ok(rd) = std::fs::read_dir(&results_dir) {
                for e in rd.flatten() {
                    if e.path().extension().is_some_and(|x| x == "trx")
                        && let Ok(t) = std::fs::read_to_string(e.path())
                    {
                        results.extend(testing::parse_trx(&t));
                    }
                }
            }
            // No results and compiler errors: the test project did not
            // build; the errors go to the Error List as after a build.
            let diags = anvil_core::diagnostics::parse_log(&log);
            if results.is_empty() && anvil_core::diagnostics::counts(&diags).errors > 0 {
                app.errors.set_build(&diags, window::solution_dir().as_deref());
                window::show_bottom("errors");
                app.status.set_message(&trf("The tests of {} did not build.", &[&project]));
            }
            for r in results {
                app.tests.results.borrow_mut().insert(r.id.full_name(), r);
            }
            app.tests.render();
        }
        let app = crate::ui::window::app();
        app.status.set_message(&tr("Test run finished"));
        let elapsed = anvil_core::build::format_elapsed(job.elapsed());
        job.finish();
        let (passed, failed) = {
            let r = app.tests.results.borrow();
            (r.values().filter(|r| r.outcome == Outcome::Passed).count(), r.values().filter(|r| r.outcome == Outcome::Failed).count())
        };
        crate::ui::jobs::result("test", &format!("{} · {elapsed}", trf("Tests: {} passed, {} failed", &[&passed.to_string(), &failed.to_string()])), crate::ui::jobs::Open::Pane("tests"));
    });
}
