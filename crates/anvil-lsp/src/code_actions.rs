//! Roslyn's code actions as a flat list, and how each one is resolved.
//!
//! Roslyn (5.x) hands two kinds of action that are not plain edits. Both
//! carry a `roslyn.client.*` command that only the VS Code extension
//! implements, so sending it to the server fails:
//!
//! - **Nested** (`roslyn.client.nestedCodeAction`, "Suppress or configure
//!   issues"): the choices sit in `command.arguments[0].NestedCodeActions`,
//!   each resolved by `codeAction/resolve` like any other action.
//! - **Fix All** (`roslyn.client.fixAllCodeAction`): resolved by
//!   `codeAction/resolveFixAll` with one of the action's
//!   `FixAllFlavors` (`Document`, `Project`, `Solution`, …) as the scope.

use serde_json::{Value, json};

pub const NESTED: &str = "roslyn.client.nestedCodeAction";
pub const FIX_ALL: &str = "roslyn.client.fixAllCodeAction";
pub const RESOLVE: &str = "codeAction/resolve";
pub const RESOLVE_FIX_ALL: &str = "codeAction/resolveFixAll";

/// One entry of the Quick Actions list.
#[derive(Clone, Debug, PartialEq)]
pub struct Offered {
    /// The path of titles down to this action, without the scope.
    pub path: Vec<String>,
    pub raw: Value,
    /// For a Fix All entry: the scope it fixes in.
    pub scope: Option<String>,
}

/// Commands only a client can run; never sent to the server.
pub fn is_client_command(command: &str) -> bool {
    command.starts_with("roslyn.client.")
}

fn command_of(a: &Value) -> Option<&str> {
    a.pointer("/command/command").and_then(Value::as_str)
}

fn argument(a: &Value) -> Option<&Value> {
    a.pointer("/command/arguments/0")
}

/// The titles from the top-level action down to `a`: Roslyn's own
/// `CodeActionPath` when it has one ("Suppress or configure issues",
/// "Suppress SA1400", "in Source"), else the parent's path and the title.
fn path_of(a: &Value, parent: &[String]) -> Vec<String> {
    let title = a.get("title").and_then(Value::as_str).unwrap_or("?").to_owned();
    let roslyn = a.pointer("/data/CodeActionPath").or_else(|| argument(a).and_then(|d| d.get("CodeActionPath"))).and_then(Value::as_array);
    match roslyn {
        Some(p) if !parent.is_empty() && p.len() > parent.len() => {
            // Roslyn's path names the plain action ("Declare accessibility")
            // for a Fix All; the title says "Fix All: …".
            let mut v: Vec<String> = p.iter().filter_map(Value::as_str).map(str::to_owned).collect();
            if let Some(last) = v.last_mut() {
                *last = title;
            }
            v
        }
        _ => parent.iter().cloned().chain([title]).collect(),
    }
}

fn push(a: &Value, parent: &[String], out: &mut Vec<Offered>) {
    let path = path_of(a, parent);
    match command_of(a) {
        Some(NESTED) => {
            for n in argument(a).and_then(|d| d.get("NestedCodeActions")).and_then(Value::as_array).into_iter().flatten() {
                push(n, &path, out);
            }
        }
        Some(FIX_ALL) => {
            let flavors = argument(a).and_then(|d| d.get("FixAllFlavors")).and_then(Value::as_array);
            for f in flavors.into_iter().flatten().filter_map(Value::as_str) {
                out.push(Offered { path: path.clone(), raw: a.clone(), scope: Some(f.to_owned()) });
            }
        }
        _ => match a.pointer("/data/NestedCodeActions").and_then(Value::as_array).filter(|n| !n.is_empty()) {
            // Older servers kept the nested actions in `data`.
            Some(nested) => {
                for n in nested {
                    push(n, &path, out);
                }
            }
            None => out.push(Offered { path, raw: a.clone(), scope: None }),
        },
    }
}

/// The server's list with nested actions offered one by one and each Fix
/// All once per scope.
pub fn flatten(list: &[Value]) -> Vec<Offered> {
    let mut out = Vec::new();
    for a in list {
        push(a, &[], &mut out);
    }
    out
}

