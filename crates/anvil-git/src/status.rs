//! `git status --porcelain=v2 --branch -z`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Untracked,
}

impl Change {
    fn from(c: u8) -> Change {
        match c {
            b'M' => Change::Modified,
            b'A' => Change::Added,
            b'D' => Change::Deleted,
            b'R' => Change::Renamed,
            b'C' => Change::Copied,
            b'T' => Change::TypeChanged,
            b'U' => Change::Unmerged,
            _ => Change::Unmodified,
        }
    }

    /// VS' one-letter mark in Git Changes.
    pub fn letter(self) -> &'static str {
        match self {
            Change::Modified | Change::TypeChanged => "M",
            Change::Added | Change::Untracked => "A",
            Change::Deleted => "D",
            Change::Renamed | Change::Copied => "R",
            Change::Unmerged => "C",
            Change::Unmodified => "",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub orig_path: Option<String>,
    pub staged: Change,
    pub unstaged: Change,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub entries: Vec<Entry>,
}

impl Status {
    pub fn staged(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.staged != Change::Unmodified)
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.unstaged != Change::Unmodified)
    }
}

pub fn args() -> Vec<String> {
    ["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"].map(String::from).to_vec()
}

pub fn parse(out: &[u8]) -> Status {
    let mut s = Status::default();
    let records: Vec<&[u8]> = out.split(|&b| b == 0).collect();
    let mut i = 0;
    while i < records.len() {
        let r = String::from_utf8_lossy(records[i]).into_owned();
        i += 1;
        if let Some(h) = r.strip_prefix("# ") {
            if let Some(b) = h.strip_prefix("branch.head ") {
                s.branch = (b != "(detached)").then(|| b.to_owned());
            } else if let Some(u) = h.strip_prefix("branch.upstream ") {
                s.upstream = Some(u.to_owned());
            } else if let Some(ab) = h.strip_prefix("branch.ab ") {
                for part in ab.split(' ') {
                    if let Some(n) = part.strip_prefix('+') {
                        s.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix('-') {
                        s.behind = n.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        let xy = |f: &str| {
            let b = f.as_bytes();
            (Change::from(*b.first().unwrap_or(&b'.')), Change::from(*b.get(1).unwrap_or(&b'.')))
        };
        if let Some(rest) = r.strip_prefix("1 ") {
            let f: Vec<&str> = rest.splitn(8, ' ').collect();
            if f.len() == 8 {
                let (x, y) = xy(f[0]);
                s.entries.push(Entry { path: f[7].to_owned(), orig_path: None, staged: x, unstaged: y });
            }
        } else if let Some(rest) = r.strip_prefix("2 ") {
            let f: Vec<&str> = rest.splitn(9, ' ').collect();
            if f.len() == 9 {
                let (x, y) = xy(f[0]);
                let orig = records.get(i).map(|o| String::from_utf8_lossy(o).into_owned());
                i += 1;
                s.entries.push(Entry { path: f[8].to_owned(), orig_path: orig, staged: x, unstaged: y });
            }
        } else if let Some(rest) = r.strip_prefix("u ") {
            let f: Vec<&str> = rest.splitn(10, ' ').collect();
            if let Some(p) = f.last() {
                s.entries.push(Entry { path: (*p).to_owned(), orig_path: None, staged: Change::Unmerged, unstaged: Change::Unmerged });
            }
        } else if let Some(p) = r.strip_prefix("? ") {
            s.entries.push(Entry { path: p.to_owned(), orig_path: None, staged: Change::Unmodified, unstaged: Change::Untracked });
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let records = ["# branch.oid abc", "# branch.head main", "# branch.upstream origin/main", "# branch.ab +2 -1", "1 .M N... 100644 100644 100644 a b Program.cs", "1 A. N... 000000 100644 100644 0 b New File.cs", "2 R. N... 100644 100644 100644 a b R100 Renamed.cs", "Old.cs", "? notes.txt", ""];
        let joined = records.join("\0");
        let out = joined.as_bytes();
        let s = parse(out);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        assert_eq!(s.entries.len(), 4);
        assert_eq!(s.entries[0].unstaged, Change::Modified);
        assert_eq!(s.entries[1].path, "New File.cs");
        assert_eq!(s.entries[2].orig_path.as_deref(), Some("Old.cs"));
        assert_eq!(s.entries[3].unstaged, Change::Untracked);
        assert_eq!(s.staged().count(), 2);
        assert_eq!(s.unstaged().count(), 2);
    }
}
