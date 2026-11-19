//! Visual Studio's keyboard model: single keys and two-key chords
//! (`Ctrl+K, Ctrl+D`), scopes (Text Editor wins over Global), user
//! overrides on top of the VS default scheme, and conflict detection.

use std::collections::BTreeMap;

use crate::commands::{Scope, COMMANDS};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Combo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Normalised key name: `K`, `F5`, `Enter`, `.`, `Up`, …
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    pub first: Combo,
    pub second: Option<Combo>,
}

const NAMES: &[(&str, &str)] = &[
    ("esc", "Escape"),
    ("escape", "Escape"),
    ("return", "Enter"),
    ("enter", "Enter"),
    ("del", "Delete"),
    ("delete", "Delete"),
    ("ins", "Insert"),
    ("insert", "Insert"),
    ("bksp", "Backspace"),
    ("backspace", "Backspace"),
    ("space", "Space"),
    ("tab", "Tab"),
    ("up", "Up"),
    ("down", "Down"),
    ("left", "Left"),
    ("right", "Right"),
    ("uparrow", "Up"),
    ("downarrow", "Down"),
    ("leftarrow", "Left"),
    ("rightarrow", "Right"),
    ("home", "Home"),
    ("end", "End"),
    ("pgup", "PageUp"),
    ("pageup", "PageUp"),
    ("pgdn", "PageDown"),
    ("pagedown", "PageDown"),
    ("break", "Break"),
    ("pause", "Break"),
];

impl Combo {
    pub fn parse(s: &str) -> Option<Combo> {
        let mut c = Combo { ctrl: false, shift: false, alt: false, key: String::new() };
        // A trailing "+" key ("Ctrl++") or "," key.
        let s = s.trim();
        let (mods, key) = if let Some(mods) = s.strip_suffix("++") {
            (mods, "+")
        } else {
            match s.rfind('+') {
                Some(i) => (&s[..i], &s[i + 1..]),
                None => ("", s),
            }
        };
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            match m.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "primary" => c.ctrl = true,
                "shift" => c.shift = true,
                "alt" => c.alt = true,
                _ => return None,
            }
        }
        c.key = normalise_key(key.trim())?;
        Some(c)
    }

    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        s.push_str(&self.key);
        s
    }
}

pub fn normalise_key(k: &str) -> Option<String> {
    if k.is_empty() {
        return None;
    }
    let lower = k.to_ascii_lowercase();
    if let Some((_, n)) = NAMES.iter().find(|(a, _)| *a == lower) {
        return Some((*n).to_owned());
    }
    if k.chars().count() == 1 {
        return Some(k.to_uppercase());
    }
    if (lower.starts_with('f') && lower[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n))) || lower.starts_with("num") {
        return Some(k.to_uppercase());
    }
    None
}

impl Chord {
    /// `"Ctrl+K, Ctrl+D"` or `"F5"`. VS writes a chord's second key with
    /// or without modifiers (`Ctrl+R, A`).
    pub fn parse(s: &str) -> Option<Chord> {
        // Split on ", " but not on a "," key ("Ctrl+,").
        let parts: Vec<&str> = match s.find(", ") {
            Some(i) if i > 0 && !s[..i].ends_with('+') => vec![&s[..i], &s[i + 2..]],
            _ => vec![s],
        };
        let first = Combo::parse(parts[0])?;
        let second = match parts.get(1) {
            Some(p) => Some(Combo::parse(p)?),
            None => None,
        };
        Some(Chord { first, second })
    }

