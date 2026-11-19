//! Line diffs (Myers) for the Git diff view and the AI's edit review, and
//! unified patches: parse, apply per hunk.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Equal,
    Delete,
    Insert,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub op: Op,
    pub text: String,
    /// 0-based line numbers in the old / new text.
    pub old: Option<usize>,
    pub new: Option<usize>,
}

/// The shortest edit script between two line lists.
pub fn lines(a: &[&str], b: &[&str]) -> Vec<Line> {
    let n = a.len();
    let m = b.len();
    let max = n + m;
    let off = max as isize;
    let mut v = vec![0usize; 2 * max + 2];
    let mut trace: Vec<Vec<usize>> = Vec::new();
    'outer: for d in 0..=max as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let idx = (k + off) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) { v[idx + 1] } else { v[idx - 1] + 1 };
            let mut y = (x as isize - k) as usize;
            while x < n && y < m && a[x] == b[y] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                trace.push(v.clone());
                break 'outer;
            }
            k += 2;
        }
    }
    // Backtrack.
    let mut out = Vec::new();
    let (mut x, mut y) = (n as isize, m as isize);
    for d in (0..trace.len() - 1).rev() {
        let v = &trace[d];
        let d = d as isize;
        let k = x - y;
        let prev_k = if k == -d || (k != d && v[(k - 1 + off) as usize] < v[(k + 1 + off) as usize]) { k + 1 } else { k - 1 };
        let prev_x = v[(prev_k + off) as usize] as isize;
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            out.push(Line { op: Op::Equal, text: a[x as usize].to_owned(), old: Some(x as usize), new: Some(y as usize) });
        }
        if d > 0 {
            if x == prev_x {
                y -= 1;
                out.push(Line { op: Op::Insert, text: b[y as usize].to_owned(), old: None, new: Some(y as usize) });
            } else {
                x -= 1;
                out.push(Line { op: Op::Delete, text: a[x as usize].to_owned(), old: Some(x as usize), new: None });
            }
        }
        x = prev_x;
        y = prev_y;
    }
    out.reverse();
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// 1-based start lines and lengths, as in `@@ -a,b +c,d @@`.
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
    pub lines: Vec<(Op, String)>,
}

