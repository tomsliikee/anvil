//! `Properties/launchSettings.json`, where Visual Studio keeps the
//! command-line arguments of a project.

use serde_json::{json, Map, Value};

use crate::text::{Encoding, Eol, TextFile};

/// The arguments of the first `Project` profile (VS starts that one).
pub fn args(text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text).ok()?;
    let profiles = v.get("profiles")?.as_object()?;
    profiles.values().find(|p| p.get("commandName").and_then(Value::as_str) == Some("Project"))?.get("commandLineArgs")?.as_str().map(str::to_owned)
}

fn project_profile(v: &Value) -> Option<&Map<String, Value>> {
    v.get("profiles")?.as_object()?.values().find(|p| p.get("commandName").and_then(Value::as_str) == Some("Project"))?.as_object()
}

/// The environment variables of the first `Project` profile, which VS sets
/// for F5 and Ctrl+F5.
pub fn env(text: &str) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    let Some(vars) = project_profile(&v).and_then(|p| p.get("environmentVariables")).and_then(Value::as_object) else { return Vec::new() };
    vars.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned()))).collect()
}

/// Sets the arguments, creating the file's content when there is none.
/// VS writes this file itself with two-space indentation; Anvil writes the
/// same shape and keeps BOM and line endings of an existing file.
pub fn set_args(existing: Option<&TextFile>, project_name: &str, args: &str) -> TextFile {
    // A file that is JSON but not the expected shape is started afresh
    // rather than crashing the Properties dialog.
    let mut v: Value = existing.and_then(|f| serde_json::from_str(&f.text).ok()).filter(Value::is_object).unwrap_or_else(|| json!({ "profiles": {} }));
    let root = v.as_object_mut().expect("checked above");
    if !root.get("profiles").is_some_and(Value::is_object) {
        root.insert("profiles".into(), Value::Object(Map::new()));
    }
    let profiles = root.get_mut("profiles").and_then(Value::as_object_mut).expect("checked above");
    let key = profiles
        .iter()
        .find(|(_, p)| p.get("commandName").and_then(Value::as_str) == Some("Project"))
        .map(|(k, _)| k.clone())
        .unwrap_or_else(|| project_name.to_owned());
    let profile = profiles.entry(key).or_insert_with(|| json!({ "commandName": "Project" }));
    if !profile.is_object() {
        *profile = json!({ "commandName": "Project" });
    }
    let obj = profile.as_object_mut().expect("checked above");
    if args.is_empty() {
        obj.remove("commandLineArgs");
    } else {
        obj.insert("commandLineArgs".into(), Value::String(args.to_owned()));
    }
    let text = serde_json::to_string_pretty(&v).expect("serialises");
    match existing {
        Some(f) => TextFile { text, ..f.clone() },
        None => TextFile { encoding: Encoding::Utf8 { bom: false }, eol: Eol::Crlf, text },
    }
}

/// Splits a command line like the .NET runtime does on Windows for
/// `Main(string[] args)`: spaces separate, double quotes group.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_json_does_not_panic() {
        for t in ["[1]", r#"{"profiles":3}"#, r#"{"profiles":{"P":5}}"#] {
            let f = TextFile { encoding: Encoding::Utf8 { bom: false }, eol: Eol::Lf, text: t.into() };
            assert_eq!(args(&set_args(Some(&f), "P", "x").text).as_deref(), Some("x"), "{t}");
        }
    }

    #[test]
    fn reads_environment_variables() {
        let t = r#"{"profiles":{"App":{"commandName":"Project","environmentVariables":{"A":"1","B":"x y"}},"IIS":{"commandName":"IISExpress"}}}"#;
        assert_eq!(env(t), [("A".to_owned(), "1".to_owned()), ("B".to_owned(), "x y".to_owned())]);
        assert!(env("[1]").is_empty());
        assert!(env("not json").is_empty());
    }

    #[test]
    fn reads_and_sets_args() {
        let f = set_args(None, "App", "a \"b c\"");
        assert!(f.text.contains("\"commandName\": \"Project\""));
        assert_eq!(args(&f.text).as_deref(), Some("a \"b c\""));
        assert_eq!(split_args("a \"b c\"  d"), ["a", "b c", "d"]);
        let g = set_args(Some(&f), "App", "");
        assert_eq!(args(&g.text), None);
        assert_eq!(g.eol, Eol::Crlf);
    }

    #[test]
    fn keeps_other_profiles() {
        let text = "{\n  \"profiles\": {\n    \"WSL\": { \"commandName\": \"WSL2\" },\n    \"App\": { \"commandName\": \"Project\", \"commandLineArgs\": \"x\" }\n  }\n}";
        let f = TextFile::decode(text.as_bytes());
        assert_eq!(args(text).as_deref(), Some("x"));
        let g = set_args(Some(&f), "App", "y");
        assert!(g.text.contains("WSL2"));
        assert_eq!(args(&g.text).as_deref(), Some("y"));
    }
}
