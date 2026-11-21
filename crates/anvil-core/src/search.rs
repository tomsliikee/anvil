//! Fuzzy matching and most-recently-used ranking for Feature Search
//! (`Ctrl+Q`) and Go To All (`Ctrl+T`): `fmtdoc` finds "Format Document",
//! `gtd` finds "Go To Definition", and what was picked last comes first.

use std::collections::BTreeMap;

/// Where a query matched: the score (higher is better) and the matched
/// positions as char indices into the text, for highlighting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    pub positions: Vec<usize>,
}

const MATCH: i32 = 16;
const WORD_START: i32 = 24;
const FIRST_CHAR: i32 = 16;
const CONSECUTIVE: i32 = 20;
const GAP: i32 = 2;
const LEADING: i32 = 1;

fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '.' | '-' | '_' | '/' | '\\' | '(' | ')' | ',' | ':' | '·' | '…' | '<' | '>')
}

/// A word starts after a separator, at an upper-case letter after a
/// lower-case one (`GoToAll`), and at a digit after a letter.
fn word_starts(chars: &[char]) -> Vec<bool> {
    (0..chars.len())
        .map(|i| {
            if i == 0 {
                return true;
            }
            let (p, c) = (chars[i - 1], chars[i]);
            is_separator(p) && !is_separator(c) || p.is_lowercase() && c.is_uppercase() || p.is_alphabetic() && c.is_ascii_digit()
        })
        .collect()
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Matches `query` against `text` as a subsequence, case-insensitively.
/// Spaces in the query are ignored, so "format doc" and "formatdoc" find
/// the same. Word starts, runs of consecutive characters and an early
/// first match score higher; gaps cost a little.
pub fn fuzzy(query: &str, text: &str) -> Option<Match> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).map(fold).collect();
    if q.is_empty() {
        return Some(Match { score: 0, positions: Vec::new() });
    }
    let t: Vec<char> = text.chars().collect();
    let lower: Vec<char> = t.iter().copied().map(fold).collect();
    // Cheap rejection before the table: most items do not match at all.
    let mut k = 0;
    for c in &lower {
        if k < q.len() && *c == q[k] {
            k += 1;
        }
    }
    if k < q.len() {
        return None;
    }
    let (n, m) = (q.len(), t.len());
    let starts = word_starts(&t);
    let bonus = |p: usize| MATCH + if starts[p] { WORD_START } else { 0 } + if p == 0 { FIRST_CHAR } else { 0 };
    // best[i][p]: the best score for the first i+1 query characters with
    // the last one at p; from[i][p] the position of the one before.
    let mut best = vec![vec![i32::MIN; m]; n];
    let mut from = vec![vec![usize::MAX; m]; n];
    for p in 0..m {
        if lower[p] == q[0] {
            best[0][p] = bonus(p) - LEADING * p.min(12) as i32;
        }
    }
    for i in 1..n {
        // The best predecessor at least two positions back, as
        // score + GAP * position, so the gap cost is subtracted once.
        let mut run: Option<(i32, usize)> = None;
        for p in i..m {
            if p >= 2 {
                let k = p - 2;
                if best[i - 1][k] != i32::MIN {
                    let v = best[i - 1][k] + GAP * k as i32;
                    if run.is_none_or(|(rv, _)| v > rv) {
                        run = Some((v, k));
                    }
                }
            }
            if lower[p] != q[i] {
                continue;
            }
            let mut cand: Option<(i32, usize)> = None;
            if best[i - 1][p - 1] != i32::MIN {
                cand = Some((best[i - 1][p - 1] + CONSECUTIVE, p - 1));
            }
            if let Some((rv, k)) = run {
                let gap = (p - k - 1) as i32;
                let v = rv - GAP * k as i32 - GAP * gap;
                if cand.is_none_or(|(c, _)| v > c) {
                    cand = Some((v, k));
                }
            }
            if let Some((v, k)) = cand {
                best[i][p] = v + bonus(p);
                from[i][p] = k;
            }
        }
    }
    let (mut p, score) = (0..m).filter(|&p| best[n - 1][p] != i32::MIN).map(|p| (p, best[n - 1][p])).max_by_key(|&(p, s)| (s, std::cmp::Reverse(p)))?;
    let mut positions = vec![0; n];
    for i in (0..n).rev() {
        positions[i] = p;
        p = from[i][p];
    }
    // Shorter texts win ties: "Save" before "Save All" for "save".
    Some(Match { score: score - (m as i32 / 8), positions })
}

