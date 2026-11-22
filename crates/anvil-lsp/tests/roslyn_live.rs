//! The real Roslyn server (as Anvil downloads it) against a sample
//! solution: quick actions with resolve, completion resolve with an
//! automatic `using`, definition into metadata, references,
//! implementations, workspace symbols and formatting. Needs `dotnet` and
//! the server in `~/.local/share/anvil/roslyn/`, so it is ignored by
//! default:
//!
//! `ANVIL_IDE_DEMO=<dir with IdeDemo.slnx> cargo test -p anvil-lsp --test roslyn_live -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anvil_lsp::client::{Client, Incoming};
use anvil_lsp::{navigation, roslyn};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver;

fn uri(p: &Path) -> String {
    roslyn::file_uri(p)
}

/// Answers the server's requests the way the app does and waits until
/// `method` arrives.
async fn wait_for(client: &Client, rx: &mut UnboundedReceiver<Incoming>, method: &str, secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let msg = match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(m)) => m,
            _ => return false,
        };
        match msg {
            Incoming::Notification { method: m, .. } if m == method => return true,
            Incoming::Request { id, method: m, params } => {
                if m == "workspace/_roslyn_projectNeedsRestore" {
                    for p in params["projectFilePaths"].as_array().cloned().unwrap_or_default() {
                        let _ = tokio::process::Command::new("dotnet").args(["restore", p.as_str().unwrap_or("")]).output().await;
                    }
                }
                client.respond(id, Value::Null);
            }
            Incoming::Exited => return false,
            _ => {}
        }
    }
}

fn pos_of(text: &str, needle: &str, plus: usize) -> Value {
    let at = text.find(needle).expect("needle in text") + plus;
    json!(anvil_lsp::position::to_position(text, at))
}

