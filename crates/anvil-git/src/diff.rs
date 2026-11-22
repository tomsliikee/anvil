//! Unified diffs of one file (`git diff`), split into hunks, and the patch
//! of a single hunk for `git apply --cached` (stage or unstage a hunk).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file`
    NoNewline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The `@@ -a,b +c,d @@ …` line.
    pub header: String,
    /// First line of the hunk in the new file (1-based).
    pub new_start: u32,
    pub lines: Vec<(LineKind, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    /// Everything before the first hunk (`diff --git`, `index`, `---`, `+++`).
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
    /// A binary file: git prints no hunks.
    pub binary: bool,
}

fn new_start(header: &str) -> u32 {
    // "@@ -1,4 +1,5 @@ ctx": the number after '+'.
    header.split(' ').find_map(|p| p.strip_prefix('+')).and_then(|p| p.split(',').next()).and_then(|n| n.parse().ok()).unwrap_or(1)
}

pub fn parse(out: &str) -> FileDiff {
    let mut d = FileDiff::default();
    for l in out.split('\n') {
        if l.starts_with("@@") {
            d.hunks.push(Hunk { header: l.to_owned(), new_start: new_start(l), lines: Vec::new() });
            continue;
        }
        let Some(h) = d.hunks.last_mut() else {
            if l.starts_with("Binary files ") {
                d.binary = true;
            }
            if !l.is_empty() {
                d.header.push(l.to_owned());
            }
            continue;
        };
        let kind = match l.as_bytes().first() {
            Some(b'+') => LineKind::Added,
            Some(b'-') => LineKind::Removed,
            Some(b' ') => LineKind::Context,
            Some(b'\\') => LineKind::NoNewline,
            // The trailing empty piece after the last newline.
            _ => continue,
        };
        h.lines.push((kind, l.get(1..).unwrap_or("").to_owned()));
    }
    d
}

/// The patch holding only hunk `i`, for `git apply --cached [--reverse] -`.
/// The hunk header's counts stay as git wrote them, which is right because
/// the hunk is taken whole.
pub fn hunk_patch(d: &FileDiff, i: usize) -> Option<String> {
    let h = d.hunks.get(i)?;
    let mut s = String::new();
    for l in &d.header {
        s.push_str(l);
        s.push('\n');
    }
    s.push_str(&h.header);
    s.push('\n');
    for (k, t) in &h.lines {
        let c = match k {
            LineKind::Context => ' ',
            LineKind::Added => '+',
            LineKind::Removed => '-',
            LineKind::NoNewline => '\\',
        };
        s.push(c);
        s.push_str(t);
        s.push('\n');
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/A.cs b/A.cs\nindex 1111111..2222222 100644\n--- a/A.cs\n+++ b/A.cs\n@@ -1,3 +1,3 @@ class A\n one\n-two\n+zwei\n three\n@@ -10,2 +10,3 @@\n ten\n eleven\n+twelve\n\\ No newline at end of file\n";

    #[test]
    fn splits_hunks() {
        let d = parse(DIFF);
        assert_eq!(d.header.len(), 4);
        assert_eq!(d.hunks.len(), 2);
        assert_eq!(d.hunks[0].new_start, 1);
        assert_eq!(d.hunks[1].new_start, 10);
        assert_eq!(d.hunks[0].lines[1], (LineKind::Removed, "two".to_owned()));
        assert_eq!(d.hunks[1].lines.last().unwrap().0, LineKind::NoNewline);
        assert!(!d.binary);
    }

    #[test]
    fn one_hunk_patch_keeps_header() {
        let d = parse(DIFF);
        let p = hunk_patch(&d, 1).unwrap();
        assert!(p.starts_with("diff --git a/A.cs b/A.cs\n"));
        assert!(p.contains("@@ -10,2 +10,3 @@\n ten\n eleven\n+twelve\n\\ No newline at end of file\n"));
        assert!(!p.contains("zwei"));
        assert_eq!(hunk_patch(&d, 2), None);
    }

    #[test]
    fn binary_has_no_hunks() {
        let d = parse("diff --git a/x.png b/x.png\nindex 1..2 100644\nBinary files a/x.png and b/x.png differ\n");
        assert!(d.binary);
        assert!(d.hunks.is_empty());
    }
}