/// Picks, most recent first, kept between sessions as JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mru {
    keys: Vec<String>,
}

const MRU_LEN: usize = 60;

impl Mru {
    pub fn from_json(json: &str) -> Mru {
        let parsed: BTreeMap<String, Vec<String>> = serde_json::from_str(json).unwrap_or_default();
        let mut keys = parsed.get("recent").cloned().unwrap_or_default();
        keys.truncate(MRU_LEN);
        Mru { keys }
    }

    pub fn to_json(&self) -> String {
        let mut m = BTreeMap::new();
        m.insert("recent", &self.keys);
        serde_json::to_string_pretty(&m).expect("serialises")
    }

    pub fn push(&mut self, key: &str) {
        self.keys.retain(|k| k != key);
        self.keys.insert(0, key.to_owned());
        self.keys.truncate(MRU_LEN);
    }

    /// 0 for the latest pick.
    pub fn rank(&self, key: &str) -> Option<usize> {
        self.keys.iter().position(|k| k == key)
    }

    pub fn keys(&self) -> &[String] {
        &self.keys
    }
}

/// One searchable thing: the text shown (and matched), an optional second
/// text matched with less weight (VS' command name, a folder), and the
/// key under which picks are remembered.
pub struct Candidate<'a> {
    pub text: &'a str,
    pub alt: Option<&'a str>,
    pub key: &'a str,
    /// Added to the score: lets commands come before files at equal
    /// matches.
    pub weight: i32,
}

/// What matched where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ranked {
    pub index: usize,
    pub score: i32,
    /// Positions in `text`; empty when only `alt` matched.
    pub text_positions: Vec<usize>,
    pub alt_positions: Vec<usize>,
}

const ALT_PENALTY: i32 = 30;

