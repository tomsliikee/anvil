//! A `.gitignore` matcher for the hand-in ZIP: `*`, `?`, `**`, character
//! classes, `!` negation, a trailing `/` for folders and a leading or inner
//! `/` that anchors the pattern to the folder of the `.gitignore`.

#[derive(Clone, Debug)]
struct Rule {
    pattern: Vec<String>,
    negate: bool,
    dir_only: bool,
    anchored: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Ignore {
    rules: Vec<Rule>,
}

impl Ignore {
    pub fn from_gitignore(text: &str) -> Ignore {
        let mut rules = Vec::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            // Trailing spaces are ignored unless escaped.
            let line = if line.ends_with("\\ ") { line } else { line.trim_end() };
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (negate, line) = match line.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, line.strip_prefix('\\').filter(|r| r.starts_with('#') || r.starts_with('!')).unwrap_or(line)),
            };
            let (dir_only, line) = match line.strip_suffix('/') {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            // Any slash (also in `**/x`) makes git match the whole path.
            let anchored = line.contains('/');
            let line = line.trim_start_matches('/');
            rules.push(Rule { pattern: line.split('/').map(str::to_owned).collect(), negate, dir_only, anchored });
        }
        Ignore { rules }
    }

    /// Whether `rel` (relative to the `.gitignore`'s folder, `/`-separated)
    /// is ignored. The last matching rule wins; callers check the parents
    /// first, because git does not look into an ignored folder.
    pub fn is_ignored(&self, rel: &str, is_dir: bool) -> bool {
        let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
        let mut ignored = false;
        for r in &self.rules {
            if r.dir_only && !is_dir {
                continue;
            }
            let hit = if r.anchored {
                match_parts(&r.pattern, &parts)
            } else {
                // An unanchored pattern without a slash matches the name at
                // any depth.
                parts.last().is_some_and(|name| glob(&r.pattern[0], name))
            };
            if hit {
                ignored = !r.negate;
            }
        }
        ignored
    }
}

fn match_parts(pat: &[String], parts: &[&str]) -> bool {
    match (pat.first(), parts.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(p), _) if p == "**" => (0..=parts.len()).any(|i| match_parts(&pat[1..], &parts[i..])),
        (Some(_), None) => false,
        (Some(p), Some(s)) => glob(p, s) && match_parts(&pat[1..], &parts[1..]),
    }
}

/// Glob for one path segment: `*`, `?`, `[a-z]`, `[!x]`, `\` escapes.
pub fn glob(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = name.chars().collect();
    glob_at(&p, &s)
}

fn glob_at(p: &[char], s: &[char]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some('*') => (0..=s.len()).any(|i| glob_at(&p[1..], &s[i..])),
        Some('?') => !s.is_empty() && glob_at(&p[1..], &s[1..]),
        Some('[') => {
            let Some(close) = p.iter().skip(1).position(|&c| c == ']').map(|i| i + 1) else {
                return s.first() == Some(&'[') && glob_at(&p[1..], &s[1..]);
            };
            let Some(&c) = s.first() else { return false };
            let class = &p[1..close];
            let (neg, class) = match class.first() {
                Some('!' | '^') => (true, &class[1..]),
                _ => (false, class),
            };
            let mut hit = false;
            let mut i = 0;
            while i < class.len() {
                if i + 2 < class.len() && class[i + 1] == '-' {
                    hit |= class[i] <= c && c <= class[i + 2];
                    i += 3;
                } else {
                    hit |= class[i] == c;
                    i += 1;
                }
            }
            hit != neg && glob_at(&p[close + 1..], &s[1..])
        }
        Some('\\') if p.len() > 1 => s.first() == Some(&p[1]) && glob_at(&p[2..], &s[1..]),
        Some(&c) => s.first() == Some(&c) && glob_at(&p[1..], &s[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_patterns() {
        let i = Ignore::from_gitignore("# comment\n*.user\n[Bb]in/\n/build\nlogs/**/*.log\n!keep.user\n");
        assert!(i.is_ignored("a/b/x.user", false));
        assert!(!i.is_ignored("a/keep.user", false));
        assert!(i.is_ignored("Proj/bin", true));
        assert!(!i.is_ignored("Proj/bin", false));
        assert!(i.is_ignored("build", true));
        assert!(!i.is_ignored("src/build", true));
        assert!(i.is_ignored("logs/a/b/c.log", false));
        assert!(i.is_ignored("logs/c.log", false));
    }

    #[test]
    fn honours_gitignore_negation() {
        let i = Ignore::from_gitignore("*.txt\n!important.txt\n");
        assert!(i.is_ignored("notes.txt", false));
        assert!(!i.is_ignored("important.txt", false));
    }

    #[test]
    fn vs_gitignore_ignores_bin_obj_vs() {
        let i = Ignore::from_gitignore(include_str!("../templates/gitignore.txt"));
        assert!(i.is_ignored("App/bin", true));
        assert!(i.is_ignored("App/obj", true));
        assert!(i.is_ignored(".vs", true));
        assert!(!i.is_ignored("App/Program.cs", false));
        assert!(!i.is_ignored("App/App.csproj", false));
        assert!(!i.is_ignored("App/ClassDiagram1.cd", false));
    }
}
