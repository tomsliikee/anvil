# Anvil: Design

> 2026-11-18 · Status: draft for approval · Author: Thomas Haiden with Claude

**Anvil** is a C# IDE for Linux, written in Rust with GTK 4 and libadwaita, in the **Insights Mono** design. It opens Visual Studio solutions, edits, builds, runs, tests and debugs them, and saves everything so that Visual Studio 2026 on Windows opens and runs it unchanged. Its own strength is the **class diagram**: drawn on a canvas, wired both ways to the code, stored as Visual Studio `.cd` files.

---

## 1. Intent

Thomas' university allows only Visual Studio. He wants to do the assignments on Linux. What he hands in (as a ZIP) must open and run in the lecturer's Visual Studio with no rework and show the same **StyleCop** warnings.

There is no deadline: Anvil is done when it is done, and is planned for the full scope. No Windows PC is available for testing, so compatibility is proven against real files from Visual Studio (section 9).

### What a course assignment looks like (*tested* on `doublyLinkedList`, a real assignment, 2026-11-18)

| Item | Found |
| :--- | :--- |
| Visual Studio | **2026** (`.vs/…/v18`, Copilot index `18.11`) |
| Solution | **`.slnx`**, CRLF, no BOM, one project |
| Project | SDK-style, `net10.0`, `OutputType Exe`, `ImplicitUsings` and `Nullable` enabled |
| StyleCop | **NuGet `StyleCop.Analyzers` 1.1.118**, no `stylecop.json`, no `.editorconfig`, no ruleset: default rules |
| Source files | **UTF-8 with BOM, CRLF**, block namespaces, `internal class`, `this.` prefix |
| Class diagram | `ClassDiagram1.cd`, `MembersFormat="FullSignature"`, only `Position` and `TypeIdentifier` per type, `Font Segoe UI 9` |
| Hand-in | ZIP of the whole folder, including `bin/`, `obj/`, `.vs/` |

`dotnet build` of that assignment on this PC (SDK 10.0.113) reports the StyleCop warnings (SA1633, SA1300, SA1200, SA1508, SA1401, SA1307, SA1202, …), so the analyzer path works on Linux unchanged (*tested*).

### Success

An assignment like the one above is created, written, tested, debugged and diagrammed in Anvil alone, then handed in with **Hand in as ZIP**; Visual Studio 2026 opens the solution and the diagram, builds with the same warnings, runs it, and does not ask to save anything.

### Goals

