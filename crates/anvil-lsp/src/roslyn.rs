//! Microsoft's Roslyn language server (`Microsoft.CodeAnalysis.
//! LanguageServer`, MIT): where Anvil keeps it, how it is started, and the
//! custom messages it needs beyond plain LSP.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The pinned server build (5.4.0-2.26179.14, `net10.0`, needs the .NET 10
/// runtime; *tested*: package downloaded and inspected 2026-11-18).
pub const VERSION: &str = "5.4.0-2.26179.14";
pub const PACKAGE: &str = "Microsoft.CodeAnalysis.LanguageServer.linux-x64";
/// Roslyn's server is published on Microsoft's public `vs-impl` feed, not
/// nuget.org.
pub const FEED_INDEX: &str = "https://pkgs.dev.azure.com/azure-public/vside/_packaging/vs-impl/nuget/v3/index.json";
/// Folder inside the package that holds the server.
pub const CONTENT_DIR: &str = "content/LanguageServer/linux-x64/";
pub const DLL: &str = "Microsoft.CodeAnalysis.LanguageServer.dll";

/// `~/.local/share/anvil/roslyn/<version>/` on the host (the Flatpak has
/// the same home).
pub fn install_dir(home: &Path, version: &str) -> PathBuf {
    home.join(".local/share/anvil/roslyn").join(version)
}

pub fn is_installed(home: &Path, version: &str) -> bool {
    install_dir(home, version).join(DLL).exists()
}

/// The package URL from the feed's `PackageBaseAddress`.
pub fn package_url(base_address: &str, version: &str) -> String {
    let id = PACKAGE.to_lowercase();
    format!("{}/{id}/{version}/{id}.{version}.nupkg", base_address.trim_end_matches('/'))
}

/// `PackageBaseAddress/3.0.0` of a NuGet v3 service index.
pub fn base_address(index_json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(index_json).ok()?;
    v.get("resources")?.as_array()?.iter().find(|r| r.get("@type").and_then(Value::as_str).is_some_and(|t| t.starts_with("PackageBaseAddress")))?.get("@id")?.as_str().map(str::to_owned)
}

/// The server's command line (run with `dotnet` on the host).
pub fn args(dir: &Path, log_dir: &Path) -> Vec<String> {
    vec![
        dir.join(DLL).to_string_lossy().into_owned(),
        "--logLevel".into(),
        "Information".into(),
        "--extensionLogDirectory".into(),
        log_dir.to_string_lossy().into_owned(),
        "--stdio".into(),
    ]
}

/// Where the server writes its temporary files, above all the
/// decompiled "metadata as source" files of Go To Definition on a library
/// type. Roslyn runs on the host; the Flatpak has its own `/tmp` but
/// shares the home, so the folder is under the home.
pub fn temp_dir(home: &Path) -> PathBuf {
    home.join(".cache/anvil/roslyn-tmp")
}

/// The server's environment beyond `dotnet`'s: `TMPDIR` (see [`temp_dir`]).
pub fn env(home: &Path) -> Vec<(String, String)> {
    vec![("TMPDIR".into(), temp_dir(home).to_string_lossy().into_owned())]
}

/// Text of a source-generated document (`roslyn-source-generated://`
/// URIs from Go To Definition): `{"textDocument": {"uri"}}` → `{"text"}`.
pub const SOURCE_GENERATED_TEXT: &str = "sourceGeneratedDocument/_roslyn_getText";

/// Roslyn does not load anything by itself: after `initialized` the client
/// names the solution (`solution/open`) or the projects (`project/open`).
pub fn open_solution_params(solution: &Path) -> Value {
    json!({ "solution": file_uri(solution) })
}

pub fn open_projects_params(projects: &[PathBuf]) -> Value {
    json!({ "projects": projects.iter().map(|p| file_uri(p)).collect::<Vec<_>>() })
}

pub const OPEN_SOLUTION: &str = "solution/open";
pub const OPEN_PROJECTS: &str = "project/open";
/// Sent by the server when every project is loaded; until then completion
/// and diagnostics are incomplete.
pub const PROJECTS_LOADED: &str = "workspace/projectInitializationComplete";

/// A `file://` URI with percent-encoding for the characters that need it.
pub fn file_uri(p: &Path) -> String {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(char::from(b));
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

/// The `initialize` capabilities Anvil uses: completion with snippets and
/// resolve, hover in markdown, signature help, pull and push diagnostics,
/// definitions, references, rename, code actions with resolve,
/// formatting, semantic tokens, document symbols, workspace edits.
pub fn client_capabilities() -> Value {
    json!({
        "workspace": {
            "applyEdit": true,
            "workspaceEdit": { "documentChanges": true, "resourceOperations": ["create", "rename", "delete"] },
            "configuration": true,
            "didChangeWatchedFiles": { "dynamicRegistration": true },
            "diagnostics": { "refreshSupport": true },
            "semanticTokens": { "refreshSupport": true }
        },
        "textDocument": {
            "synchronization": { "didSave": true, "willSave": false, "dynamicRegistration": false },
            "completion": {
                "completionItem": { "snippetSupport": true, "documentationFormat": ["markdown", "plaintext"], "resolveSupport": { "properties": ["documentation", "detail", "additionalTextEdits"] }, "insertReplaceSupport": false },
                "contextSupport": true
            },
            "hover": { "contentFormat": ["markdown", "plaintext"] },
            "signatureHelp": { "signatureInformation": { "documentationFormat": ["markdown", "plaintext"], "parameterInformation": { "labelOffsetSupport": true } } },
            "definition": { "linkSupport": false },
            "implementation": { "linkSupport": false },
            "references": {},
            "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
            "codeAction": { "codeActionLiteralSupport": { "codeActionKind": { "valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"] } }, "resolveSupport": { "properties": ["edit"] }, "dataSupport": true },
            "formatting": {},
            "rangeFormatting": {},
            "rename": { "prepareSupport": true },
            "publishDiagnostics": { "relatedInformation": false, "versionSupport": true },
            "diagnostic": { "dynamicRegistration": true },
            "semanticTokens": { "requests": { "full": true, "range": true }, "tokenTypes": ["namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter", "variable", "property", "enumMember", "event", "function", "method", "macro", "keyword", "modifier", "comment", "string", "number", "regexp", "operator"], "tokenModifiers": ["declaration", "definition", "readonly", "static", "deprecated", "abstract"], "formats": ["relative"] }
        },
        "window": { "workDoneProgress": true }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip_with_spaces_and_umlauts() {
        let p = Path::new("/home/t/Übung 1/A.slnx");
        let u = file_uri(p);
        assert_eq!(u, "file:///home/t/%C3%9Cbung%201/A.slnx");
        assert_eq!(path_from_uri(&u).unwrap(), p);
    }

    #[test]
    fn package_location() {
        let idx = r#"{"resources":[{"@id":"https://x/flat2/","@type":"PackageBaseAddress/3.0.0"}]}"#;
        let base = base_address(idx).unwrap();
        assert_eq!(package_url(&base, VERSION), format!("https://x/flat2/microsoft.codeanalysis.languageserver.linux-x64/{VERSION}/microsoft.codeanalysis.languageserver.linux-x64.{VERSION}.nupkg"));
        let a = args(Path::new("/r"), Path::new("/l"));
        assert_eq!(a.last().unwrap(), "--stdio");
        assert_eq!(open_solution_params(Path::new("/a/S.slnx"))["solution"], "file:///a/S.slnx");
    }
}
