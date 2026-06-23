//! The launcher's pure state machine — query, filtering, and selection.
//!
//! No flux, lens, or Wayland dependency. The chrome component in
//! `ass-shell` owns one of these and delegates key handling and rendering to
//! it; this module is unit-tested in isolation. See ADR-0022.

use crate::app::Entry;
use crate::input::KeyAction;

/// What the launcher asked the main loop to do when the user activates a row.
#[derive(Debug, Clone)]
pub enum Launch {
    /// Spawn this entry (it is not already running, or its `app_id` is
    /// unknown to the compositor). Boxed: `Entry` carries several owned
    /// strings and would otherwise inflate the enum's stack footprint on the
    /// small `Focus` variant.
    Spawn(Box<Entry>),
    /// Focus an already-running instance whose Wayland `app_id` matched this
    /// entry's `StartupWMClass` (or its desktop id). Carries the surface id
    /// to feed `Server::focus_surface_by_id`.
    Focus(usize),
}

/// The launcher's interaction state.
///
/// Holds the full enumerable set of [`Entry`] values and a typed-search brain
/// over them. `open` toggles whether the launcher overlay is shown and whether
/// it captures keyboard input; `query` filters the list; `selection` tracks
/// the highlighted row (an index into [`Launcher::filtered`]). A per-frame
/// snapshot of which `app_id`s are running lets the launcher focus an existing
/// instance instead of spawning a duplicate.
pub struct Launcher {
    apps: Vec<Entry>,
    open: bool,
    query: String,
    /// Index into the filtered list (not into `apps`). Clamped on every
    /// mutation that can shrink the filtered list.
    selection: usize,
    /// `(app_id, surface_id)` pairs the chrome refreshes each frame from the
    /// server's live toplevel snapshot. Empty when nothing matches.
    running: Vec<(String, usize)>,
}

impl Launcher {
    /// Construct with the launchable entries the binary enumerated.
    pub fn new(apps: Vec<Entry>) -> Launcher {
        Launcher {
            apps,
            open: false,
            query: String::new(),
            selection: 0,
            running: Vec::new(),
        }
    }

