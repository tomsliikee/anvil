//! Command lines for the operations (`git` arguments; the app runs them
//! on the host in the repository folder).

pub fn stage(paths: &[&str]) -> Vec<String> {
    let mut a = vec!["add".into(), "--".into()];
    a.extend(paths.iter().map(|p| (*p).to_owned()));
    a
}

pub fn unstage(paths: &[&str]) -> Vec<String> {
    let mut a = vec!["restore".into(), "--staged".into(), "--".into()];
    a.extend(paths.iter().map(|p| (*p).to_owned()));
    a
}

/// Discard changes in the working tree (VS: *Undo Changes*).
pub fn discard(paths: &[&str]) -> Vec<String> {
    let mut a = vec!["restore".into(), "--".into()];
    a.extend(paths.iter().map(|p| (*p).to_owned()));
    a
}

/// The message goes through stdin (`-F -`), so no quoting matters.
pub fn commit(amend: bool, all: bool) -> Vec<String> {
    let mut a = vec!["commit".to_owned(), "-F".to_owned(), "-".to_owned()];
    if amend {
        a.push("--amend".into());
    }
    if all {
        a.push("--all".into());
    }
    a
}

pub fn branches() -> Vec<String> {
    vec!["branch".into(), "--all".into(), "--format=%(HEAD)%(refname:short)".into()]
}

pub fn parse_branches(out: &str) -> (Option<String>, Vec<String>) {
    let mut current = None;
    let mut all = Vec::new();
    for l in out.lines() {
        let (head, name) = l.split_at(1.min(l.len()));
        if name.is_empty() || name.ends_with("/HEAD") {
            continue;
        }
        if head == "*" {
            current = Some(name.to_owned());
        }
        all.push(name.to_owned());
    }
    (current, all)
}

pub fn create_branch(name: &str) -> Vec<String> {
    vec!["switch".into(), "-c".into(), name.into()]
}

pub fn switch(name: &str) -> Vec<String> {
    vec!["switch".into(), name.into()]
}

pub fn merge(name: &str) -> Vec<String> {
    vec!["merge".into(), "--no-edit".into(), name.into()]
}

pub fn fetch() -> Vec<String> {
    vec!["fetch".into(), "--prune".into()]
}

pub fn pull() -> Vec<String> {
    vec!["pull".into(), "--ff-only".into()]
}

pub fn push(set_upstream: Option<&str>) -> Vec<String> {
    match set_upstream {
        Some(branch) => vec!["push".into(), "--set-upstream".into(), "origin".into(), branch.into()],
        None => vec!["push".into()],
    }
}

pub fn init() -> Vec<String> {
    vec!["init".into(), "--initial-branch=main".into()]
}

/// Diff of one file: staged (`--cached`) or the working tree.
pub fn diff(path: &str, staged: bool) -> Vec<String> {
    let mut a = vec!["diff".to_owned(), "--no-color".to_owned(), "--no-ext-diff".to_owned()];
    if staged {
        a.push("--cached".into());
    }
    a.extend(["--".into(), path.into()]);
    a
}

/// The file as committed in HEAD (left side of the side-by-side diff).
pub fn show_head(path: &str) -> Vec<String> {
    vec!["show".into(), format!("HEAD:{path}")]
}

/// VS' `.gitignore` for *Create Git Repository*.
pub const VS_GITIGNORE: &str = include_str!("../../anvil-core/templates/gitignore.txt");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines() {
        assert_eq!(stage(&["a b.cs"]), ["add", "--", "a b.cs"]);
        assert_eq!(commit(true, false), ["commit", "-F", "-", "--amend"]);
        let (cur, all) = parse_branches("*main\n feature\n origin/HEAD\n origin/main\n");
        assert_eq!(cur.as_deref(), Some("main"));
        assert_eq!(all, ["main", "feature", "origin/main"]);
        assert!(VS_GITIGNORE.contains("[Bb]in/"));
    }
}
