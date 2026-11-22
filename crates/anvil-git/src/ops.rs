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

/// Stage (`cached`) or unstage (`cached` + `reverse`) one hunk, or undo it
/// in the working tree (`reverse` only); the patch comes on stdin.
pub fn apply_patch(cached: bool, reverse: bool) -> Vec<String> {
    let mut a = vec!["apply".to_owned(), "--whitespace=nowarn".to_owned()];
    if cached {
        a.push("--cached".into());
    }
    if reverse {
        a.push("--reverse".into());
    }
    a.push("-".into());
    a
}

/// The last commit's full message (to prefill Amend).
pub fn last_message() -> Vec<String> {
    vec!["log".into(), "-1".into(), "--format=%B".into()]
}

/// Commands that resolve a conflict by taking one side whole (VS: *Take
/// Current* = ours, *Take Incoming* = theirs). When the chosen side deleted
/// the file, resolving means removing it.
pub fn take_side(path: &str, conflict: &str, ours: bool) -> Vec<Vec<String>> {
    let (us, them) = (conflict.as_bytes().first().copied(), conflict.as_bytes().get(1).copied());
    let deleted = if ours { us == Some(b'D') } else { them == Some(b'D') };
    if deleted {
        return vec![vec!["rm".into(), "--quiet".into(), "--".into(), path.into()]];
    }
    vec![vec!["checkout".into(), if ours { "--ours" } else { "--theirs" }.into(), "--".into(), path.into()], stage(&[path])]
}

pub fn merge_abort() -> Vec<String> {
    vec!["merge".into(), "--abort".into()]
}

/// Whether a failed fetch/pull/push wanted credentials it did not get
/// (no helper, no stored password, or a wrong one).
pub fn needs_credentials(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    ["could not read username", "could not read password", "authentication failed", "terminal prompts disabled", "invalid username or password", "http basic: access denied"].iter().any(|m| s.contains(m))
}

/// `GIT_ASKPASS` program: git calls it with the prompt ("Username for
/// 'https://…': ") and reads the answer from stdout. It answers from the
/// file `ANVIL_ASKPASS_FILE` names (user on the first line, password on the
/// second), which Anvil writes with mode 0600 only after the user typed
/// them, and fails otherwise, so git reports that it needs credentials. A
/// successful command hands them to the user's credential helper, if any.
pub const ASKPASS_SCRIPT: &str = "#!/bin/sh\nf=\"$ANVIL_ASKPASS_FILE\"\n[ -n \"$f\" ] && [ -r \"$f\" ] || exit 1\ncase \"$1\" in\n  Username*) sed -n 1p \"$f\" ;;\n  *) sed -n 2p \"$f\" ;;\nesac\n";

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

    #[test]
    fn conflicts_and_credentials() {
        assert_eq!(take_side("A.cs", "UU", true), [vec!["checkout", "--ours", "--", "A.cs"], vec!["add", "--", "A.cs"]]);
        assert_eq!(take_side("A.cs", "UD", false), [vec!["rm", "--quiet", "--", "A.cs"]]);
        assert_eq!(take_side("A.cs", "UD", true)[0][1], "--ours");
        assert_eq!(apply_patch(true, true), ["apply", "--whitespace=nowarn", "--cached", "--reverse", "-"]);
        assert!(needs_credentials("fatal: could not read Username for 'https://example.test': terminal prompts disabled"));
        assert!(needs_credentials("remote: Invalid username or password.\nfatal: Authentication failed for 'https://x/'"));
        assert!(!needs_credentials("fatal: 'origin' does not appear to be a git repository"));
    }
}
