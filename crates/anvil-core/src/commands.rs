//! Every command Anvil has, with Visual Studio's command name and its key
//! bindings in VS' default scheme (General / C# profile). Menus, the
//! keymap, the command search and Settings → Keyboard all read this table.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    Global,
    /// VS' "Text Editor" scope.
    Editor,
    /// Solution Explorer.
    Explorer,
    /// Class Designer.
    Designer,
}

#[derive(Debug)]
pub struct CommandDef {
    pub id: &'static str,
    /// VS' canonical command name (`Edit.FormatDocument`), searched in
    /// Settings → Keyboard like VS' Options dialog.
    pub vs_name: &'static str,
    /// English title; German through gettext.
    pub title: &'static str,
    pub keys: &'static [&'static str],
}

macro_rules! cmd {
    ($id:literal, $vs:literal, $title:literal, [$($k:literal),*]) => {
        CommandDef { id: $id, vs_name: $vs, title: $title, keys: &[$($k),*] }
    };
}

pub const COMMANDS: &[CommandDef] = &[
    // File
    cmd!("file.new-project", "File.NewProject", "New Project…", ["Ctrl+Shift+N"]),
    cmd!("file.open-solution", "File.OpenProject", "Open Project or Solution…", ["Ctrl+Shift+O"]),
    cmd!("file.open-folder", "File.OpenFolder", "Open Folder…", ["Ctrl+Shift+Alt+O"]),
    cmd!("file.open-file", "File.OpenFile", "Open File…", ["Ctrl+O"]),
    cmd!("file.save", "File.SaveSelectedItems", "Save", ["Ctrl+S"]),
    cmd!("file.save-all", "File.SaveAll", "Save All", ["Ctrl+Shift+S"]),
    cmd!("file.close-solution", "File.CloseSolution", "Close Solution", []),
    cmd!("file.handin", "File.HandInAsZip", "Hand in as ZIP…", []),
    cmd!("file.exit", "File.Exit", "Exit", ["Alt+F4"]),
    // Edit
    cmd!("edit.undo", "Edit.Undo", "Undo", ["Ctrl+Z", "Alt+Backspace"]),
    cmd!("edit.redo", "Edit.Redo", "Redo", ["Ctrl+Y", "Ctrl+Shift+Z", "Shift+Alt+Backspace"]),
    cmd!("edit.find", "Edit.Find", "Find", ["Ctrl+F"]),
    cmd!("edit.replace", "Edit.Replace", "Replace", ["Ctrl+H"]),
    cmd!("edit.find-in-files", "Edit.FindinFiles", "Find in Files", ["Ctrl+Shift+F"]),
    cmd!("edit.replace-in-files", "Edit.ReplaceinFiles", "Replace in Files", ["Ctrl+Shift+H"]),
    cmd!("edit.find-next", "Edit.FindNext", "Find Next", ["F3"]),
    cmd!("edit.find-previous", "Edit.FindPrevious", "Find Previous", ["Shift+F3"]),
    cmd!("edit.goto-all", "Edit.GoToAll", "Go To All", ["Ctrl+T", "Ctrl+,"]),
    cmd!("edit.goto-line", "Edit.GoTo", "Go To Line", ["Ctrl+G"]),
    cmd!("edit.comment", "Edit.CommentSelection", "Comment Selection", ["Ctrl+K, Ctrl+C"]),
    cmd!("edit.uncomment", "Edit.UncommentSelection", "Uncomment Selection", ["Ctrl+K, Ctrl+U"]),
    cmd!("edit.toggle-comment", "Edit.ToggleLineComment", "Toggle Line Comment", ["Ctrl+K, Ctrl+/"]),
    cmd!("edit.format-document", "Edit.FormatDocument", "Format Document", ["Ctrl+K, Ctrl+D"]),
    cmd!("edit.format-selection", "Edit.FormatSelection", "Format Selection", ["Ctrl+K, Ctrl+F"]),
    cmd!("edit.move-line-up", "Edit.MoveSelectedLinesUp", "Move Selected Lines Up", ["Alt+Up"]),
    cmd!("edit.move-line-down", "Edit.MoveSelectedLinesDown", "Move Selected Lines Down", ["Alt+Down"]),
    cmd!("edit.duplicate", "Edit.Duplicate", "Duplicate", ["Ctrl+D"]),
    cmd!("edit.delete-line", "Edit.LineDelete", "Delete Line", ["Ctrl+Shift+L"]),
    cmd!("edit.cut-line", "Edit.LineCut", "Cut Line", ["Ctrl+L"]),
    cmd!("edit.word-wrap", "Edit.ToggleWordWrap", "Word Wrap", ["Ctrl+E, Ctrl+W"]),
    cmd!("edit.view-whitespace", "Edit.ViewWhiteSpace", "View White Space", ["Ctrl+R, Ctrl+W"]),
    cmd!("edit.uppercase", "Edit.MakeUppercase", "Make Uppercase", ["Ctrl+Shift+U"]),
    cmd!("edit.lowercase", "Edit.MakeLowercase", "Make Lowercase", ["Ctrl+U"]),
    cmd!("edit.goto-brace", "Edit.GotoBrace", "Go To Brace", ["Ctrl+]"]),
    cmd!("edit.complete-word", "Edit.CompleteWord", "Complete Word", ["Ctrl+Space", "Alt+Right"]),
    cmd!("edit.parameter-info", "Edit.ParameterInfo", "Parameter Info", ["Ctrl+Shift+Space"]),
    cmd!("edit.quick-info", "Edit.QuickInfo", "Quick Info", ["Ctrl+K, Ctrl+I"]),
    cmd!("edit.quick-actions", "View.QuickActions", "Quick Actions and Refactorings…", ["Ctrl+.", "Alt+Enter"]),
    cmd!("edit.rename", "Refactor.Rename", "Rename…", ["Ctrl+R, Ctrl+R", "F2"]),
    cmd!("edit.goto-definition", "Edit.GoToDefinition", "Go To Definition", ["F12"]),
    cmd!("edit.peek-definition", "Edit.PeekDefinition", "Peek Definition", ["Alt+F12"]),
    cmd!("edit.goto-implementation", "Edit.GoToImplementation", "Go To Implementation", ["Ctrl+F12"]),
    cmd!("edit.find-references", "Edit.FindAllReferences", "Find All References", ["Shift+F12"]),
    cmd!("edit.insert-snippet", "Edit.InsertSnippet", "Insert Snippet", ["Ctrl+K, Ctrl+X"]),
    cmd!("edit.surround-with", "Edit.SurroundWith", "Surround With", ["Ctrl+K, Ctrl+S"]),
    cmd!("edit.navigate-back", "View.NavigateBackward", "Navigate Backward", ["Ctrl+-"]),
    cmd!("edit.navigate-forward", "View.NavigateForward", "Navigate Forward", ["Ctrl+Shift+-"]),
    cmd!("edit.next-error", "View.NextError", "Next Error", ["Ctrl+Shift+F12"]),
    cmd!("edit.document-outline", "View.DocumentOutline", "Document Outline", ["Ctrl+Alt+T"]),
    // View
    cmd!("view.solution-explorer", "View.SolutionExplorer", "Solution Explorer", ["Ctrl+Alt+L"]),
    cmd!("view.error-list", "View.ErrorList", "Error List", ["Ctrl+\\, Ctrl+E", "Ctrl+\\, E"]),
    cmd!("view.output", "View.Output", "Output", ["Ctrl+Alt+O"]),
    cmd!("view.terminal", "View.Terminal", "Terminal", ["Ctrl+`"]),
    cmd!("view.test-explorer", "TestExplorer.ShowTestExplorer", "Test Explorer", ["Ctrl+E, T"]),
    cmd!("view.git-changes", "View.GitWindow", "Git Changes", ["Ctrl+0, Ctrl+G"]),
    cmd!("view.git-repository", "View.GitRepositoryWindow", "Git Repository", ["Ctrl+0, Ctrl+R"]),
    cmd!("view.chat", "View.CopilotChat", "AI Chat", ["Ctrl+\\, Ctrl+C"]),
    cmd!("view.full-screen", "View.FullScreen", "Full Screen", ["Shift+Alt+Enter"]),
    cmd!("view.toggle-theme", "View.ToggleTheme", "Light or Dark", []),
    cmd!("view.feature-search", "Window.QuickLaunch", "Feature Search", ["Ctrl+Q"]),
    cmd!("view.nuget", "Project.ManageNuGetPackages", "Manage NuGet Packages…", []),
    // Window
    cmd!("window.close-document", "Window.CloseDocumentWindow", "Close", ["Ctrl+F4"]),
    cmd!("window.next-document", "Window.NextDocumentWindow", "Next Document", ["Ctrl+F6", "Ctrl+Alt+PageDown"]),
    cmd!("window.previous-document", "Window.PreviousDocumentWindow", "Previous Document", ["Ctrl+Shift+F6", "Ctrl+Alt+PageUp"]),
    cmd!("window.pin-tab", "Window.PinTab", "Pin Tab", []),
    cmd!("window.split", "Window.Split", "Split", []),
    cmd!("window.move-to-next-group", "Window.MovetoNextTabGroup", "Move to Next Document Group", []),
    cmd!("window.switch-next", "Window.NextDocumentWindowNav", "Switch to Next File", ["Ctrl+Tab"]),
    cmd!("window.switch-previous", "Window.PreviousDocumentWindowNav", "Switch to Previous File", ["Ctrl+Shift+Tab"]),
    cmd!("window.close-tool-window", "Window.CloseToolWindow", "Hide Panel", ["Shift+Escape"]),
    cmd!("window.reset-layout", "Window.ResetWindowLayout", "Reset Window Layout", []),
    // Build
    cmd!("build.solution", "Build.BuildSolution", "Build Solution", ["Ctrl+Shift+B"]),
    cmd!("build.rebuild-solution", "Build.RebuildSolution", "Rebuild Solution", []),
    cmd!("build.clean-solution", "Build.CleanSolution", "Clean Solution", []),
    cmd!("build.project", "Build.BuildSelection", "Build Project", ["Ctrl+B"]),
    cmd!("build.cancel", "Build.Cancel", "Cancel Build", ["Ctrl+Break"]),
    // Debug
    cmd!("debug.start", "Debug.Start", "Start Debugging", ["F5"]),
    cmd!("debug.start-without", "Debug.StartWithoutDebugging", "Start Without Debugging", ["Ctrl+F5"]),
    cmd!("debug.stop", "Debug.StopDebugging", "Stop Debugging", ["Shift+F5"]),
    cmd!("debug.restart", "Debug.Restart", "Restart", ["Ctrl+Shift+F5"]),
    cmd!("debug.break-all", "Debug.BreakAll", "Break All", ["Ctrl+Alt+Break"]),
    cmd!("debug.step-over", "Debug.StepOver", "Step Over", ["F10"]),
    cmd!("debug.step-into", "Debug.StepInto", "Step Into", ["F11"]),
    cmd!("debug.step-out", "Debug.StepOut", "Step Out", ["Shift+F11"]),
    cmd!("debug.run-to-cursor", "Debug.RunToCursor", "Run To Cursor", ["Ctrl+F10"]),
    cmd!("debug.toggle-breakpoint", "Debug.ToggleBreakpoint", "Toggle Breakpoint", ["F9"]),
    cmd!("debug.delete-breakpoints", "Debug.DeleteAllBreakpoints", "Delete All Breakpoints", ["Ctrl+Shift+F9"]),
    cmd!("debug.quick-watch", "Debug.QuickWatch", "QuickWatch…", ["Shift+F9"]),
    cmd!("debug.locals", "Debug.Locals", "Locals", ["Ctrl+Alt+V, L"]),
    cmd!("debug.autos", "Debug.Autos", "Autos", ["Ctrl+Alt+V, A"]),
    cmd!("debug.watch", "Debug.Watch1", "Watch", ["Ctrl+Alt+W, 1"]),
    cmd!("debug.call-stack", "Debug.CallStack", "Call Stack", ["Ctrl+Alt+C"]),
    cmd!("debug.immediate", "Debug.Immediate", "Immediate", ["Ctrl+Alt+I"]),
    cmd!("debug.breakpoints", "Debug.Breakpoints", "Breakpoints", ["Ctrl+Alt+B"]),
    cmd!("debug.exception-settings", "Debug.ExceptionSettings", "Exception Settings", ["Ctrl+Alt+E"]),
    cmd!("debug.hot-reload", "Debug.ApplyCodeChanges", "Hot Reload", ["Alt+F10"]),
    // Test
    cmd!("test.run-all", "TestExplorer.RunAllTests", "Run All Tests", ["Ctrl+R, A"]),
    cmd!("test.debug-all", "TestExplorer.DebugAllTests", "Debug All Tests", ["Ctrl+R, Ctrl+A"]),
    cmd!("test.run-context", "TestExplorer.RunAllTestsInContext", "Run Tests", ["Ctrl+R, T"]),
    cmd!("test.debug-context", "TestExplorer.DebugAllTestsInContext", "Debug Tests", ["Ctrl+R, Ctrl+T"]),
    cmd!("test.repeat-last", "TestExplorer.RepeatLastRun", "Repeat Last Run", ["Ctrl+R, L"]),
    cmd!("test.run-failed", "TestExplorer.RunFailedTests", "Run Failed Tests", ["Ctrl+R, F"]),
    // Project
    cmd!("project.add-new-item", "Project.AddNewItem", "Add New Item…", ["Ctrl+Shift+A"]),
    cmd!("project.add-existing-item", "Project.AddExistingItem", "Add Existing Item…", ["Shift+Alt+A"]),
    cmd!("project.add-class", "Project.AddClass", "Add Class…", ["Shift+Alt+C"]),
    cmd!("project.add-folder", "Project.NewFolder", "New Folder", []),
    cmd!("project.add-class-diagram", "Project.AddClassDiagram", "Class Diagram", []),
    cmd!("project.view-class-diagram", "Project.ViewClassDiagram", "View Class Diagram", []),
    cmd!("project.add-reference", "Project.AddProjectReference", "Add Project Reference…", []),
    cmd!("project.set-startup", "Project.SetasStartupProject", "Set as Startup Project", []),
    cmd!("project.properties", "Project.Properties", "Properties", ["Alt+Enter"]),
    cmd!("project.new-project-in-solution", "File.AddNewProject", "Add New Project…", []),
    // Solution Explorer (scope Explorer)
    cmd!("explorer.rename", "SolutionExplorer.Rename", "Rename", ["F2"]),
    cmd!("explorer.delete", "SolutionExplorer.Delete", "Delete", ["Delete"]),
    cmd!("explorer.exclude", "Project.ExcludeFromProject", "Exclude From Project", []),
    cmd!("explorer.open-folder", "File.OpenContainingFolder", "Open Folder in File Manager", []),
    cmd!("explorer.open-terminal", "View.OpenInTerminal", "Open in Terminal", []),
    // Class Designer (scope Designer)
    cmd!("designer.delete", "ClassDiagram.RemoveFromDiagram", "Remove from Diagram", ["Delete"]),
    cmd!("designer.delete-code", "ClassDiagram.DeleteCode", "Delete Code", ["Ctrl+Delete"]),
    cmd!("designer.layout", "ClassDiagram.LayoutDiagram", "Layout Diagram", []),
    cmd!("designer.zoom-in", "ClassDiagram.ZoomIn", "Zoom In", ["Ctrl+Shift+."]),
    cmd!("designer.zoom-out", "ClassDiagram.ZoomOut", "Zoom Out", ["Ctrl+Shift+,"]),
    cmd!("designer.export", "ClassDiagram.ExportAsImage", "Export Diagram as Image…", []),
    // Git
    cmd!("git.commit", "Team.Git.CommitAll", "Commit All", []),
    cmd!("git.fetch", "Team.Git.Fetch", "Fetch", []),
    cmd!("git.pull", "Team.Git.Pull", "Pull", []),
    cmd!("git.push", "Team.Git.Push", "Push", []),
    cmd!("git.init", "Team.Git.CreateRepository", "Create Git Repository…", []),
    // Tools
    cmd!("tools.options", "Tools.Options", "Settings", []),
    cmd!("tools.ai-off", "Tools.ToggleAI", "AI Off", []),
    cmd!("tools.restart-roslyn", "Tools.RestartLanguageServer", "Restart Language Server", []),
    cmd!("help.shortcuts", "Help.KeyboardShortcuts", "Keyboard Shortcuts", ["F1"]),
];

/// Edit commands that do not need a caret work from any panel, as in VS
/// (Ctrl+T from the Solution Explorer, Ctrl+Shift+F from the Error List).
const GLOBAL_EDIT: [&str; 6] = ["edit.find-in-files", "edit.replace-in-files", "edit.goto-all", "edit.navigate-back", "edit.navigate-forward", "edit.next-error"];

pub fn scope_of(id: &str) -> Scope {
    if GLOBAL_EDIT.contains(&id) {
        return Scope::Global;
    }
    match id.split('.').next() {
        Some("edit") => Scope::Editor,
        Some("explorer") => Scope::Explorer,
        Some("designer") => Scope::Designer,
        _ => Scope::Global,
    }
}

pub fn find(id: &str) -> Option<&'static CommandDef> {
    COMMANDS.iter().find(|c| c.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_independent_edit_commands_are_global() {
        assert_eq!(scope_of("edit.goto-all"), Scope::Global);
        assert_eq!(scope_of("edit.find-in-files"), Scope::Global);
        assert_eq!(scope_of("edit.comment"), Scope::Editor);
        assert_eq!(scope_of("designer.delete"), Scope::Designer);
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<&str> = COMMANDS.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }
}