    /// Whether the overlay is shown and captures keyboard input.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Show the overlay. Resets the query and selection.
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selection = 0;
    }

    /// Hide the overlay and clear the query.
    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selection = 0;
    }

    /// Toggle the overlay.
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    /// The current search query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The full entry set (for size reporting / fallback rendering).
    pub fn apps(&self) -> &[Entry] {
        &self.apps
    }

    /// Indices into [`Launcher::apps`] matching the current query, in display
    /// order (which is the apps' already-sorted order). Empty query matches
    /// everything.
    pub fn filtered(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..self.apps.len()).collect();
        }
        let needle = self.query.to_lowercase();
        self.apps
            .iter()
            .enumerate()
            .filter(|(_, e)| matches_query(e, &needle))
            .map(|(i, _)| i)
            .collect()
    }

    /// The currently highlighted filtered index. Clamped to the filtered
    /// list length so callers never read out of bounds.
    pub fn selection(&self) -> usize {
        let len = self.filtered().len();
        if len == 0 {
            0
        } else {
            self.selection.min(len - 1)
        }
    }

    /// Advance the state machine with one key action. Returns `Some(outcome)`
    /// when the action activates an app (`Enter` on a non-empty filtered
    /// list): [`Launch::Spawn`] if the app is not running, [`Launch::Focus`]
    /// to bring an already-running instance forward. The launcher closes
    /// itself on activation.
    ///
    /// No-op when the launcher is closed — opening happens via mouse toggle
    /// or the global Super-tap hotkey.
    pub fn handle(&mut self, action: KeyAction) -> Option<Launch> {
        if !self.open {
            return None;
        }
        match action {
            KeyAction::Escape => {
                self.close();
                None
            }
            KeyAction::Backspace => {
                self.query.pop();
                self.clamp_selection();
                None
            }
            KeyAction::Char(c) => {
                self.query.push(c);
                self.selection = 0;
                None
            }
            KeyAction::Enter => {
                let outcome = self
                    .filtered()
                    .get(self.selection())
                    .copied()
                    .map(|i| self.launch_outcome(i));
                if outcome.is_some() {
                    self.close();
                }
                outcome
            }
            KeyAction::Up => {
                self.move_selection(-1);
                None
            }
            KeyAction::Down => {
                self.move_selection(1);
                None
            }
            KeyAction::Tab | KeyAction::Ignore => None,
        }
    }

    /// Activate by filtered index directly (mouse click on a row). Closes the
    /// launcher on success.
    pub fn launch_filtered(&mut self, filtered_index: usize) -> Option<Launch> {
        let outcome = self
            .filtered()
            .get(filtered_index)
            .copied()
            .map(|i| self.launch_outcome(i));
        if outcome.is_some() {
            self.close();
        }
        outcome
    }

    /// Refresh the snapshot of running applications. The chrome calls this
    /// each frame with `(app_id, surface_id)` pairs from the server's live
    /// toplevel list. Matching an entry against it lets the launcher focus an
    /// existing instance instead of spawning a duplicate.
    pub fn set_running(&mut self, running: Vec<(String, usize)>) {
        self.running = running;
    }

    /// Whether the entry at `app_idx` has a running instance. Exposed so the
    /// chrome can mark running rows (e.g. a leading `●`) in its render.
    pub fn is_running(&self, app_idx: usize) -> bool {
        self.surface_if_running(app_idx).is_some()
    }

    /// Decide what activating `app_idx` should do: focus a matching running
    /// instance, or spawn a fresh one.
    fn launch_outcome(&self, app_idx: usize) -> Launch {
        match self.surface_if_running(app_idx) {
            Some(sid) => Launch::Focus(sid),
            None => Launch::Spawn(Box::new(self.apps[app_idx].clone())),
        }
    }

    /// Find the surface id of a running instance matching `apps[app_idx]`, if
    /// any. An entry matches when the client's `app_id` equals the entry's
    /// `StartupWMClass`, or — when that is unset — equals the entry's desktop
    /// id with its `.desktop` suffix stripped (case-insensitive, mirroring
    /// how most toolkits derive `app_id` from the desktop file name).
    fn surface_if_running(&self, app_idx: usize) -> Option<usize> {
        let e = &self.apps[app_idx];
        let id_stem = e.id.trim_end_matches(".desktop");
        for (app_id, sid) in &self.running {
            if let Some(wm) = e.startup_wm_class.as_deref() {
                if wm == app_id.as_str() {
                    return Some(*sid);
                }
            }
            if id_stem.eq_ignore_ascii_case(app_id.as_str()) {
                return Some(*sid);
            }
        }
        None
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.filtered().len();
        if len == 0 {
            self.selection = 0;
            return;
        }
        let cur = self.selection().min(len - 1) as i32;
        // Wrap around the filtered list.
        let next = ((cur + delta).rem_euclid(len as i32)) as usize;
        self.selection = next;
    }

    fn clamp_selection(&mut self) {
        let len = self.filtered().len();
        if len == 0 {
            self.selection = 0;
        } else {
            self.selection = self.selection.min(len - 1);
        }
    }
}

