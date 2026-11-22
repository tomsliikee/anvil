//! What each command does. Menus, toolbar, keymap and context menus all
//! end here.

use adw::prelude::*;
use gtk::{gio, glib};
use sourceview5::prelude::*;

use crate::i18n::tr;
use crate::ui::editor::Doc;
use crate::ui::window::{self};
use crate::ui::{build, designer, dialogs, editor, explorer, find, lsp, newitem, nuget, properties, settings};

fn code() -> Option<std::rc::Rc<editor::CodeDoc>> {
    crate::ui::window::app().tabs.current_code()
}

fn with_code(f: impl FnOnce(&editor::CodeDoc)) {
    if let Some(d) = code() {
        f(&d);
    }
}

/// The text view under the keyboard focus, if any (undo in the chat entry
/// or a dialog belongs to it).
fn focused_text_view() -> Option<gtk::TextView> {
    let app = crate::ui::window::app();
    let mut w = GtkWindowExt::focus(&app.window);
    while let Some(x) = w {
        if let Ok(tv) = x.clone().downcast::<gtk::TextView>() {
            return Some(tv);
        }
        w = x.parent();
    }
    None
}

pub fn dispatch(id: &str) {
    let app = crate::ui::window::app();
    match id {
        // File
        "file.new-project" => dialogs::new_project(None),
        "file.open-solution" => dialogs::open_solution(),
        "file.open-folder" => dialogs::open_folder(),
        "file.open-file" => dialogs::open_file(),
        "file.save" => app.tabs.save_current(),
        "file.save-all" => {
            app.tabs.save_all();
        }
        "file.close-solution" => window::close_solution(),
        "file.handin" => build::hand_in(),
        "file.exit" => app.window.close(),

        // Edit
        "edit.undo" | "edit.redo" => {
            let redo = id == "edit.redo";
            if let Some(Doc::Diagram(d)) = app.tabs.current().as_deref() {
                d.undo(redo);
            } else if let Some(tv) = focused_text_view() {
                let _ = tv.activate_action(if redo { "text.redo" } else { "text.undo" }, None);
            } else if let Some(d) = app.tabs.current_code() {
                // From the menu or a panel: the editor in front.
                if redo {
                    d.buffer.redo();
                } else {
                    d.buffer.undo();
                }
            }
        }
        "edit.find" | "edit.replace" => with_code(|d| d.find.show(id == "edit.replace", &d.selection_text())),
        "edit.find-in-files" => find::find_in_files_dialog(false),
        "edit.replace-in-files" => find::find_in_files_dialog(true),
        "edit.find-next" | "edit.find-previous" => with_code(|d| {
            d.find.entry.emit_by_name::<()>(if id == "edit.find-next" { "next-match" } else { "previous-match" }, &[]);
        }),
        "edit.goto-all" => dialogs::goto_all(),
        "edit.goto-line" => editor::goto_line_dialog(),
        "edit.comment" => with_code(|d| d.comment(Some(true))),
        "edit.uncomment" => with_code(|d| d.comment(Some(false))),
        "edit.toggle-comment" => with_code(|d| d.comment(None)),
        "edit.format-document" => lsp::format(false),
        "edit.format-selection" => lsp::format(true),
        "edit.move-line-up" => with_code(|d| d.view.emit_move_lines(false)),
        "edit.move-line-down" => with_code(|d| d.view.emit_move_lines(true)),
        "edit.duplicate" => with_code(|d| d.duplicate()),
        "edit.delete-line" => with_code(|d| d.delete_line(false)),
        "edit.cut-line" => with_code(|d| d.delete_line(true)),
        "edit.word-wrap" => {
            let on = !app.settings.boolean("word-wrap");
            let _ = app.settings.set_boolean("word-wrap", on);
            for d in app.tabs.code_docs() {
                d.view.set_wrap_mode(if on { gtk::WrapMode::WordChar } else { gtk::WrapMode::None });
            }
        }
        "edit.view-whitespace" => {
            let on = !app.settings.boolean("show-whitespace");
            let _ = app.settings.set_boolean("show-whitespace", on);
            for d in app.tabs.code_docs() {
                editor::set_whitespace(&d.view, on);
            }
        }
        "edit.uppercase" => with_code(|d| d.change_case(true)),
        "edit.lowercase" => with_code(|d| d.change_case(false)),
        "edit.goto-brace" => with_code(|d| d.goto_brace()),
        "edit.complete-word" => with_code(|d| d.view.emit_show_completion()),
        "edit.parameter-info" => lsp::signature_help(),
        "edit.quick-info" => lsp::quick_info(),
        "edit.quick-actions" => lsp::quick_actions(),
        "edit.rename" => {
            if let Some(Doc::Diagram(d)) = app.tabs.current().as_deref() {
                d.rename_selected();
            } else {
                lsp::rename();
            }
        }
        "edit.goto-definition" => lsp::goto("textDocument/definition"),
        "edit.peek-definition" => lsp::peek_definition(),
        "edit.goto-implementation" => lsp::goto("textDocument/implementation"),
        "edit.find-references" => lsp::find_references(),
        "edit.insert-snippet" => dialogs::insert_snippet(false),
        "edit.surround-with" => dialogs::insert_snippet(true),
        "edit.navigate-back" => app.tabs.navigate(true),
        "edit.navigate-forward" => app.tabs.navigate(false),
        "edit.next-error" => app.errors.next(),
        "edit.document-outline" => lsp::document_outline(),

        // View
        "view.solution-explorer" => window::show_side("explorer"),
        "view.git-changes" | "view.git-repository" => {
            window::show_side("git");
            app.git.refresh();
            if id == "view.git-repository" {
                app.git.show_history();
            }
        }
        "view.chat" => {
            if app.settings.boolean("ai-enabled") && app.settings.boolean("ai-chat") {
                window::show_side("chat");
                app.chat.focus();
            } else {
                window::toast(&tr("The AI chat is switched off (Tools → Settings → AI)."));
            }
        }
        "view.error-list" => window::show_bottom("errors"),
        "view.output" => window::show_bottom("output"),
        "view.terminal" => {
            window::show_bottom("terminal");
            app.terminal.ensure_shell();
        }
        "view.test-explorer" => window::show_bottom("tests"),
        "view.full-screen" => {
            if app.window.is_fullscreen() {
                app.window.unfullscreen();
            } else {
                app.window.fullscreen();
            }
        }
        "view.toggle-theme" => {
            let _ = app.settings.set_string(crate::theme::KEY, crate::theme::flipped());
            crate::theme::apply(&app.settings);
        }
        "view.feature-search" => crate::ui::palette_search::open(),
        "view.nuget" => nuget::open(explorer::selected_project()),

        // Window
        "window.close-document" => app.tabs.close_current(),
        "window.next-document" | "window.previous-document" => {
            let n = app.tabs.view.n_pages();
            if n > 0
                && let Some(p) = app.tabs.view.selected_page()
            {
                let i = app.tabs.view.page_position(&p);
                let next = if id == "window.next-document" { (i + 1) % n } else { (i + n - 1) % n };
                app.tabs.view.set_selected_page(&app.tabs.view.nth_page(next));
            }
        }
        "window.pin-tab" => {
            if let Some(p) = app.tabs.view.selected_page() {
                app.tabs.view.set_page_pinned(&p, !p.is_pinned());
            }
        }
        "window.split" => window::toast(&tr("Split view: drag a tab out to a second window.")),

        // Build
        "build.solution" => build::build(build::Target::Solution, build::Verb::Build),
        "build.rebuild-solution" => build::build(build::Target::Solution, build::Verb::Rebuild),
        "build.clean-solution" => build::build(build::Target::Solution, build::Verb::Clean),
        "build.project" => match explorer::selected_project().or_else(build::current_project) {
            Some(p) => build::build(build::Target::Project(p), build::Verb::Build),
            None => build::build(build::Target::Solution, build::Verb::Build),
        },
        "build.cancel" => app.builder.cancel(),

        // Debug
        "debug.start" => app.debugger.start_or_continue(),
        "debug.start-without" => build::run_without_debugging(),
        "debug.stop" => app.debugger.stop(),
        "debug.restart" => app.debugger.restart(),
        "debug.break-all" => app.debugger.pause(),
        "debug.step-over" => app.debugger.step("next"),
        "debug.step-into" => app.debugger.step("stepIn"),
        "debug.step-out" => app.debugger.step("stepOut"),
        "debug.run-to-cursor" => app.debugger.run_to_cursor(),
        "debug.toggle-breakpoint" => with_code(|d| {
            let line = d.buffer.iter_at_mark(&d.buffer.get_insert()).line() as u32 + 1;
            app.debugger.toggle_breakpoint(&d.path(), line);
        }),
        "debug.delete-breakpoints" => app.debugger.clear_breakpoints(),
        "debug.quick-watch" => app.debugger.quick_watch(),
        "debug.locals" | "debug.autos" | "debug.watch" | "debug.call-stack" | "debug.immediate" | "debug.breakpoints" => {
            app.debugger.show_window(id.trim_start_matches("debug."));
        }
        "debug.exception-settings" => app.debugger.exception_settings(),
        "debug.hot-reload" => build::hot_reload(),

        // Test
        "test.run-all" => app.tests.run_all(false),
        "test.debug-all" => app.tests.run_all(true),
        "test.run-context" | "test.debug-context" => app.tests.run_at_caret(id == "test.debug-context"),
        "test.repeat-last" => app.tests.repeat_last(),
        "test.run-failed" => app.tests.run_failed(),

        // Project
        "project.add-new-item" => newitem::show(None),
        "project.add-existing-item" => dialogs::add_existing_item(),
        "project.add-class" => newitem::show(Some(anvil_core::templates::ItemTemplate::Class)),
        "project.add-folder" => explorer::new_folder(),
        "project.add-class-diagram" => designer::add_class_diagram(),
        "project.view-class-diagram" => designer::view_class_diagram(),
        "project.add-reference" => dialogs::add_project_reference(),
        "project.set-startup" => {
            if let Some(p) = explorer::selected_project() {
                window::set_startup_project(&p);
                window::refresh_startup_dropdown();
            }
        }
        "project.properties" => properties::show(explorer::selected_project().or_else(build::current_project)),
        "project.new-project-in-solution" => dialogs::new_project(app.solution.borrow().as_ref().map(|s| s.path.clone())),

        // Solution Explorer
        "explorer.rename" => explorer::rename_selected(),
        "explorer.delete" => explorer::delete_selected(),
        "explorer.exclude" => explorer::exclude_selected(),
        "explorer.open-folder" => explorer::open_folder_of_selected(),
        "explorer.open-terminal" => explorer::open_terminal_at_selected(),

        // Class Designer
        "designer.delete" | "designer.delete-code" | "designer.layout" | "designer.zoom-in" | "designer.zoom-out" | "designer.export" => {
            if let Some(Doc::Diagram(d)) = app.tabs.current().as_deref() {
                d.command(id);
            }
        }

        // Git
        "git.commit" => {
            window::show_side("git");
            app.git.focus_message();
        }
        "git.fetch" => app.git.remote("fetch"),
        "git.pull" => app.git.remote("pull"),
        "git.push" => app.git.remote("push"),
        "git.init" => app.git.init(),

        // Tools
        "tools.options" => settings::show(None),
        "tools.restart-roslyn" => app.lsp.restart(),
        "tools.ai-off" => {
            let _ = app.settings.set_boolean("ai-enabled", false);
            window::toast(&tr("All AI features are off."));
        }
        "help.shortcuts" => settings::show(Some("keyboard")),
        other => app.status.flash(&format!("{other}: {}", tr("not available here"))),
    }
    let _ = (gio::Cancellable::NONE, glib::Propagation::Proceed);
}
