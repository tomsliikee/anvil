//! The classic `.sln` text format. Only project lines are read; everything
//! else is kept as text because membership changes go through `dotnet sln`.

use crate::slnx::ProjectRef;

/// Project type GUID of a solution folder.
pub const FOLDER_TYPE: &str = "{2150E333-8FDC-42A3-9474-1A3956D46DE8}";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlnProject {
    pub type_guid: String,
    pub name: String,
    pub path: String,
    pub guid: String,
}

pub fn parse_projects(text: &str) -> Vec<SlnProject> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix("Project(\"")?;
            let (type_guid, rest) = rest.split_once("\")")?;
            let rest = rest.trim_start().strip_prefix('=')?;
            let parts: Vec<&str> = rest.split(',').map(|p| p.trim().trim_matches('"')).collect();
            if parts.len() < 3 {
                return None;
            }
            Some(SlnProject { type_guid: type_guid.to_owned(), name: parts[0].to_owned(), path: parts[1].to_owned(), guid: parts[2].to_owned() })
        })
        .collect()
}

/// The real projects (no solution folders), in file order.
pub fn projects(text: &str) -> Vec<ProjectRef> {
    parse_projects(text)
        .into_iter()
        .filter(|p| !p.type_guid.eq_ignore_ascii_case(FOLDER_TYPE))
        .map(|p| ProjectRef { name: p.name, path: p.path.replace('\\', "/"), folder: None })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sdk_sln() {
        let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sdk-samples/Sample.sln")).unwrap();
        let p = projects(&text);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].name, "App");
        assert_eq!(p[0].path, "App/App.csproj");
        assert_eq!(p[1].path, "My Lib/Lib.csproj");
        assert_eq!(parse_projects(&text).len(), 3);
    }
}