/// The candidates that match `query`, best first, at most `limit`. Recent
/// picks get a boost that fades with age; with an empty query they come
/// first in the order they were picked, then the rest in their own order.
pub fn rank(candidates: &[Candidate], query: &str, mru: &Mru, limit: usize) -> Vec<Ranked> {
    let boost = |key: &str| mru.rank(key).map_or(0, |r| 60 - (r.min(30) as i32) * 2);
    let mut out: Vec<Ranked> = Vec::new();
    if query.trim().is_empty() {
        let mut recent: Vec<(usize, usize)> = candidates.iter().enumerate().filter_map(|(i, c)| mru.rank(c.key).map(|r| (r, i))).collect();
        recent.sort_unstable();
        out.extend(recent.iter().map(|&(_, i)| Ranked { index: i, score: 0, text_positions: Vec::new(), alt_positions: Vec::new() }));
        out.extend(candidates.iter().enumerate().filter(|(_, c)| mru.rank(c.key).is_none()).map(|(i, _)| Ranked { index: i, score: 0, text_positions: Vec::new(), alt_positions: Vec::new() }));
        out.truncate(limit);
        return out;
    }
    for (i, c) in candidates.iter().enumerate() {
        let main = fuzzy(query, c.text);
        let alt = c.alt.and_then(|a| fuzzy(query, a)).map(|m| Match { score: m.score - ALT_PENALTY, positions: m.positions });
        let r = match (main, alt) {
            (Some(a), Some(b)) if b.score > a.score => Ranked { index: i, score: b.score, text_positions: Vec::new(), alt_positions: b.positions },
            (Some(a), _) => Ranked { index: i, score: a.score, text_positions: a.positions, alt_positions: Vec::new() },
            (None, Some(b)) => Ranked { index: i, score: b.score, text_positions: Vec::new(), alt_positions: b.positions },
            (None, None) => continue,
        };
        out.push(Ranked { score: r.score + c.weight + boost(c.key), ..r });
    }
    out.sort_by_key(|r| (std::cmp::Reverse(r.score), r.index));
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand<'a>(text: &'a str, key: &'a str) -> Candidate<'a> {
        Candidate { text, alt: None, key, weight: 0 }
    }

    #[test]
    fn subsequence_case_insensitive() {
        assert!(fuzzy("fmtdoc", "Format Document").is_some());
        assert!(fuzzy("FORMAT", "Format Document").is_some());
        assert!(fuzzy("xyz", "Format Document").is_none());
        assert_eq!(fuzzy("", "anything").unwrap().positions, Vec::<usize>::new());
    }

    #[test]
    fn positions_prefer_word_starts() {
        // "fd" should light the F of Format and the D of Document, not the
        // "d" inside nothing else.
        assert_eq!(fuzzy("fd", "Format Document").unwrap().positions, [0, 7]);
        assert_eq!(fuzzy("gtd", "Go To Definition").unwrap().positions, [0, 3, 6]);
        assert_eq!(fuzzy("gta", "GoToAll").unwrap().positions, [0, 2, 4]);
    }

    #[test]
    fn spaces_in_the_query_are_ignored() {
        assert_eq!(fuzzy("format doc", "Format Document").unwrap().positions, fuzzy("formatdoc", "Format Document").unwrap().positions);
    }

    #[test]
    fn consecutive_and_prefix_beat_scattered() {
        let prefix = fuzzy("save", "Save All").unwrap().score;
        let scattered = fuzzy("save", "Show Active View Error").unwrap().score;
        assert!(prefix > scattered, "{prefix} {scattered}");
        assert!(fuzzy("save", "Save").unwrap().score > fuzzy("save", "Save All").unwrap().score);
    }

    #[test]
    fn umlauts_match() {
        assert_eq!(fuzzy("lö", "Zeile löschen").unwrap().positions, [6, 7]);
    }

    #[test]
    fn ranking_orders_by_score_and_mru_boost() {
        let c = [cand("Build Solution", "build"), cand("Rebuild Solution", "rebuild"), cand("Open Folder", "open")];
        let none = Mru::default();
        let r = rank(&c, "build", &none, 10);
        assert_eq!(r.iter().map(|r| r.index).collect::<Vec<_>>(), [0, 1]);
        let mut mru = Mru::default();
        mru.push("rebuild");
        let r = rank(&c, "build", &mru, 10);
        assert_eq!(r[0].index, 1, "a recent pick comes first");
    }

    #[test]
    fn empty_query_lists_recent_first() {
        let c = [cand("A", "a"), cand("B", "b"), cand("C", "c")];
        let mut mru = Mru::default();
        mru.push("c");
        mru.push("b");
        let r = rank(&c, "", &mru, 10);
        assert_eq!(r.iter().map(|r| r.index).collect::<Vec<_>>(), [1, 2, 0]);
        assert_eq!(rank(&c, "", &mru, 2).len(), 2);
    }

    #[test]
    fn alt_text_matches_with_less_weight() {
        let c = [Candidate { text: "Feature Search", alt: Some("Window.QuickLaunch"), key: "q", weight: 0 }, cand("Quick Info", "i")];
        let r = rank(&c, "quick", &Mru::default(), 10);
        assert_eq!(r[0].index, 1);
        assert_eq!(r[1].index, 0);
        assert!(r[1].text_positions.is_empty() && !r[1].alt_positions.is_empty());
    }

    #[test]
    fn mru_round_trips_and_dedups() {
        let mut m = Mru::default();
        m.push("a");
        m.push("b");
        m.push("a");
        assert_eq!(m.keys(), ["a", "b"]);
        assert_eq!(Mru::from_json(&m.to_json()), m);
        assert_eq!(Mru::from_json("not json"), Mru::default());
    }
}
