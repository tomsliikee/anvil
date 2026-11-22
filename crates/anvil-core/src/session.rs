//! Per-solution UI state in `.vs/anvil/` (never in the project folder, so
//! SDK-style projects do not list it): startup project, open tabs,
//! breakpoints, diagram view state, window layout, caret and scroll per file.

use std::collections::BTreeMap;
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
    /// Caret and first visible line of each open code file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<FileView>,
    /// Files in the second editor group (split view), in tab order. A file
    /// that is also in [`Session::open_files`] is the same buffer shown twice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub second_group: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_active: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
}

/// Where a file was when the solution was closed. Lines and columns are
/// 0-based, as GTK counts them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileView {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub top_line: u32,
}

/// The three places a tool window can dock, as in VS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DockArea {
    Left,
    Right,
    Bottom,
}

impl DockArea {
    pub const ALL: [DockArea; 3] = [DockArea::Left, DockArea::Right, DockArea::Bottom];

    pub fn nick(self) -> &'static str {
        match self {
            DockArea::Left => "left",
            DockArea::Right => "right",
            DockArea::Bottom => "bottom",
        }
    }

    pub fn from_nick(s: &str) -> Option<DockArea> {
        DockArea::ALL.into_iter().find(|a| a.nick() == s)
    }
}

/// One dock area: shown or hidden, its size in pixels (width for the
/// sides, height for the bottom; 0 = the default) and the page in front.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AreaState {
    #[serde(default = "yes")]
    pub open: bool,
    #[serde(default)]
    pub size: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
}

impl Default for AreaState {
    fn default() -> AreaState {
        AreaState { open: true, size: 0, active: None }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowState {
    pub width: i32,
    pub height: i32,
    #[serde(default)]
    pub maximized: bool,
}

/// The window layout: which tool window docks where, the areas' sizes and
/// whether they are open, the window size and the editor split.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    /// Panels moved away from their default area, by panel name.
    #[serde(default)]
    pub panels: BTreeMap<String, DockArea>,
    #[serde(default)]
    pub areas: BTreeMap<DockArea, AreaState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowState>,
    /// Width of the first editor group while the editor is split.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<i32>,
}

impl Layout {
    /// The area a panel docks in: where the user put it, else its default.
    pub fn area_of(&self, panel: &str, default: DockArea) -> DockArea {
        self.panels.get(panel).copied().unwrap_or(default)
    }

    /// Records a move; a panel back in its default area is not stored, so
    /// a later change of a default reaches users who never moved it.
    pub fn move_panel(&mut self, panel: &str, to: DockArea, default: DockArea) {
        if to == default {
            self.panels.remove(panel);
        } else {
            self.panels.insert(panel.to_owned(), to);
        }
    }

    pub fn area(&self, area: DockArea) -> AreaState {
        self.areas.get(&area).cloned().unwrap_or_default()
    }

    pub fn from_json(json: &str) -> Option<Layout> {
        serde_json::from_str(json).ok()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("serialises")
    }
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
    fn layout_round_trip_and_defaults() {
        let mut l = Layout::default();
        assert_eq!(l.area_of("explorer", DockArea::Right), DockArea::Right);
        l.move_panel("explorer", DockArea::Left, DockArea::Right);
        assert_eq!(l.area_of("explorer", DockArea::Right), DockArea::Left);
        l.move_panel("explorer", DockArea::Right, DockArea::Right);
        assert!(l.panels.is_empty());
        l.move_panel("output", DockArea::Right, DockArea::Bottom);
        l.areas.insert(DockArea::Bottom, AreaState { open: false, size: 300, active: Some("errors".into()) });
        l.window = Some(WindowState { width: 1200, height: 800, maximized: false });
        let back = Layout::from_json(&l.to_json()).unwrap();
        assert_eq!(back, l);
        assert!(back.to_json().contains(r#""output":"right""#));
        // An area never stored is open at its default size.
        assert_eq!(l.area(DockArea::Left), AreaState { open: true, size: 0, active: None });
        assert_eq!(Layout::from_json("{}"), Some(Layout::default()));
        assert_eq!(DockArea::from_nick("bottom"), Some(DockArea::Bottom));
    }

    #[test]
    fn session_with_views_and_split_round_trips() {
        let d = std::env::temp_dir().join(format!("anvil-session-views-{}", std::process::id()));
        let s = Session {
            open_files: vec!["/a/P.cs".into(), "/a/Q.cs".into()],
            views: vec![FileView { path: "/a/P.cs".into(), line: 40, column: 4, top_line: 22 }],
            second_group: vec!["/a/P.cs".into()],
            second_active: Some("/a/P.cs".into()),
            layout: Some(Layout { split: Some(700), ..Default::default() }),
            ..Default::default()
        };
        s.save(&d).unwrap();
        assert_eq!(Session::load(&d), s);
        let _ = std::fs::remove_dir_all(&d);
        // A state file from before views and layouts existed.
        let old: Session = serde_json::from_str(r#"{"open_files":["/a.cs"],"active_file":"/a.cs"}"#).unwrap();
        assert!(old.views.is_empty() && old.layout.is_none());
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