/// The request that turns `o` into an action with an edit, or `None` when
/// it already has one (or has nothing to resolve).
pub fn resolve_request(o: &Offered) -> Option<(&'static str, Value)> {
    if let Some(scope) = &o.scope {
        let data = argument(&o.raw).or_else(|| o.raw.get("data")).cloned().unwrap_or(Value::Null);
        let title = data.get("UniqueIdentifier").cloned().or_else(|| o.raw.get("title").cloned()).unwrap_or(Value::Null);
        return Some((RESOLVE_FIX_ALL, json!({"title": title, "data": data, "scope": scope})));
    }
    if o.raw.get("edit").is_some() || o.raw.get("data").is_none() {
        return None;
    }
    Some((RESOLVE, o.raw.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shapes as Roslyn 5.4.0-2.26179.14 sent them for SA1400 on
    /// `class Program` (trimmed to the fields read here).
    fn sa1400() -> Vec<Value> {
        vec![
            json!({"title": "Declare accessibility", "kind": "quickfix", "data": {"UniqueIdentifier": "Declare accessibility", "CodeActionPath": ["Declare accessibility"]}}),
            json!({"title": "Fix All: Declare accessibility", "command": {"command": FIX_ALL, "arguments": [{"UniqueIdentifier": "Fix All: Declare accessibility", "CodeActionPath": ["Declare accessibility"], "FixAllFlavors": ["Document", "Project", "Solution"]}]}, "data": {"UniqueIdentifier": "Fix All: Declare accessibility", "FixAllFlavors": ["Document", "Project", "Solution"]}}),
            json!({"title": "Suppress or configure issues", "command": {"command": NESTED, "arguments": [{"CodeActionPath": ["Suppress or configure issues"], "NestedCodeActions": [
                {"title": "in Source", "data": {"UniqueIdentifier": "in Source", "CodeActionPath": ["Suppress or configure issues", "Suppress SA1400", "in Source"]}},
                {"title": "Fix All: in Source", "command": {"command": FIX_ALL, "arguments": [{"UniqueIdentifier": "Fix All: in Source", "CodeActionPath": ["Suppress or configure issues", "Suppress SA1400", "in Source"], "FixAllFlavors": ["Document"]}]}},
                {"title": "Warning", "data": {"UniqueIdentifier": "Warning", "CodeActionPath": ["Suppress or configure issues", "Configure SA1400 severity", "Warning"]}}
            ]}]}}),
        ]
    }

    fn titles(o: &[Offered]) -> Vec<String> {
        o.iter().map(|o| o.path.join(" › ") + &o.scope.as_ref().map(|s| format!(" [{s}]")).unwrap_or_default()).collect()
    }

    #[test]
    fn nested_and_fix_all_are_offered_one_by_one() {
        assert_eq!(
            titles(&flatten(&sa1400())),
            [
                "Declare accessibility",
                "Fix All: Declare accessibility [Document]",
                "Fix All: Declare accessibility [Project]",
                "Fix All: Declare accessibility [Solution]",
                "Suppress or configure issues › Suppress SA1400 › in Source",
                "Suppress or configure issues › Suppress SA1400 › Fix All: in Source [Document]",
                "Suppress or configure issues › Configure SA1400 severity › Warning",
            ]
        );
    }

    #[test]
    fn older_servers_nest_in_data() {
        let list = vec![json!({"title": "Suppress or configure issues", "data": {"NestedCodeActions": [{"title": "Suppress CA1859"}]}})];
        assert_eq!(titles(&flatten(&list)), ["Suppress or configure issues › Suppress CA1859"]);
    }

    #[test]
    fn fix_all_resolves_with_its_scope_and_never_runs_the_client_command() {
        let o = flatten(&sa1400());
        let (method, params) = resolve_request(&o[2]).unwrap();
        assert_eq!(method, RESOLVE_FIX_ALL);
        assert_eq!(params["scope"], "Project");
        assert_eq!(params["title"], "Fix All: Declare accessibility");
        assert_eq!(params["data"]["FixAllFlavors"][0], "Document");
        assert!(is_client_command(FIX_ALL) && is_client_command(NESTED) && !is_client_command("dotnet.test.run"));
    }

    #[test]
    fn plain_actions_resolve_once() {
        let o = flatten(&sa1400());
        assert_eq!(resolve_request(&o[0]).unwrap().0, RESOLVE);
        assert_eq!(resolve_request(&o[4]).unwrap().0, RESOLVE);
        let done = Offered { path: vec!["x".into()], raw: json!({"title": "x", "edit": {}, "data": {}}), scope: None };
        assert!(resolve_request(&done).is_none());
    }
}
