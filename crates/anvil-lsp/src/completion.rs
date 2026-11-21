//! Filtering and ordering of a completion list on the client, so typing
//! after the list opened narrows it without asking the server again, and
//! the best match is always the first (selected) row, as in VS.

use lsp_types::CompletionItem;

/// How well `word` matches `name`; lower is better, `None` drops the item.
/// Exact-case prefix, any-case prefix, camel humps (`SB` for
/// `StringBuilder`), then any-case substring.
pub fn score(name: &str, word: &str) -> Option<u8> {
    if word.is_empty() {
        return Some(0);
    }
    if name.starts_with(word) {
        return Some(0);
    }
    let lower = name.to_lowercase();
    let wl = word.to_lowercase();
    if lower.starts_with(&wl) {
        return Some(1);
    }
    if humps_match(name, word) {
        return Some(2);
    }
    lower.contains(&wl).then_some(3)
}

/// `word`'s letters are, in order, the starts of `name`'s humps (capitals
/// and letters after `_`), each followed by any of that hump's letters.
fn humps_match(name: &str, word: &str) -> bool {
    let humps: Vec<usize> = name
        .char_indices()
        .filter(|(i, c)| *i == 0 || c.is_uppercase() || name[..*i].ends_with('_'))
        .map(|(i, _)| i)
        .collect();
    let mut w = word.chars().peekable();
    for (n, &start) in humps.iter().enumerate() {
        let end = humps.get(n + 1).copied().unwrap_or(name.len());
        let mut hump = name[start..end].chars();
        // The hump's first letter must match the next word letter.
        match (hump.next(), w.peek()) {
            (Some(h), Some(c)) if h.to_lowercase().eq(c.to_lowercase()) => {
                w.next();
            }
            _ => continue,
        }
        // Then as many of the hump's following letters as the word has.
        for h in hump {
            match w.peek() {
                Some(c) if h.to_lowercase().eq(c.to_lowercase()) => {
                    w.next();
                }
                _ => break,
            }
        }
        if w.peek().is_none() {
            return true;
        }
    }
    w.peek().is_none()
}

fn filter_text(i: &CompletionItem) -> &str {
    i.filter_text.as_deref().unwrap_or(&i.label)
}

/// Indices of the items that match `word`, best first: match quality,
/// then the server's preselection, then its sort text.
pub fn rank(items: &[CompletionItem], word: &str) -> Vec<usize> {
    let mut keyed: Vec<(u8, bool, &str, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(n, i)| score(filter_text(i), word).map(|s| (s, i.preselect != Some(true), i.sort_text.as_deref().unwrap_or(&i.label), n)))
        .collect();
    keyed.sort();
    keyed.into_iter().map(|(_, _, _, n)| n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str) -> CompletionItem {
        CompletionItem { label: label.into(), ..Default::default() }
    }

    #[test]
    fn prefix_beats_humps_beats_substring() {
        assert_eq!(score("WriteLine", "Wri"), Some(0));
        assert_eq!(score("WriteLine", "wri"), Some(1));
        assert_eq!(score("WriteLine", "WL"), Some(2));
        assert_eq!(score("StringBuilder", "SBu"), Some(2));
        assert_eq!(score("WriteLine", "line"), Some(2));
        assert_eq!(score("WriteLine", "itel"), Some(3));
        assert_eq!(score("WriteLine", "xyz"), None);
    }

    #[test]
    fn rank_orders_best_first_and_drops_misses() {
        let items = [item("ReadLine"), item("WriteLine"), item("Write"), item("Beep")];
        let names: Vec<&str> = rank(&items, "Wri").into_iter().map(|i| items[i].label.as_str()).collect();
        assert_eq!(names, ["Write", "WriteLine"]);
        let names: Vec<&str> = rank(&items, "line").into_iter().map(|i| items[i].label.as_str()).collect();
        assert_eq!(names, ["ReadLine", "WriteLine"]);
    }

    #[test]
    fn preselected_item_leads_among_equals() {
        let mut p = item("List");
        p.preselect = Some(true);
        let items = [item("Array"), p];
        assert_eq!(rank(&items, ""), [1, 0]);
    }
}