/// Groups a diff into hunks with `context` lines around each change.
pub fn hunks(diff: &[Line], context: usize) -> Vec<Hunk> {
    let changed: Vec<usize> = diff.iter().enumerate().filter(|(_, l)| l.op != Op::Equal).map(|(i, _)| i).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < changed.len() {
        let start = changed[i].saturating_sub(context);
        let mut end = (changed[i] + context + 1).min(diff.len());
        while i + 1 < changed.len() && changed[i + 1] <= end + context {
            i += 1;
            end = (changed[i] + context + 1).min(diff.len());
        }
        let slice = &diff[start..end];
        let old_start = slice.iter().find_map(|l| l.old).unwrap_or_else(|| diff[..start].iter().rev().find_map(|l| l.old).map_or(0, |o| o + 1)) + 1;
        let new_start = slice.iter().find_map(|l| l.new).unwrap_or_else(|| diff[..start].iter().rev().find_map(|l| l.new).map_or(0, |o| o + 1)) + 1;
        out.push(Hunk {
            old_start,
            old_len: slice.iter().filter(|l| l.op != Op::Insert).count(),
            new_start,
            new_len: slice.iter().filter(|l| l.op != Op::Delete).count(),
            lines: slice.iter().map(|l| (l.op, l.text.clone())).collect(),
        });
        i += 1;
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePatch {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub hunks: Vec<Hunk>,
}

impl FilePatch {
    pub fn path(&self) -> Option<&str> {
        self.new_path.as_deref().or(self.old_path.as_deref())
    }
}

fn strip_prefix(p: &str) -> Option<String> {
    let p = p.split('\t').next().unwrap_or(p).trim();
    if p == "/dev/null" {
        return None;
    }
    Some(p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(p).to_owned())
}

/// Parses a unified diff (git or plain). Lenient: AI models often get the
/// line counts wrong, so lengths come from the lines that follow.
pub fn parse_unified(text: &str) -> Vec<FilePatch> {
    let mut out: Vec<FilePatch> = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(old) = line.strip_prefix("--- ") {
            let Some(new) = lines.peek().and_then(|l| l.strip_prefix("+++ ")) else { continue };
            let new = new.to_owned();
            lines.next();
            out.push(FilePatch { old_path: strip_prefix(old), new_path: strip_prefix(&new), hunks: Vec::new() });
        } else if let Some(h) = line.strip_prefix("@@") {
            let Some(fp) = out.last_mut() else { continue };
            let nums: Vec<usize> = h.split(['-', '+', ',', ' ', '@']).filter_map(|x| x.parse().ok()).collect();
            let mut hunk = Hunk { old_start: *nums.first().unwrap_or(&1), old_len: 0, new_start: *nums.get(2).or(nums.get(1)).unwrap_or(&1), new_len: 0, lines: Vec::new() };
            while let Some(l) = lines.peek() {
                let (op, t) = if let Some(t) = l.strip_prefix('+') {
                    if l.starts_with("+++ ") { break; }
                    (Op::Insert, t)
                } else if let Some(t) = l.strip_prefix('-') {
                    if l.starts_with("--- ") { break; }
                    (Op::Delete, t)
                } else if let Some(t) = l.strip_prefix(' ') {
                    (Op::Equal, t)
                } else if l.is_empty() {
                    (Op::Equal, "")
                } else {
                    break;
                };
                hunk.lines.push((op, t.to_owned()));
                lines.next();
            }
            hunk.old_len = hunk.lines.iter().filter(|(o, _)| *o != Op::Insert).count();
            hunk.new_len = hunk.lines.iter().filter(|(o, _)| *o != Op::Delete).count();
            fp.hunks.push(hunk);
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
pub struct ApplyError(pub usize);

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "hunk {} does not match the file", self.0 + 1)
    }
}

impl std::error::Error for ApplyError {}

/// Applies hunks to `text` (lines without their endings, joined by `\n`).
/// Each hunk is placed where its old lines match, searching outward from
/// its stated line, so a slightly wrong line number still applies.
pub fn apply_hunks(text: &str, hunks: &[Hunk]) -> Result<String, ApplyError> {
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    let mut shift: isize = 0;
    for (i, h) in hunks.iter().enumerate() {
        let old: Vec<&str> = h.lines.iter().filter(|(o, _)| *o != Op::Insert).map(|(_, t)| t.as_str()).collect();
        let new: Vec<String> = h.lines.iter().filter(|(o, _)| *o != Op::Delete).map(|(_, t)| t.clone()).collect();
        let guess = (h.old_start as isize - 1 + shift).max(0) as usize;
        let fits = |at: usize| at + old.len() <= lines.len() && lines[at..at + old.len()].iter().zip(&old).all(|(a, b)| a.trim_end() == b.trim_end());
        let at = (0..=lines.len()).flat_map(|d| [guess.checked_add(d), guess.checked_sub(d)]).flatten().find(|&at| fits(at)).ok_or(ApplyError(i))?;
        lines.splice(at..at + old.len(), new.iter().cloned());
        shift += new.len() as isize - old.len() as isize;
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn myers_basic() {
        let a = ["a", "b", "c", "d"];
        let b = ["a", "x", "c", "d", "e"];
        let d = lines(&a, &b);
        let ops: Vec<(Op, &str)> = d.iter().map(|l| (l.op, l.text.as_str())).collect();
        assert_eq!(ops, [(Op::Equal, "a"), (Op::Delete, "b"), (Op::Insert, "x"), (Op::Equal, "c"), (Op::Equal, "d"), (Op::Insert, "e")]);
        assert!(lines(&[], &[]).is_empty());
        assert_eq!(lines(&["a"], &[]).len(), 1);
    }

    #[test]
    fn hunks_and_round_trip() {
        let a: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
        let mut b = a.clone();
        b[3] = "changed".into();
        b.insert(15, "new".into());
        let ar: Vec<&str> = a.iter().map(String::as_str).collect();
        let br: Vec<&str> = b.iter().map(String::as_str).collect();
        let hs = hunks(&lines(&ar, &br), 3);
        assert_eq!(hs.len(), 2);
        assert_eq!((hs[0].old_start, hs[0].old_len, hs[0].new_start, hs[0].new_len), (1, 7, 1, 7));
        assert_eq!(apply_hunks(&a.join("\n"), &hs).unwrap(), b.join("\n"));
    }

    #[test]
    fn parses_and_applies_git_patch_with_wrong_line_numbers() {
        let patch = "diff --git a/A.cs b/A.cs\n--- a/A.cs\n+++ b/A.cs\n@@ -10,3 +10,3 @@\n class A\n {\n-    int x;\n+    int y;\n";
        let fps = parse_unified(patch);
        assert_eq!(fps[0].path(), Some("A.cs"));
        assert_eq!(fps[0].hunks[0].old_len, 3);
        let src = "namespace N\n{\nclass A\n{\n    int x;\n}\n}";
        assert_eq!(apply_hunks(src, &fps[0].hunks).unwrap(), "namespace N\n{\nclass A\n{\n    int y;\n}\n}");
        assert_eq!(apply_hunks("nothing", &fps[0].hunks), Err(ApplyError(0)));
    }

    #[test]
    fn new_file_patch() {
        let fps = parse_unified("--- /dev/null\n+++ b/B.cs\n@@ -0,0 +1,2 @@\n+class B\n+{}\n");
        assert_eq!(fps[0].old_path, None);
        assert_eq!(apply_hunks("", &fps[0].hunks).unwrap(), "class B\n{}\n");
    }
}
