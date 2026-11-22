//! NuGet Package Manager data: the v3 search API's JSON, the flat
//! container's version list, and package sources from `NuGet.Config`.

use serde::Deserialize;

pub const NUGET_ORG: &str = "https://api.nuget.org/v3/index.json";
/// nuget.org's search endpoint, used when the service index cannot be read.
pub const NUGET_ORG_SEARCH: &str = "https://azuresearch-usnc.nuget.org/query";

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Package {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub authors: Authors,
    #[serde(default, rename = "totalDownloads")]
    pub total_downloads: u64,
    #[serde(default)]
    pub versions: Vec<VersionInfo>,
    #[serde(default)]
    pub verified: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Default)]
#[serde(untagged)]
pub enum Authors {
    One(String),
    Many(Vec<String>),
    #[default]
    None,
}

impl Authors {
    pub fn joined(&self) -> String {
        match self {
            Authors::One(s) => s.clone(),
            Authors::Many(v) => v.join(", "),
            Authors::None => String::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct VersionInfo {
    pub version: String,
}

#[derive(Deserialize)]
struct SearchReply {
    data: Vec<Package>,
}

pub fn parse_search(json: &str) -> Result<Vec<Package>, serde_json::Error> {
    Ok(serde_json::from_str::<SearchReply>(json)?.data)
}

/// The search endpoint of a v3 service index.
pub fn search_service(index_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(index_json).ok()?;
    v.get("resources")?
        .as_array()?
        .iter()
        .find(|r| r.get("@type").and_then(|t| t.as_str()).is_some_and(|t| t.starts_with("SearchQueryService")))
        .and_then(|r| r.get("@id")?.as_str().map(str::to_owned))
}

pub fn search_url(endpoint: &str, query: &str, prerelease: bool, skip: usize) -> String {
    format!("{endpoint}?q={}&skip={skip}&take=25&prerelease={prerelease}&semVerLevel=2.0.0", percent_encode(query))
}

/// All versions of a package, newest first, from the flat container
/// (`…/v3-flatcontainer/<id>/index.json`).
pub fn parse_versions(json: &str) -> Vec<String> {
    #[derive(Deserialize)]
    struct V {
        versions: Vec<String>,
    }
    let mut v = serde_json::from_str::<V>(json).map(|v| v.versions).unwrap_or_default();
    v.reverse();
    v
}

/// The version Install picks first: the newest stable one (VS' "Latest
/// stable"), or the newest of all when prereleases are wanted or there is
/// no stable one. `versions` is newest first.
pub fn default_version(versions: &[String], prerelease: bool) -> usize {
    if prerelease {
        return 0;
    }
    versions.iter().position(|v| !v.contains('-')).unwrap_or(0)
}

pub fn versions_url(id: &str) -> String {
    format!("https://api.nuget.org/v3-flatcontainer/{}/index.json", id.to_lowercase())
}

pub fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { char::from(b).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// Package sources of a `NuGet.Config` (`<clear />` drops the inherited
/// ones; `<disabledPackageSources>` removes by key).
pub fn sources(config: &str, inherited: &[(String, String)]) -> Vec<(String, String)> {
    let Ok(doc) = crate::xml::Doc::parse(config.trim_start_matches('\u{feff}')) else { return inherited.to_vec() };
    let mut out = inherited.to_vec();
    if let Some(ps) = doc.child(0, "packageSources") {
        for c in doc.children(ps) {
            match doc.el(c).name.as_str() {
                "clear" => out.clear(),
                "add" => {
                    if let (Some(k), Some(v)) = (doc.attr(c, "key"), doc.attr(c, "value")) {
                        out.retain(|(ek, _)| ek != k);
                        out.push((k.to_owned(), v.to_owned()));
                    }
                }
                "remove" => {
                    if let Some(k) = doc.attr(c, "key") {
                        out.retain(|(ek, _)| ek != k);
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(dis) = doc.child(0, "disabledPackageSources") {
        for c in doc.children(dis) {
            if doc.attr(c, "value") == Some("true")
                && let Some(k) = doc.attr(c, "key")
            {
                out.retain(|(ek, _)| ek != k);
            }
        }
    }
    out
}

/// A top-level package of `dotnet list <project> package --format json`
/// (with `--outdated`, `latest` is the newest version on the sources).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedPackage {
    pub id: String,
    pub requested: String,
    pub resolved: String,
    pub latest: Option<String>,
}

/// Packages (deduplicated over target frameworks) and the problems the SDK
/// reported, e.g. "No assets file was found … run a NuGet package restore".
pub fn parse_list(json: &str) -> Result<(Vec<ListedPackage>, Vec<String>), String> {
    #[derive(Deserialize)]
    struct Pkg {
        id: String,
        #[serde(default, rename = "requestedVersion")]
        requested: String,
        #[serde(default, rename = "resolvedVersion")]
        resolved: String,
        #[serde(default, rename = "latestVersion")]
        latest: Option<String>,
    }
    #[derive(Deserialize)]
    struct Fw {
        #[serde(default, rename = "topLevelPackages")]
        top: Vec<Pkg>,
    }
    #[derive(Deserialize)]
    struct Proj {
        #[serde(default)]
        frameworks: Vec<Fw>,
    }
    #[derive(Deserialize)]
    struct Problem {
        #[serde(default)]
        text: String,
    }
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        projects: Vec<Proj>,
        #[serde(default)]
        problems: Vec<Problem>,
    }
    // The SDK may print a line before the JSON; start at the first brace.
    let start = json.find('{').ok_or_else(|| json.trim().to_owned())?;
    let r: Reply = serde_json::from_str(&json[start..]).map_err(|e| e.to_string())?;
    let mut out: Vec<ListedPackage> = Vec::new();
    for p in r.projects.into_iter().flat_map(|p| p.frameworks).flat_map(|f| f.top) {
        if out.iter().any(|o| o.id.eq_ignore_ascii_case(&p.id)) {
            continue;
        }
        // "Not found at the sources" is what --outdated says for a
        // package it cannot look up.
        let latest = p.latest.filter(|l| !l.is_empty() && !l.contains(' '));
        out.push(ListedPackage { id: p.id, requested: p.requested, resolved: p.resolved, latest });
    }
    Ok((out, r.problems.into_iter().map(|p| p.text).filter(|t| !t.is_empty()).collect()))
}

/// The `error: …` / `NU1101`-style lines of a failed `dotnet add/remove
/// package` or restore, for a short message; the whole output goes to the
/// Output window.
pub fn error_lines(out: &str) -> String {
    let errs: Vec<&str> = out.lines().map(str::trim).filter(|l| l.starts_with("error") || l.contains(": error ") || l.contains("error NU")).collect();
    if errs.is_empty() { out.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").to_owned() } else { errs.join("\n") }
}

pub fn default_sources() -> Vec<(String, String)> {
    vec![("nuget.org".to_owned(), NUGET_ORG.to_owned())]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_version_is_latest_stable() {
        let v: Vec<String> = ["1.2.0-beta.556", "1.2.0-beta.507", "1.1.118", "1.0.0"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(default_version(&v, false), 2);
        assert_eq!(default_version(&v, true), 0);
        assert_eq!(default_version(&["2.0.0-rc.1".to_owned()], false), 0);
        assert_eq!(default_version(&[], false), 0);
    }

    #[test]
    fn parses_search_reply() {
        let json = r#"{"totalHits":1,"data":[{"id":"StyleCop.Analyzers","version":"1.1.118","description":"An implementation of StyleCop rules using the .NET Compiler Platform","authors":["Sam Harwell"],"totalDownloads":123,"verified":true,"versions":[{"version":"1.1.118","downloads":5}]}]}"#;
        let p = parse_search(json).unwrap();
        assert_eq!(p[0].id, "StyleCop.Analyzers");
        assert_eq!(p[0].authors.joined(), "Sam Harwell");
        assert_eq!(p[0].versions[0].version, "1.1.118");
        assert_eq!(parse_versions(r#"{"versions":["1.0.0","1.1.118"]}"#), ["1.1.118", "1.0.0"]);
        assert_eq!(search_url("https://s/q", "style cop", false, 0), "https://s/q?q=style%20cop&skip=0&take=25&prerelease=false&semVerLevel=2.0.0");
        assert_eq!(search_service(r#"{"resources":[{"@id":"https://x/query","@type":"SearchQueryService/3.5.0"}]}"#).as_deref(), Some("https://x/query"));
    }

    #[test]
    fn parses_dotnet_list_package_json() {
        // As `dotnet list Lib.XunitTests package --format json --outdated`
        // (SDK 10.0.113) printed it, trimmed.
        let json = r#"{
  "version": 1,
  "parameters": "--outdated",
  "sources": ["https://api.nuget.org/v3/index.json"],
  "projects": [
    {
      "path": "/p/Lib.XunitTests/Lib.XunitTests.csproj",
      "frameworks": [
        { "framework": "net10.0", "topLevelPackages": [
            { "id": "coverlet.collector", "requestedVersion": "6.0.4", "resolvedVersion": "6.0.4", "latestVersion": "10.1.1" },
            { "id": "xunit.runner.visualstudio", "requestedVersion": "3.1.4", "resolvedVersion": "3.1.4", "latestVersion": "Not found at the sources" } ] },
        { "framework": "net8.0", "topLevelPackages": [
            { "id": "coverlet.collector", "requestedVersion": "6.0.4", "resolvedVersion": "6.0.4", "latestVersion": "10.1.1" } ] }
      ]
    }
  ]
}"#;
        let (p, problems) = parse_list(json).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0], ListedPackage { id: "coverlet.collector".into(), requested: "6.0.4".into(), resolved: "6.0.4".into(), latest: Some("10.1.1".into()) });
        assert_eq!(p[1].latest, None);
        assert!(problems.is_empty());
        let (p, _) = parse_list(r#"{"version":1,"parameters":"","projects":[{"path":"/p/Lib.csproj","frameworks":[{"framework":"net10.0"}]}]}"#).unwrap();
        assert!(p.is_empty());
        let (_, problems) = parse_list(r#"{"version":1,"problems":[{"project":"/p/A.csproj","level":"error","text":"No assets file was found for `/p/A.csproj`. Please run restore before running this command."}],"projects":[]}"#).unwrap();
        assert!(problems[0].starts_with("No assets file"));
        assert!(parse_list("error: not json").is_err());
    }

    #[test]
    fn short_error_of_dotnet_add_package() {
        let out = "  Determining projects to restore...\n  Writing /tmp/x.tmp\ninfo : Adding PackageReference for package 'Nope.Nope' into project '/p/A.csproj'.\nerror: There are no versions available for the package 'Nope.Nope'.\n";
        assert_eq!(error_lines(out), "error: There are no versions available for the package 'Nope.Nope'.");
        assert_eq!(error_lines("something went wrong\n"), "something went wrong");
    }

    #[test]
    fn nuget_config_sources() {
        let cfg = r#"<?xml version="1.0" encoding="utf-8"?>
<configuration>
  <packageSources>
    <clear />
    <add key="local" value="/home/t/feed" />
    <add key="nuget.org" value="https://api.nuget.org/v3/index.json" protocolVersion="3" />
  </packageSources>
  <disabledPackageSources>
    <add key="local" value="true" />
  </disabledPackageSources>
</configuration>"#;
        assert_eq!(sources(cfg, &[("old".into(), "x".into())]), [("nuget.org".to_owned(), NUGET_ORG.to_owned())]);
    }
}