- Open, edit, build, run, test and debug .NET 10+ console solutions (`.slnx` and `.sln`).
- Behave like Visual Studio wherever muscle memory counts: the complete VS default keyboard scheme (C# profile, including chords), the VS context menus, the VS words (German and English UI). Look better: Insights Mono throughout.
- Write files exactly as Visual Studio would: same templates, same encoding, same line endings.
- The class diagram as in section 5, round-tripping with Visual Studio's Class Designer.
- StyleCop warnings and fixes exactly as in Visual Studio.
- Git, Test Explorer, NuGet Package Manager, Hot Reload.
- Local AI (Ollama only): inline completion accepted with Tab, and a chat assistant that proposes edits shown as a diff; both off-switchable.
- Fast: an open solution is usable in under 1 s before the language server is ready; typing never waits on the language server, a build, the diagram or the AI.

### Non-goals

- WinForms, WPF, MAUI, ASP.NET designers; C++ (`.vcxproj`); .NET Framework 4.x; Azure and cloud tooling; Live Share; profiler.
- Visual Studio extensions (VSIX).
- Cloud AI (no Claude, no OpenAI): nothing leaves the machine.
- Other languages than C#.

### Anti-goals (failure even if it works)

- **A file Visual Studio cannot open, or opens with changes it wants to save.** Anvil never rewrites a file it did not need to change and changes only the part that changed; encoding (BOM) and line endings of every file are kept.
- **Warnings that differ from Visual Studio's.**
- **Generated code that differs from what Visual Studio would generate** for the same action.
- **The look of a generic IDE skin**: coloured icons everywhere, status dots, glows, heavy outlines.
- **Typing that stutters.**

---

## 2. Approaches considered

| Approach | Gives | Costs | Decision |
| :--- | :--- | :--- | :--- |
| **A. Fork Zed** (Rust, GPUI) | Editor, LSP, debugger, terminal already there; very fast | Insights Mono's shapes need rewrites deep in Zed's UI; class diagram from scratch in GPUI; upstream merges forever; Zed plugins cannot draw panels | Rejected |
| **B. New IDE from scratch with GPUI or iced** | Full control | Months of text-editor work before any VS feature | Rejected |
| **C. Rust core + Tauri + Monaco** | Fastest to show | WebKitGTK is noticeably slower on Linux | Rejected |
| **D. New app on Atlas' stack** (Rust, GTK 4, libadwaita, GtkSourceView 5) | Insights Mono pipeline, fonts, springs, diagram canvas (cards, edges, layout, zoom, drag, undo, export) and SourceView setup exist in Atlas | Editor conveniences (hover, peek, rename UI, debugger UI, terminal panel) are ours to build | **Chosen by Thomas**, 2026-11-18 |

Why D: Thomas' two priorities are the Mono design and the class diagram; both are half-built in Atlas. The language intelligence comes from Microsoft's Roslyn server in every approach, so D builds only the display, never the analysis.

Anvil is a **new repository** (`~/git/anvil`), not a fork of Atlas. The needed Atlas parts are **copied** (Thomas' own MIT code); a shared crate is considered only once it is clear what both share.

---

## 3. Architecture

```
┌──────────────────────────── anvil (GTK 4 app) ─────────────────────────────┐
│ window · solution explorer · editor tabs · class designer · panels         │
│ (error list, output, terminal, test explorer, git, debugger, AI chat)      │
└──────┬──────────────┬──────────────┬──────────────┬──────────────┬─────────┘
       │ async channels (Tokio)      │              │              │
┌──────▼──────┐ ┌─────▼──────┐ ┌─────▼──────┐ ┌─────▼──────┐ ┌─────▼──────┐
│ anvil-core  │ │ anvil-lsp  │ │ anvil-dap  │ │ anvil-ai   │ │ anvil-git  │
│ (no GTK)    │ │ Roslyn     │ │ netcoredbg │ │ Ollama     │ │ git on the │
│ sln/slnx/   │ │ client     │ │ client     │ │ HTTP       │ │ host       │
│ csproj, .cd,│ └─────┬──────┘ └─────┬──────┘ └─────┬──────┘ └────────────┘
│ C# structure│       │ stdio        │ stdio        │ localhost:11434
│ edit planner│  Roslyn LS      netcoredbg       Ollama
│ templates,  │  (host)         (host)           (host)
│ keymap,     │
│ tokens      │   build / run / test / watch / new / package: dotnet CLI (host)
└─────────────┘
```

### Crates

| Crate | Holds | GTK |
| :--- | :--- | :--- |
| **`anvil-core`** | solution/project model; lossless `.sln`/`.slnx`/`.csproj`/`.cd` editing; C# structure model (`tree-sitter-c-sharp`); edit planner (diagram actions → text edits); VS item templates; keymap model (VS scheme, chords, user overrides); `.cd` HashCode; hand-in ZIP builder; Insights Mono tokens | no; tested on the host |
| **`anvil-lsp`** | LSP client over stdio (`lsp-types`), Roslyn's custom notifications (`solution/open`, `project/open`, `workspace/projectInitializationComplete`) | no |
| **`anvil-dap`** | DAP client for `netcoredbg` | no |
| **`anvil-ai`** | Ollama client: fill-in-the-middle completion and chat; context building; edit proposals as patches | no |
| **`anvil-git`** | status, diff, stage, commit, branches, pull/push through the host's `git` | no |
| **`anvil`** | the app | yes; built in the GNOME SDK like Atlas |

### Threads

The GTK main thread only draws and handles input. LSP, DAP, AI, git, file watching and `dotnet` processes run on a Tokio runtime and talk to the UI through channels (Atlas' `runtime.rs` pattern). Every reply carries the document version it answers; stale replies are dropped. Inline AI completion is cancelled on every keystroke.

### Packaging and the host

Built and shipped as a **Flatpak** on GNOME 51, like Atlas, for Thomas' own machines first (Flathub is a later question). The .NET SDK, Roslyn server, `netcoredbg`, `git`, Ollama and the user's program run **on the host** through `flatpak-spawn --host` (`--talk-name=org.freedesktop.Flatpak`, as GNOME Builder does), so the user's SDK, NuGet cache and git config are the ones the command line uses. The manifest grants `--filesystem=home` so host processes and the app see the same paths.

- **Roslyn server:** `Microsoft.CodeAnalysis.LanguageServer.linux-x64` (MIT), downloaded on first start with consent into `~/.local/share/anvil/roslyn/<version>/`, version pinned per release. Published on Microsoft's Azure DevOps feed (`vs-impl`), not nuget.org (*to verify*). Started with `--stdio`. May need a matching .NET runtime on the host.
- **netcoredbg** (Samsung, MIT): release binary into `~/.local/share/anvil/netcoredbg/<version>/`. Microsoft's `vsdbg` is licensed for Microsoft products only.
- **.NET SDK 10.x** expected on the host; Anvil says so plainly if missing and does not install it.
- **Ollama** expected at `http://localhost:11434` when AI is switched on; Anvil lists the installed models and does not pull any without asking.
- **VTE** is not in the GNOME 51 runtime (*tested*), so the manifest builds it as a module.

---

## 4. Visual Studio features

Words follow Visual Studio in both languages (German: *Projektmappen-Explorer*, *Fehlerliste*, *Ausgabe*; English: *Solution Explorer*, *Error List*, *Output*).

### Solutions and projects

- Open `.slnx` and `.sln`; recent solutions on the start page; open a folder with one solution.
- **Solution Explorer** with the VS context menus: *Add → New Item… (`Ctrl+Shift+A`), Existing Item… (`Shift+Alt+A`), Class… (`Shift+Alt+C`), New Folder, Class Diagram*; *Rename (`F2`), Delete, Exclude*, *Set as Startup Project*, *Manage NuGet Packages…*, *Properties (`Alt+Enter`)*, *Open Folder in File Manager*, *Open in Terminal*. Dependencies node (packages, project references, frameworks).
- **New Item dialog** with VS 2026's C# item templates: Class, Interface, Enum, Struct, Record, Class Diagram, Unit Test class, JSON/Text file, `.editorconfig`.
- New solution and new project (Console App, Class Library, MSTest/xUnit/NUnit test project) through `dotnet new`. SDK 10 writes `.slnx` by default (*tested*); `--format sln` is a setting.
- **How files are changed:**
  - Solution and project membership go through the `dotnet` CLI (`dotnet sln add/remove`, `dotnet add reference`, `dotnet add package`). The SDK writes what VS expects. Newer SDKs rewrite the whole solution file with the same writer VS uses; a test checks the result against VS-written `.slnx` samples.
  - SDK-style projects include `**/*.cs` by default; adding a class is creating the file.
  - Direct `.csproj` edits are lossless: original text kept, only the changed element replaced, same BOM and line endings.
- **New files are written as Visual Studio writes them** (Thomas' choice): the VS 2026 item template text (block namespace from the folder path, `internal class Name`, empty body as VS leaves it), **UTF-8 with BOM, CRLF**. Not StyleCop-cleaned; the warnings are the same as in VS.
- Project properties page: target framework, output type, nullable, implicit usings, root namespace, assembly name, `TreatWarningsAsErrors`, warnings to suppress; anything else read-only.

### Editor

- Tabs (preview tab, pinned tabs), split view, find and replace in file and in files (`Ctrl+Shift+F`), go to all (`Ctrl+T`), go to line (`Ctrl+G`), bracket matching, auto-indent, comment/uncomment (`Ctrl+K, Ctrl+C` / `Ctrl+K, Ctrl+U`), line move (`Alt+Up/Down`), duplicate (`Ctrl+D`), multi-caret where GtkSourceView allows it, word wrap, whitespace display.
- From Roslyn: IntelliSense with signature help, squiggles, Quick Info, go to definition (`F12`), peek definition (`Alt+F12`), go to implementation (`Ctrl+F12`), find all references (`Shift+F12`), rename (`Ctrl+R, Ctrl+R`), quick actions (`Ctrl+.`) including StyleCop's code fixes, format document (`Ctrl+K, Ctrl+D`), semantic highlighting, document outline, CodeLens-style reference counts (off by default).
- Context menu in the editor as in VS: *Quick Actions and Refactorings, Rename, Go To Definition, Peek Definition, Find All References, Run/Debug Tests* (in test files), *Insert Snippet, Surround With*, *Cut/Copy/Paste*.
- Snippets: VS' C# snippets (`prop`, `ctor`, `for`, `foreach`, `cw`, `try`, `if`, `svm`, …) with Tab expansion.
- Highlighting: Cascadia Mono (VS' code font) with a muted palette; type and method names in its cursive italic (changed 2026-11-19 after Thomas' first use; first Monaspace, earlier ink weights in Geist Mono).
- Each file keeps its encoding and line endings; new lines typed into a CRLF file are CRLF.

### Keyboard

- The complete **Visual Studio default scheme for C#**, including two-key chords (`Ctrl+K, Ctrl+D`), shown in menus and tooltips.
- Every command can be rebound in Settings → Keyboard (search by command or key, conflicts shown); overrides stored per user, the VS scheme stays the base.
- GTK/GNOME shortcuts that clash (e.g. `Ctrl+.` emoji input on some setups) are taken over inside Anvil.

### Build, run, debug

- **Build Solution** (`Ctrl+Shift+B`), **Rebuild**, **Clean**, build one project: `dotnet build` with file/line/code output; **Output** panel; **Error List** merges build and live Roslyn diagnostics (Errors / Warnings / Messages, Build + IntelliSense, filter by code so StyleCop's `SA` rules can be shown or hidden).
- **Start Without Debugging** (`Ctrl+F5`): runs the startup project in the **terminal panel** (VTE) so `Console.ReadLine` works; "Press any key to close" like VS.
- **Start Debugging** (`F5`): `Console.ReadLine` works here too. netcoredbg does not support DAP `runInTerminal`, so Anvil starts the program itself in the terminal panel, held at startup (`DOTNET_DefaultDiagnosticPortSuspend` or an equivalent, *to verify*), and attaches netcoredbg by process id so breakpoints in `Main` still hit. Breakpoints (`F9`, conditions, hit counts), step over/into/out (`F10`, `F11`, `Shift+F11`), run to cursor (`Ctrl+F10`), continue, stop (`Shift+F5`), restart (`Ctrl+Shift+F5`), exception settings, Locals, Autos, Watch, Call Stack, Immediate (evaluate), hover values, current-line marker.
- **Hot Reload** (without debugger, Thomas' choice): a toolbar button starts the program with `dotnet watch`; saving applies the change. Not available while stopped at a breakpoint (netcoredbg has no Edit and Continue).
- Command-line arguments in `Properties/launchSettings.json` (the file VS reads). Startup project kept in `.vs/anvil/`; VS falls back to the first project in the solution, so Anvil warns when they differ and offers to move it first.

### Test Explorer

- Discovers MSTest, xUnit and NUnit tests through `dotnet test` (list, run, filter), shown as a tree (project → namespace → class → test) with pass/fail/skip, duration, message and stack trace; run all (`Ctrl+R, A`), run selected, debug a test (netcoredbg attach, as for `F5`), rerun failed. *Builder chooses* between VSTest (`--list-tests`, TRX results) and Microsoft.Testing.Platform, whichever the project uses.

### Git

- The VS *Git Changes* and *Git Repository* windows in reduced form: status, diff (side by side in Mono), stage/unstage, commit, amend, branches (create, switch, merge), fetch/pull/push, history of a file, blame in the editor gutter (off by default), *Create Git Repository* with VS' `.gitignore`. Through the host's `git` so credentials and config are the user's own.

### NuGet Package Manager

- Browse (nuget.org search API v3), Installed, Updates; version picker; install, update, uninstall through `dotnet add/remove package`; package sources from the user's `NuGet.Config`.

### Hand in as ZIP

- *File → Hand in as ZIP…* (Thomas' choice): writes `<Solution>.zip` of the solution folder **without `bin/`, `obj/`, `.vs/`** and without anything `.gitignore` would exclude; Visual Studio restores them on open. Before writing, it runs a build and shows errors and warning counts so nothing broken goes out by accident; the user can hand in anyway.

---

## 5. Class diagram (Class Designer)

### What the user does

| Action | In the diagram | In the code |
| :--- | :--- | :--- |
| **Add → Class Diagram** (project menu) | new empty canvas | new `ClassDiagram1.cd` (next free number) |
| **View Class Diagram** (project menu) | all types of the project, laid out | new `.cd` |
| Drag a `.cs` file or type from Solution Explorer | its types appear as cards | nothing |
| Canvas context menu **Add → Class / Enum / Interface / Abstract Class / Struct / Record / Delegate / Comment**, or the Class Designer toolbox | new card, name edited in place | new `<Name>.cs` like VS' Class Designer writes it (VS asks for the file name; Anvil too, default `<Name>.cs`) |
| Card context menu **Add → Method / Property / Field / Event / Constructor / Destructor / Constant** | new row, name in edit | member inserted as VS' Class Designer does it (methods with `throw new NotImplementedException();`), after the last member of its kind |
| **Class Details** panel (VS' grid: name, type, modifier, summary; parameters) | edits rows | declaration rewritten in place |
| Rename on the card | renamed | Roslyn rename (file renamed when it matches) |
| Delete | removed | asks: *Remove from Diagram* or *Delete Code*, as VS |
| **Inheritance** tool from A to B | edge | `: B` in A's base list; removing the edge removes it |
| **Implement Interface** | lollipop/edge | interface added, members stubbed through Roslyn's "Implement interface" |
| **Association** tool / *Show as Association*, *Show as Collection Association* | arrow | nothing (diagram only, as VS) |
| *Show Base Class*, *Show Derived Classes*, *Collapse/Expand*, *Group Members* (by kind / access / alphabetical), *Change Members Format* (name only / name and type / full signature), *Layout Diagram*, *Adjust Shapes Width*, *Zoom*, *Export Diagram as Image* | as VS | nothing |
| Double-click a member or card | | editor opens at the declaration |
| Edit the code | cards follow within one parse (tree-sitter, debounced 150 ms) | |

Card content in Insights Mono: type name in Inter 600, kind as a mono chip (`class`, `interface`, `abstract class`, `enum`), members in Cascadia Mono with VS' member format (the diagram's `MembersFormat`), compartments Fields / Properties / Methods / Events / Nested Types that fold like VS. Zoom, pan, minimap, export PNG/SVG/PDF from Atlas.

### Code → diagram

`tree-sitter-c-sharp` parses every `.cs` of the project into a **structure model** (types, members with signatures, base lists, partial parts, nesting, file and range of each), fast and tolerant of broken code. Roslyn resolves what needs semantics: which `B` in `: B` is meant, rename, implement-interface.

### Diagram → code (the edit planner)

A diagram action becomes a **text edit plan** in `anvil-core` (file, byte range, text), built from the structure model and from **the text Visual Studio's Class Designer generates for the same action** (gathered as golden samples before the planner is written, section 9). It is applied as one undo step to the open buffer or the file, keeping the file's BOM and line endings. Roslyn's `rangeFormatting` then applies `.editorconfig` indentation; StyleCop layout is not enforced, because VS does not enforce it either (Thomas' choice). Undo in the diagram and in the editor share one history entry per action.

### The `.cd` file

Visual Studio's Class Designer format: `<ClassDiagram MajorVersion="1" MinorVersion="1" MembersFormat="…">` with `<Class>`, `<Interface>`, `<Enum>`, `<Struct>`, `<Delegate>`, `<Comment>`, `<Position X Y Width/>` in inches, `<Compartments>`, `<ShowAsAssociation>`, `<ShowAsCollectionAssociation>`, `<AssociationLine>`, `<InheritanceLine>`, `<Lollipop>`, `<TypeIdentifier><HashCode/><FileName/></TypeIdentifier>`, `<Font>`.

- Read losslessly; unknown elements and attributes kept in place; writing an unchanged diagram gives identical bytes (UTF-8 with BOM, CRLF, as VS writes it).
- Positions convert between inches and pixels at 96 dpi.
- **`HashCode`** (32 bytes, base64) is VS' fingerprint of a type's members. Anvil must compute it exactly, so that VS does not rewrite the file and ask to save. The algorithm is reconstructed from samples and checked against them: the course assignment already gives five (`Person`, `Program`, `DoublyLinkedPersonList` (empty), `ListNode`, `Sex`), and more are gathered from public `.cd` files with their sources (section 9).
- Anvil-only view state (zoom, scroll) goes to `.vs/anvil/diagrams/<name>.json`, never into the project folder, because SDK-style projects would list any extra file in VS.

---

## 6. StyleCop

The course uses **StyleCop.Analyzers 1.1.118** as a NuGet package with default rules (*tested* on the assignment). This needs no special code: `dotnet build` reports its warnings and the Roslyn server loads the same analyzers and code fixes, so squiggles, the Error List and `Ctrl+.` fixes are identical to VS. Anvil adds:

- An Error List filter for `SA` rules.
- **New Project** offers *Console App with StyleCop* (the course's `PackageReference` block exactly as in the assignment); the user can save their own project templates.
- Code Anvil generates follows VS' templates, not StyleCop (section 4), so it produces the same warnings VS would.

The old StyleCop VS extension is out of scope (the course does not use it).

---

## 7. AI (Ollama only)

Both parts are off by default, each with its own switch in Settings and a toolbar toggle, and a single **AI off** switch for exams. Nothing leaves the machine.

- **Inline completion:** grey ghost text after the cursor from a fill-in-the-middle model (e.g. `qwen2.5-coder`), requested 250 ms after typing stops, cancelled by any keystroke. **Tab** accepts, `Ctrl+Right` accepts a word, **Esc** dismisses. When IntelliSense's list is open, Tab belongs to IntelliSense, as in VS. Context: the current file around the cursor plus the signatures of types it uses (from the structure model).
- **Chat panel** (like Copilot Chat in VS): ask about the code, the selection, an error from the Error List. **Edits with preview** (Thomas' choice): the assistant proposes changes across files as a patch; Anvil shows each file as a Mono diff; the user accepts or rejects per file or per hunk; accepted edits are one undo step. It never runs commands and never writes without that review.
- Model choice per part from the models Ollama reports; status (model loading, unreachable) shown in plain words.

---

## 8. Design: Insights Mono

Rules: the vault note `references/insights-mono` and Atlas' `docs/DESIGN.md`; Anvil copies `insights-mono/` (tokens and fonts) and Atlas' token-to-CSS pipeline.

- Black, white, grey. Colour only for meaning: errors (`down`), passing tests and added diff lines (`up`), removed lines (`down`). Warnings told apart by shape and weight, not amber. Breakpoints a solid ink disc; the debugger's current line a `fill` band with a heavier ink mark. AI ghost text in `ink3`.
- Inter for the interface, Cascadia Mono for code and values, Geist Serif for statements and headings (start page, empty states, dialog and panel titles, diagram cards). The debugger's breakpoint red and current-statement yellow are VS' own and the only colour literals outside `tokens.json`.
- Layout as in VS where muscle memory counts: Solution Explorer right, editor centre, bottom panel with tabs (Error List, Output, Terminal, Test Explorer, debugger panels), Git Changes and AI chat docked right beside the Solution Explorer as tabs. Panels are Mono cards (`card` on `bg`, radius 22, no borders); the toolbar is a floating `bar` (radius 30) whose one `solid` button is **Start**.
- Motion from Islands: springs for panels and cards, 90/170 ms content fades, nothing animates at rest.
- Light and dark follow the system, with Atlas' sun/moon toggle.
- No emoji, no status dots, no glows; one icon family (Adwaita symbolic) at one weight.
- German and English UI, following the system language and switchable in Settings (Thomas' choice); strings through gettext as in Atlas.

---

## 9. Compatibility evidence (no Windows PC)

Because no Visual Studio is available for testing, every format Anvil writes is checked against **real files written by VS 2026**, kept as test fixtures in `anvil-core/tests/fixtures/` (with permission for the course assignment; public samples otherwise):

- `.slnx`, `.sln`, `.csproj`: read → write unchanged gives identical bytes; each edit gives what `dotnet` CLI writes.
- Item templates: the VS 2026 template texts (from VS' template packages and `dotnet new` templates) as golden files.
- `.cd`: identical bytes for unchanged diagrams; HashCode reproduced for every sample type.
- Class Designer code generation: golden files of what VS inserts for each Add action, gathered from documentation and public samples; where none is found, Anvil follows the item template and the case is listed in the README as *planned, not verified against VS*.

---

## 10. Data on disk

| What | Where |
| :--- | :--- |
| Settings, keymap overrides | GSettings `io.github.tomsliikee.Anvil` and `~/.var/app/io.github.tomsliikee.Anvil/config/anvil/keymap.json` |
| Recent solutions, window layout | `~/.var/app/io.github.tomsliikee.Anvil/config/anvil/` |
| Per-solution UI state (tabs, breakpoints, startup project, diagram view state) | `.vs/anvil/` in the solution folder (excluded from the hand-in ZIP) |
| Roslyn server, netcoredbg | `~/.local/share/anvil/` on the host |

Network: the two first-start downloads (with consent), nuget.org in the package manager and `dotnet restore`, git remotes. AI talks only to localhost.

---

## 11. Risks

1. **`.cd` HashCode** must be reproduced exactly; if the samples do not pin down the algorithm, VS will rewrite hashes and want to save after opening an Anvil diagram. Milestone 1 settles this first.
2. **Class Designer code-generation samples** may be incomplete without a VS to try; gaps are marked as unverified.
3. **Roslyn server outside VS Code**: custom notifications and options; versions pinned, updates tested.
4. **netcoredbg attach-with-hold** for `F5` with console input must be proven early in milestone 6.
5. **`flatpak-spawn --host`** makes the sandbox mostly symbolic; accepted, like GNOME Builder.
6. **GtkSourceView limits**: multi-caret is basic; ghost text, peek views and inline diffs need custom drawing.
7. **Keyboard chords and GNOME shortcuts** may collide (input methods, `Ctrl+.`); each clash is listed.

---

## 12. Milestones

Each milestone ends with something Thomas can use and is checked by running it.

1. **Formats**: `anvil-core` reads and writes `.slnx`, `.sln`, `.csproj`, `.cd` losslessly; HashCode reproduced for all sample types; item templates as golden files; hand-in ZIP builder. Unit tests only.
2. **Shell**: window, Mono tokens, start page, Solution Explorer with VS context menus, New Item dialog, editor tabs with highlighting, open/save with encoding kept, keymap engine with the VS scheme, German and English.
3. **Language**: Roslyn download and start, IntelliSense, squiggles, Error List, Quick Info, `F12` and friends, rename, quick actions, format, snippets; StyleCop warnings equal to `dotnet build` on the course assignment.
4. **Build and run**: build, Output, terminal panel, `Ctrl+F5`, Hot Reload, new solution/project, add files/projects/references, properties, Hand in as ZIP.
5. **Class Designer**: canvas from Atlas, code → diagram, `.cd` read/write, diagram → code, Class Details, toolbox, shared undo.
6. **Debugger**: netcoredbg attach-with-hold, breakpoints, stepping, Locals/Autos/Watch/Call Stack/Immediate.
7. **Test Explorer**.
8. **Git**.
9. **NuGet Package Manager**.
10. **AI**: inline completion, chat with edit preview.
11. **Polish**: keymap settings UI, every VS shortcut checked, Flatpak, README in the house style.

The class diagram comes before the debugger because it is Anvil's own reason to exist; the order after milestone 6 is free.

---

## 13. Left to the builder

- Widget structure, module layout inside crates, the LSP/DAP clients' internals. **No new dependency without Thomas' approval**: the implementation plan lists every crate with its reason (expected: `gtk4`, `libadwaita`, `sourceview5`, `vte4`, `cairo-rs`, `tokio`, `lsp-types`, `tree-sitter`, `tree-sitter-c-sharp`, `quick-xml`, `serde`/`serde_json`, `reqwest` or similar for Ollama/NuGet, `zip`).
- How the edit planner finds insertion points; auto-layout for new diagrams (Atlas' layout as a start).
- VSTest vs. Microsoft.Testing.Platform handling.
- App ID `io.github.tomsliikee.Anvil`, licence MIT, like Atlas.