    pub fn label(&self) -> String {
        match &self.second {
            Some(s) => format!("{}, {}", self.first.label(), s.label()),
            None => self.first.label(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Feed {
    /// Run this command.
    Command(&'static str),
    /// The first key of a chord; waiting for the second.
    Pending(Combo),
    /// A chord that is not bound (VS: "The key combination (…) is not a
    /// command.").
    NotACommand(Chord),
    /// Not a shortcut; let the widget have the key.
    Unbound,
}

#[derive(Clone, Debug)]
pub struct Keymap {
    /// command id → chords
    bindings: BTreeMap<&'static str, Vec<Chord>>,
    pending: Option<Combo>,
}

impl Keymap {
    pub fn vs_default() -> Keymap {
        let bindings = COMMANDS.iter().map(|c| (c.id, c.keys.iter().filter_map(|k| Chord::parse(k)).collect())).collect();
        Keymap { bindings, pending: None }
    }

    /// The VS scheme with the user's overrides (`{"edit.format-document":
    /// ["Ctrl+Shift+I"]}`; an empty list removes a binding). Unknown
    /// commands and unparsable keys are skipped and returned.
    pub fn with_overrides(json: &str) -> (Keymap, Vec<String>) {
        let mut map = Keymap::vs_default();
        let mut skipped = Vec::new();
        let parsed: BTreeMap<String, Vec<String>> = serde_json::from_str(json).unwrap_or_default();
        for (id, keys) in parsed {
            let Some(cmd) = COMMANDS.iter().find(|c| c.id == id) else {
                skipped.push(id);
                continue;
            };
            let chords: Vec<Chord> = keys.iter().filter_map(|k| Chord::parse(k).or_else(|| { skipped.push(k.clone()); None })).collect();
            map.bindings.insert(cmd.id, chords);
        }
        (map, skipped)
    }

    /// The overrides that differ from VS' defaults, as stored JSON.
    pub fn overrides_json(&self) -> String {
        let defaults = Keymap::vs_default();
        let diff: BTreeMap<&str, Vec<String>> = self
            .bindings
            .iter()
            .filter(|(id, chords)| defaults.bindings.get(*id) != Some(chords))
            .map(|(id, chords)| (*id, chords.iter().map(Chord::label).collect()))
            .collect();
        serde_json::to_string_pretty(&diff).expect("serialises")
    }

    pub fn set(&mut self, id: &'static str, chords: Vec<Chord>) {
        self.bindings.insert(id, chords);
    }

    pub fn chords(&self, id: &str) -> &[Chord] {
        self.bindings.get(id).map_or(&[], Vec::as_slice)
    }

    /// The label shown in menus and tooltips (`Ctrl+K, Ctrl+D`).
    pub fn label(&self, id: &str) -> Option<String> {
        self.chords(id).first().map(Chord::label)
    }

    pub fn pending(&self) -> Option<&Combo> {
        self.pending.as_ref()
    }

    pub fn cancel(&mut self) {
        self.pending = None;
    }

    fn lookup(&self, chord: &Chord, scope: Scope) -> Option<&'static str> {
        let mut global = None;
        for (id, chords) in &self.bindings {
            if chords.contains(chord) {
                let s = crate::commands::scope_of(id);
                if s == scope {
                    return Some(id);
                }
                if s == Scope::Global {
                    global = Some(*id);
                }
            }
        }
        global
    }

    fn is_prefix(&self, first: &Combo, scope: Scope) -> bool {
        self.bindings.iter().any(|(id, chords)| {
            let s = crate::commands::scope_of(id);
            (s == scope || s == Scope::Global) && chords.iter().any(|c| &c.first == first && c.second.is_some())
        })
    }

    /// Feeds one key press in `scope`.
    pub fn feed(&mut self, combo: Combo, scope: Scope) -> Feed {
        if let Some(first) = self.pending.take() {
            let chord = Chord { first, second: Some(combo) };
            return match self.lookup(&chord, scope) {
                Some(id) => Feed::Command(id),
                None => Feed::NotACommand(chord),
            };
        }
        if self.is_prefix(&combo, scope) {
            self.pending = Some(combo.clone());
            return Feed::Pending(combo);
        }
        match self.lookup(&Chord { first: combo, second: None }, scope) {
            Some(id) => Feed::Command(id),
            None => Feed::Unbound,
        }
    }

    /// Pairs of commands that share a chord in overlapping scopes, and
    /// single keys that shadow a chord's first key.
    pub fn conflicts(&self) -> Vec<(String, &'static str, &'static str)> {
        let mut out = Vec::new();
        let all: Vec<(&'static str, &Chord)> = self.bindings.iter().flat_map(|(id, cs)| cs.iter().map(move |c| (*id, c))).collect();
        for (i, (a, ca)) in all.iter().enumerate() {
            for (b, cb) in &all[i + 1..] {
                if a == b {
                    continue;
                }
                let (sa, sb) = (crate::commands::scope_of(a), crate::commands::scope_of(b));
                if sa != sb {
                    continue;
                }
                let clash = ca == cb || (ca.second.is_none() && cb.second.is_some() && ca.first == cb.first) || (cb.second.is_none() && ca.second.is_some() && ca.first == cb.first);
                if clash {
                    out.push((ca.label(), *a, *b));
                }
            }
        }
        out
    }

    /// Commands whose chords contain `query` in their label (Settings →
    /// Keyboard search by key).
    pub fn find_by_key(&self, query: &str) -> Vec<&'static str> {
        let q = query.to_ascii_lowercase();
        self.bindings.iter().filter(|(_, cs)| cs.iter().any(|c| c.label().to_ascii_lowercase().contains(&q))).map(|(id, _)| *id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> Combo {
        Combo::parse(s).unwrap()
    }

    #[test]
    fn parses_labels() {
        assert_eq!(Chord::parse("Ctrl+K, Ctrl+D").unwrap().label(), "Ctrl+K, Ctrl+D");
        assert_eq!(Chord::parse("Ctrl+R, A").unwrap().second.unwrap().key, "A");
        assert_eq!(Chord::parse("Ctrl+,").unwrap().first.key, ",");
        assert_eq!(Chord::parse("Shift+Alt+Enter").unwrap().label(), "Shift+Alt+Enter");
        assert_eq!(Chord::parse("ctrl+shift+b").unwrap().label(), "Ctrl+Shift+B");
        assert!(Chord::parse("Hyper+X").is_none());
    }

    #[test]
    fn ctrl_k_ctrl_d_formats() {
        let mut m = Keymap::vs_default();
        assert_eq!(m.feed(k("Ctrl+K"), Scope::Editor), Feed::Pending(k("Ctrl+K")));
        assert_eq!(m.feed(k("Ctrl+D"), Scope::Editor), Feed::Command("edit.format-document"));
        assert_eq!(m.label("edit.format-document").as_deref(), Some("Ctrl+K, Ctrl+D"));
    }

    #[test]
    fn ctrl_r_ctrl_r_renames_and_ctrl_r_a_runs_tests() {
        let mut m = Keymap::vs_default();
        m.feed(k("Ctrl+R"), Scope::Editor);
        assert_eq!(m.feed(k("Ctrl+R"), Scope::Editor), Feed::Command("edit.rename"));
        m.feed(k("Ctrl+R"), Scope::Global);
        assert_eq!(m.feed(k("A"), Scope::Global), Feed::Command("test.run-all"));
        m.feed(k("Ctrl+K"), Scope::Editor);
        assert!(matches!(m.feed(k("Q"), Scope::Editor), Feed::NotACommand(_)));
        assert_eq!(m.feed(k("F5"), Scope::Editor), Feed::Command("debug.start"));
        assert_eq!(m.feed(k("Ctrl+Shift+B"), Scope::Explorer), Feed::Command("build.solution"));
        assert_eq!(m.feed(k("X"), Scope::Editor), Feed::Unbound);
    }

    #[test]
    fn scope_wins_over_global() {
        let mut m = Keymap::vs_default();
        assert_eq!(m.feed(k("F2"), Scope::Explorer), Feed::Command("explorer.rename"));
        assert_eq!(m.feed(k("F2"), Scope::Editor), Feed::Command("edit.rename"));
        assert_eq!(m.feed(k("Delete"), Scope::Explorer), Feed::Command("explorer.delete"));
        assert_eq!(m.feed(k("Delete"), Scope::Editor), Feed::Unbound);
    }

    #[test]
    fn override_replaces_binding() {
        let (mut m, skipped) = Keymap::with_overrides(r#"{"edit.format-document": ["Ctrl+Shift+I"], "nope": ["F1"]}"#);
        assert_eq!(skipped, ["nope"]);
        assert_eq!(m.feed(k("Ctrl+Shift+I"), Scope::Editor), Feed::Command("edit.format-document"));
        assert!(m.overrides_json().contains("\"edit.format-document\": [\n    \"Ctrl+Shift+I\"\n  ]"));
        assert_eq!(Keymap::vs_default().overrides_json(), "{}");
    }

    #[test]
    fn vs_scheme_has_no_conflicts_and_every_key_parses() {
        for c in COMMANDS {
            for key in c.keys {
                assert!(Chord::parse(key).is_some(), "{} {key}", c.id);
            }
        }
        assert_eq!(Keymap::vs_default().conflicts(), Vec::<(String, &str, &str)>::new());
    }

    #[test]
    fn conflict_reported() {
        let mut m = Keymap::vs_default();
        m.set("build.solution", vec![Chord::parse("F5").unwrap()]);
        assert!(m.conflicts().iter().any(|(k, a, b)| k == "F5" && [*a, *b].contains(&"debug.start")));
        assert_eq!(m.find_by_key("ctrl+k, ctrl+d"), ["edit.format-document"]);
    }
}
