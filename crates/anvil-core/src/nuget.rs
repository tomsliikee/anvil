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
