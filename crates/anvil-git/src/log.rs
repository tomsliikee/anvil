//! History (`git log`) with fields separated by control characters, so
//! commit messages cannot break the parsing.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub author: String,
    pub date: String,
    pub subject: String,
    pub refs: String,
}

const FMT: &str = "--pretty=format:%H%x1f%an%x1f%ad%x1f%s%x1f%D%x1e";

pub fn args(file: Option<&str>, max: usize) -> Vec<String> {
    let mut a = vec!["log".to_owned(), FMT.to_owned(), "--date=format:%Y-%m-%d %H:%M".to_owned(), format!("-n{max}")];
    if let Some(f) = file {
        a.extend(["--follow".to_owned(), "--".to_owned(), f.to_owned()]);
    }
    a
}

pub fn parse(out: &str) -> Vec<Commit> {
    out.split('\u{1e}')
        .filter_map(|rec| {
            let f: Vec<&str> = rec.trim_start_matches('\n').split('\u{1f}').collect();
            (f.len() >= 5).then(|| Commit { hash: f[0].to_owned(), author: f[1].to_owned(), date: f[2].to_owned(), subject: f[3].to_owned(), refs: f[4].to_owned() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_records() {
        let out = "abc\u{1f}Thomas\u{1f}2026-11-18 10:00\u{1f}Add list; fix | bug\u{1f}HEAD -> main\u{1e}\ndef\u{1f}T\u{1f}2026-11-17 09:00\u{1f}Init\u{1f}\u{1e}";
        let c = parse(out);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].subject, "Add list; fix | bug");
        assert_eq!(c[1].hash, "def");
        assert!(args(Some("A.cs"), 50).contains(&"--follow".to_owned()));
    }
}