#[tokio::test]
#[ignore = "needs dotnet and the downloaded Roslyn server"]
async fn roslyn_ide_features() {
    let dir = PathBuf::from(std::env::var("ANVIL_IDE_DEMO").expect("ANVIL_IDE_DEMO"));
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let server = roslyn::install_dir(&home, roslyn::VERSION);
    let logs = std::env::temp_dir().join("anvil-roslyn-live");
    let tmp = roslyn::temp_dir(&home);
    std::fs::create_dir_all(&tmp).unwrap();
    let mut child = tokio::process::Command::new("dotnet")
        .args(roslyn::args(&server, &logs))
        .envs(roslyn::env(&home))
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("dotnet");
    let (client, mut rx) = Client::start(child.stdout.take().unwrap(), child.stdin.take().unwrap());
    let init = json!({"processId": null, "rootUri": uri(&dir), "workspaceFolders": [{"uri": uri(&dir), "name": "IdeDemo"}], "capabilities": roslyn::client_capabilities()});
    client.request_raw("initialize", init).await.expect("initialize");
    client.notify_raw("initialized", json!({}));
    client.notify_raw(roslyn::OPEN_SOLUTION, roslyn::open_solution_params(&dir.join("IdeDemo.slnx")));
    assert!(wait_for(&client, &mut rx, roslyn::PROJECTS_LOADED, 240).await, "projects loaded");

    let program = dir.join("IdeDemo/Program.cs");
    let shapes = dir.join("IdeDemo/Shapes.cs");
    let ptext = std::fs::read_to_string(&program).unwrap();
    let stext = std::fs::read_to_string(&shapes).unwrap();
    for (p, t) in [(&program, &ptext), (&shapes, &stext)] {
        client.notify_raw("textDocument/didOpen", json!({"textDocument": {"uri": uri(p), "languageId": "csharp", "version": 1, "text": t}}));
    }
    let doc = json!({"uri": uri(&program)});
    // Diagnostics settle a moment after the open.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let pull = client.request_raw("textDocument/diagnostic", json!({"textDocument": doc})).await.unwrap_or(Value::Null);
    let diags: Vec<Value> = pull["items"].as_array().cloned().unwrap_or_default();
    println!("diagnostics: {}", diags.iter().map(|d| format!("{} {}", d["code"], d["message"])).collect::<Vec<_>>().join(" | "));
    let cs0246: Vec<Value> = diags.iter().filter(|d| d["code"] == "CS0246").cloned().collect();
    assert!(!cs0246.is_empty(), "StringBuilder is unknown without a using");

    // Quick actions on StringBuilder: "using System.Text;" resolves to an edit.
    let range = cs0246[0]["range"].clone();
    let actions = client.request_raw("textDocument/codeAction", json!({"textDocument": doc, "range": range, "context": {"diagnostics": cs0246}})).await.expect("codeAction");
    let list = actions.as_array().cloned().unwrap_or_default();
    let titles: Vec<&str> = list.iter().filter_map(|a| a["title"].as_str()).collect();
    println!("code actions: {titles:?}");
    let using = list.iter().find(|a| a["title"].as_str().is_some_and(|t| t.contains("using System.Text"))).expect("add using action").clone();
    let resolved = if using.get("edit").is_some() { using } else { client.request_raw("codeAction/resolve", using).await.expect("resolve") };
    let edit = resolved.get("edit").cloned().expect("resolved action has an edit");
    let flat = navigation::workspace_edit_files(&serde_json::from_value(edit).unwrap());
    println!("using edit: {flat:?}");
    assert!(flat.iter().any(|(_, e)| e.iter().any(|t| t.new_text.contains("using System.Text;"))));

    // Code actions without a diagnostic (refactorings) at a member.
    let r = json!({"start": pos_of(&stext, "Radius", 1), "end": pos_of(&stext, "Radius", 1)});
    let refactors = client.request_raw("textDocument/codeAction", json!({"textDocument": {"uri": uri(&shapes)}, "range": r, "context": {"diagnostics": []}})).await.expect("refactorings");
    println!("refactorings: {:?}", refactors.as_array().map(|a| a.iter().filter_map(|x| x["title"].as_str()).collect::<Vec<_>>()));

    // Completion: `StringBuild` offers the unimported type; resolving it
    // brings the `using` as additionalTextEdits.
    let typed = ptext.replace("var sb = new StringBuilder();", "var sb = new StringBuild");
    client.notify_raw("textDocument/didChange", json!({"textDocument": {"uri": uri(&program), "version": 2}, "contentChanges": [anvil_lsp::position::change_event(&ptext, &typed)]}));
    let at = pos_of(&typed, "new StringBuild", "new StringBuild".len());
    let comp = client.request_raw("textDocument/completion", json!({"textDocument": doc, "position": at, "context": {"triggerKind": 1}})).await.expect("completion");
    let items = comp["items"].as_array().cloned().or_else(|| comp.as_array().cloned()).unwrap_or_default();
    let sb = items.iter().find(|i| i["label"] == "StringBuilder").cloned();
    println!("completion: {} items, StringBuilder: {}", items.len(), sb.as_ref().map(|i| i.to_string()).unwrap_or_default());
    if let Some(item) = sb {
        let r = client.request_raw("completionItem/resolve", item).await.expect("resolve item");
        println!("resolved: additionalTextEdits={} textEdit={}", r["additionalTextEdits"], r["textEdit"]);
        let extra: Vec<lsp_types::TextEdit> = serde_json::from_value(r["additionalTextEdits"].clone()).unwrap_or_default();
        assert!(extra.iter().any(|e| e.new_text.contains("using System.Text;")) || r["command"].is_object(), "auto-using after resolve");
    }
    client.notify_raw("textDocument/didChange", json!({"textDocument": {"uri": uri(&program), "version": 3}, "contentChanges": [anvil_lsp::position::change_event(&typed, &ptext)]}));

    // Go to Definition into metadata (Console): a file Anvil can read.
    let def = client.request_raw("textDocument/definition", json!({"textDocument": doc, "position": pos_of(&ptext, "Console.WriteLine", 2)})).await.expect("definition");
    let locs = navigation::locations(def.clone());
    println!("definition of Console: {def}");
    let target = locs.first().and_then(|l| roslyn::path_from_uri(l.uri.as_str())).expect("metadata location");
    assert!(target.exists(), "metadata-as-source file {} is readable", target.display());
    assert!(navigation::is_metadata(&target, &home));

    // Definition in source.
    let def = client.request_raw("textDocument/definition", json!({"textDocument": doc, "position": pos_of(&ptext, "Circle", 1)})).await.expect("definition");
    let l = navigation::locations(def);
    assert_eq!(l.len(), 1);
    assert_eq!(roslyn::path_from_uri(l[0].uri.as_str()).unwrap(), shapes);

    // References of Area: the interface member, both implementations, the call.
    let refs = client.request_raw("textDocument/references", json!({"textDocument": {"uri": uri(&shapes)}, "position": pos_of(&stext, "double Area", 8), "context": {"includeDeclaration": true}})).await.expect("references");
    let refs = navigation::locations(refs);
    let groups = navigation::group_by_file(&refs);
    println!("references: {} in {} files", refs.len(), groups.len());
    assert!(refs.len() >= 2 && groups.iter().any(|(p, _)| p.ends_with("Program.cs")));

    // Implementations of IShape.
    let imp = client.request_raw("textDocument/implementation", json!({"textDocument": {"uri": uri(&shapes)}, "position": pos_of(&stext, "IShape", 1)})).await.expect("implementation");
    let imp = navigation::locations(imp);
    println!("implementations: {}", imp.len());
    assert_eq!(imp.len(), 2);

    // Ctrl+T: workspace symbols.
    let sym = client.request_raw("workspace/symbol", json!({"query": "Circ"})).await.expect("workspace/symbol");
    let syms = navigation::symbols(sym);
    println!("symbols: {:?}", syms.iter().map(|s| &s.name).collect::<Vec<_>>());
    assert!(syms.iter().any(|s| s.name == "Circle"));

    // Formatting the messy lines.
    let fmt = client.request_raw("textDocument/formatting", json!({"textDocument": doc, "options": {"tabSize": 4, "insertSpaces": true}})).await.expect("formatting");
    let edits: Vec<lsp_types::TextEdit> = serde_json::from_value(fmt).unwrap_or_default();
    let formatted = navigation::apply_lsp_edits(&ptext, &edits);
    println!("formatted:\n{formatted}");
    assert!(formatted.contains("int x = 1 + 2;"));

    let _ = client.request_raw("shutdown", Value::Null).await;
    client.notify_raw("exit", Value::Null);
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
}
