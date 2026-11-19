//! Per-solution UI state in `.vs/anvil/` (never in the project folder, so
//! SDK-style projects do not list it): startup project, open tabs,
//! breakpoints, diagram view state.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Breakpoint {
    pub file: PathBuf,
    /// 1-based.
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_count: Option<u32>,
    #[serde(default, skip_serializing_if = "HitMode::is_default")]
    pub hit_mode: HitMode,
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// VS' "Hit Count" choices; the count itself is [`Breakpoint::hit_count`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HitMode {
    #[default]
    Equal,
    AtLeast,
    MultipleOf,
}

impl HitMode {
    fn is_default(&self) -> bool {
        *self == HitMode::Equal
    }
}

impl Breakpoint {
    /// Whether the `hits`-th time this breakpoint is reached stops the
    /// program. netcoredbg ignores DAP's `hitCondition`, so Anvil counts.
    pub fn should_break(&self, hits: u32) -> bool {
        match (self.hit_count, self.hit_mode) {
            (None | Some(0), _) => true,
            (Some(n), HitMode::Equal) => hits == n,
            (Some(n), HitMode::AtLeast) => hits >= n,
            (Some(n), HitMode::MultipleOf) => hits.is_multiple_of(n),
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub startup_project: Option<String>,
    #[serde(default)]
    pub open_files: Vec<PathBuf>,
    #[serde(default)]
    pub active_file: Option<PathBuf>,
    #[serde(default)]
    pub breakpoints: Vec<Breakpoint>,
    #[serde(default)]
    pub watches: Vec<String>,
}

pub fn dir(solution_dir: &Path) -> PathBuf {
    solution_dir.join(".vs").join("anvil")
}

impl Session {
    pub fn load(solution_dir: &Path) -> Session {
        std::fs::read_to_string(dir(solution_dir).join("state.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, solution_dir: &Path) -> std::io::Result<()> {
        let d = dir(solution_dir);
        std::fs::create_dir_all(&d)?;
        crate::text::write_atomic(&d.join("state.json"), serde_json::to_string_pretty(self).expect("serialises").as_bytes())
    }
}

/// Zoom and scroll of a diagram, kept beside the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DiagramView {
    pub zoom: f64,
    pub x: f64,
    pub y: f64,
}

pub fn diagram_view_path(solution_dir: &Path, diagram: &Path) -> PathBuf {
    let name = diagram.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    dir(solution_dir).join("diagrams").join(format!("{name}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_in_dot_vs() {
        let d = std::env::temp_dir().join(format!("anvil-session-{}", std::process::id()));
        let s = Session { startup_project: Some("App".into()), breakpoints: vec![Breakpoint { file: "/a/P.cs".into(), line: 3, enabled: true, ..Default::default() }], ..Default::default() };
        s.save(&d).unwrap();
        assert!(d.join(".vs/anvil/state.json").exists());
        assert_eq!(Session::load(&d), s);
        assert_eq!(Session::load(Path::new("/nonexistent")), Session::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hit_count_modes() {
        let bp = |n, m| Breakpoint { hit_count: n, hit_mode: m, ..Default::default() };
        assert!((1..5).all(|h| bp(None, HitMode::Equal).should_break(h)));
        let eq = bp(Some(3), HitMode::Equal);
        assert_eq!((1..=5).map(|h| eq.should_break(h)).collect::<Vec<_>>(), [false, false, true, false, false]);
        let ge = bp(Some(3), HitMode::AtLeast);
        assert_eq!((1..=5).map(|h| ge.should_break(h)).collect::<Vec<_>>(), [false, false, true, true, true]);
        let mul = bp(Some(2), HitMode::MultipleOf);
        assert_eq!((1..=5).map(|h| mul.should_break(h)).collect::<Vec<_>>(), [false, true, false, true, false]);
    }

    #[test]
    fn old_state_files_still_load() {
        let bp: Breakpoint = serde_json::from_str(r#"{"file":"/a.cs","line":2,"hit_count":4}"#).unwrap();
        assert_eq!((bp.hit_count, bp.hit_mode, bp.enabled), (Some(4), HitMode::Equal, true));
        let s = serde_json::to_string(&Breakpoint { hit_mode: HitMode::MultipleOf, ..bp }).unwrap();
        assert!(s.contains(r#""hit_mode":"multiple-of""#));
    }
}