/// Case-insensitive substring match across the fields a user would search by.
fn matches_query(e: &Entry, needle: &str) -> bool {
    if e.name.to_lowercase().contains(needle) {
        return true;
    }
    if e.generic_name
        .as_deref()
        .map(|s| s.to_lowercase().contains(needle))
        .unwrap_or(false)
    {
        return true;
    }
    if e.comment
        .as_deref()
        .map(|s| s.to_lowercase().contains(needle))
        .unwrap_or(false)
    {
        return true;
    }
    if e.id.to_lowercase().contains(needle) {
        return true;
    }
    e.keywords.iter().any(|k| k.to_lowercase().contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Entry;

    fn entry(id: &str, name: &str) -> Entry {
        Entry {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    fn entry_with(id: &str, name: &str, gen: &str, kws: &[&str]) -> Entry {
        Entry {
            id: id.into(),
            name: name.into(),
            generic_name: Some(gen.into()),
            keywords: kws.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn closed_launcher_ignores_keys() {
        let mut l = Launcher::new(vec![entry("a.desktop", "A")]);
        assert!(l.handle(KeyAction::Char('x')).is_none());
        assert!(l.query().is_empty());
        assert!(!l.is_open());
    }

    #[test]
    fn open_and_type_builds_query() {
        let mut l = Launcher::new(vec![entry("a.desktop", "Abc")]);
        l.open();
        l.handle(KeyAction::Char('a'));
        l.handle(KeyAction::Char('B'));
        l.handle(KeyAction::Char('c'));
        assert_eq!(l.query(), "aBc");
    }

    #[test]
    fn backspace_pops_query() {
        let mut l = Launcher::new(vec![]);
        l.open();
        l.handle(KeyAction::Char('f'));
        l.handle(KeyAction::Char('o'));
        l.handle(KeyAction::Backspace);
        assert_eq!(l.query(), "f");
    }

    #[test]
    fn escape_closes_and_clears() {
        let mut l = Launcher::new(vec![]);
        l.open();
        l.handle(KeyAction::Char('x'));
        l.handle(KeyAction::Escape);
        assert!(!l.is_open());
        assert!(l.query().is_empty());
    }

    #[test]
    fn filter_matches_case_insensitively_across_fields() {
        let apps = vec![
            entry_with(
                "firefox.desktop",
                "Firefox",
                "Web Browser",
                &["www", "internet"],
            ),
            entry_with("code.desktop", "Code", "Editor", &["ide"]),
            entry_with("foot.desktop", "Foot", "Terminal", &["shell"]),
        ];
        let mut l = Launcher::new(apps);
        l.open();

        // Name match.
        l.handle(KeyAction::Char('f'));
        l.handle(KeyAction::Char('i'));
        l.handle(KeyAction::Char('r'));
        assert_eq!(l.query(), "fir");
        assert_eq!(l.filtered(), vec![0]); // Firefox

        // Clear and match by keyword.
        l.handle(KeyAction::Backspace);
        l.handle(KeyAction::Backspace);
        l.handle(KeyAction::Backspace);
        l.handle(KeyAction::Char('i'));
        l.handle(KeyAction::Char('d'));
        l.handle(KeyAction::Char('e'));
        assert_eq!(l.query(), "ide");
        assert_eq!(l.filtered(), vec![1]); // Code (keyword "ide")
    }

    #[test]
    fn empty_query_matches_everything() {
        let apps = vec![entry("a.desktop", "A"), entry("b.desktop", "B")];
        let mut l = Launcher::new(apps);
        l.open();
        assert_eq!(l.filtered().len(), 2);
    }

    #[test]
    fn up_down_wrap_around_filtered_list() {
        let apps = vec![
            entry("a.desktop", "A"),
            entry("b.desktop", "B"),
            entry("c.desktop", "C"),
        ];
        let mut l = Launcher::new(apps);
        l.open();
        assert_eq!(l.selection(), 0);

        l.handle(KeyAction::Down);
        assert_eq!(l.selection(), 1);
        l.handle(KeyAction::Down);
        assert_eq!(l.selection(), 2);
        // Wrap to top.
        l.handle(KeyAction::Down);
        assert_eq!(l.selection(), 0);
        // Wrap to bottom from top.
        l.handle(KeyAction::Up);
        assert_eq!(l.selection(), 2);
    }

    #[test]
    fn enter_launches_selected_entry_and_closes() {
        let apps = vec![
            entry("a.desktop", "A"),
            entry("b.desktop", "B"),
            entry("c.desktop", "C"),
        ];
        let mut l = Launcher::new(apps);
        l.open();
        l.handle(KeyAction::Down); // select B (index 1)
        match l.handle(KeyAction::Enter) {
            Some(Launch::Spawn(e)) => assert_eq!(e.id, "b.desktop"),
            other => panic!("expected Spawn, got {other:?}"),
        }
        assert!(!l.is_open());
        assert!(l.query().is_empty());
    }

    #[test]
    fn enter_on_empty_filtered_list_launches_nothing() {
        let apps = vec![entry("a.desktop", "A")];
        let mut l = Launcher::new(apps);
        l.open();
        l.handle(KeyAction::Char('z')); // no match
        assert!(l.filtered().is_empty());
        let launched = l.handle(KeyAction::Enter);
        assert!(launched.is_none());
        // Launcher stays open so the user can correct the query.
        assert!(l.is_open());
    }

    #[test]
    fn typing_resets_selection_to_top() {
        let apps = vec![
            entry("a.desktop", "A"),
            entry("b.desktop", "B"),
            entry("c.desktop", "C"),
        ];
        let mut l = Launcher::new(apps);
        l.open();
        l.handle(KeyAction::Down);
        l.handle(KeyAction::Down); // selection = 2
        l.handle(KeyAction::Char('b')); // narrows to [B]
        assert_eq!(l.selection(), 0);
        assert_eq!(l.filtered(), vec![1]);
    }

    #[test]
    fn mouse_launch_by_filtered_index() {
        let apps = vec![entry("a.desktop", "A"), entry("b.desktop", "B")];
        let mut l = Launcher::new(apps);
        l.open();
        match l.launch_filtered(1) {
            Some(Launch::Spawn(e)) => assert_eq!(e.id, "b.desktop"),
            other => panic!("expected Spawn, got {other:?}"),
        }
        assert!(!l.is_open());
    }

    #[test]
    fn ignore_action_is_a_noop() {
        let mut l = Launcher::new(vec![entry("a.desktop", "A")]);
        l.open();
        assert!(l.handle(KeyAction::Ignore).is_none());
        assert!(l.handle(KeyAction::Tab).is_none());
        assert_eq!(l.query(), "");
        assert!(l.is_open());
    }

    #[test]
    fn running_match_by_startup_wm_class_focuses_instead_of_spawning() {
        // A desktop entry whose StartupWMClass matches a running app_id.
        let mut firefox = entry("org.mozilla.firefox.desktop", "Firefox");
        firefox.startup_wm_class = Some("firefox".into());
        let apps = vec![firefox];
        let mut l = Launcher::new(apps);
        l.open();
        // Surface 42 is running with app_id "firefox".
        l.set_running(vec![("firefox".into(), 42)]);

        assert!(l.is_running(0));
        match l.handle(KeyAction::Enter) {
            Some(Launch::Focus(sid)) => assert_eq!(sid, 42),
            other => panic!("expected Focus, got {other:?}"),
        }
        assert!(!l.is_open());
    }

    #[test]
    fn running_match_by_desktop_id_stem() {
        // Entry has no StartupWMClass; the app_id is the desktop id stem.
        let apps = vec![entry("foot.desktop", "Foot")];
        let mut l = Launcher::new(apps);
        l.open();
        // app_id "foot" matches "foot.desktop" with the suffix stripped,
        // case-insensitively.
        l.set_running(vec![("FOOT".into(), 7)]);
        assert!(l.is_running(0));
        match l.launch_filtered(0) {
            Some(Launch::Focus(sid)) => assert_eq!(sid, 7),
            other => panic!("expected Focus, got {other:?}"),
        }
    }

    #[test]
    fn non_matching_app_id_is_not_running() {
        let mut e = entry("a.desktop", "A");
        e.startup_wm_class = Some("A".into());
        let mut l = Launcher::new(vec![e]);
        l.open();
        l.set_running(vec![("something-else".into(), 1)]);
        assert!(!l.is_running(0));
        match l.handle(KeyAction::Enter) {
            Some(Launch::Spawn(e)) => assert_eq!(e.id, "a.desktop"),
            other => panic!("expected Spawn, got {other:?}"),
        }
    }

    #[test]
    fn set_running_replaces_previous_snapshot() {
        let apps = vec![entry("foot.desktop", "Foot")];
        let mut l = Launcher::new(apps);
        l.open();
        l.set_running(vec![("foot".into(), 1)]);
        assert!(l.is_running(0));
        // Window closed: next frame reports no running apps.
        l.set_running(Vec::new());
        assert!(!l.is_running(0));
    }
}
