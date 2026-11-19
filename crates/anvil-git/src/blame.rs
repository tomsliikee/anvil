//! `git blame --porcelain` for the editor gutter (off by default).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    /// 1-based line in the current file.
    pub line: u32,
    pub hash: String,
    pub author: String,
    pub summary: String,
}

pub fn args(file: &str) -> Vec<String> {
    vec!["blame".into(), "--porcelain".into(), "--".into(), file.into()]
}

pub fn parse(out: &str) -> Vec<BlameLine> {
    let mut info: std::collections::HashMap<String, (String, String)> = Default::default();
    let mut out_lines = Vec::new();
    let mut current: Option<(String, u32)> = None;
    for l in out.lines() {
        if l.starts_with('\t') {
            if let Some((h, n)) = current.take() {
                let (a, s) = info.get(&h).cloned().unwrap_or_default();
                out_lines.push(BlameLine { line: n, hash: h, author: a, summary: s });
            }
            continue;
        }
        let mut parts = l.split(' ');
        let first = parts.next().unwrap_or("");
        if first.len() == 40 && first.chars().all(|c| c.is_ascii_hexdigit()) {
            let _orig = parts.next();
            let fin: u32 = parts.next().and_then(|x| x.parse().ok()).unwrap_or(0);
            current = Some((first.to_owned(), fin));
            info.entry(first.to_owned()).or_default();
        } else if let Some((h, _)) = &current {
            if let Some(a) = l.strip_prefix("author ") {
                info.entry(h.clone()).or_default().0 = a.to_owned();
            } else if let Some(s) = l.strip_prefix("summary ") {
                info.entry(h.clone()).or_default().1 = s.to_owned();
            }
        }
    }
    out_lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain() {
        let h = "a".repeat(40);
        let out = format!("{h} 1 1 2\nauthor Thomas\nsummary Init\nfilename A.cs\n\tline one\n{h} 2 2\n\tline two\n");
        let b = parse(&out);
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].line, 2);
        assert_eq!(b[1].author, "Thomas");
        assert_eq!(b[0].summary, "Init");
    }
}
