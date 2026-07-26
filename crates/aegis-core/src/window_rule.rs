//! Config-driven window rules (ADR-0024/0026).
//!
//! A window rule matches a newly-mapped toplevel by `app_id` and/or `title`
//! and prescribes a placement action: move it to a workspace, force a layout
//! role. The compositor evaluates rules on first map and applies the first
//! match. The matching logic is pure so it is unit-tested in isolation; the
//! `ass-config` crate deserializes the same type from TOML, and `ass-server`
//! applies it.

/// A rule prescribing placement for toplevels it matches. Every set matcher
/// must match (AND). A rule with no matchers matches nothing, so a bare
/// `{ role = "floating" }` does not catch every window.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
#[derive(Debug, Clone, PartialEq)]
pub struct WindowRule {
    /// Match if the toplevel's `app_id` contains this, case-insensitively.
    /// `None` = do not constrain on app_id.
    pub app_id: Option<String>,
    /// Match if the toplevel's `title` contains this, case-insensitively.
    pub title: Option<String>,
    /// Move the window to this 1-based workspace index on the focused output.
    /// Applied only if that workspace exists (dynamic workspaces may not have
    /// created it yet); otherwise the rule's other actions still apply.
    pub workspace: Option<u32>,
    /// Force the layout role (ADR-0024). A `Floating` role exempts the
    /// window from tiling even when its workspace is in tiled mode.
    pub role: Option<crate::layout::LayoutRole>,
    /// Explicit position override in compositor logical coordinates.
    #[cfg_attr(feature = "serde", serde(default))]
    pub position: Option<crate::Point>,
    /// Explicit size override in compositor logical coordinates.
    #[cfg_attr(feature = "serde", serde(default))]
    pub size: Option<crate::Size>,
    /// Controls whether window geometry changes are remembered for this app across restarts.
    /// `None` or `Some(true)` allows auto-remembering; `Some(false)` disables it for matching windows.
    #[cfg_attr(feature = "serde", serde(default))]
    pub remember: Option<bool>,
}

impl WindowRule {
    /// Whether this rule matches a toplevel with the given `app_id`/`title`.
    /// All set matchers must match; a rule with no matchers matches nothing.
    pub fn matches(&self, app_id: Option<&str>, title: Option<&str>) -> bool {
        // A rule with neither matcher would match every window; reject it.
        if self.app_id.is_none() && self.title.is_none() {
            return false;
        }
        let mut ok = true;
        if let Some(want) = &self.app_id {
            ok &= contains_ci(app_id, want);
        }
        if let Some(want) = &self.title {
            ok &= contains_ci(title, want);
        }
        ok
    }
}

/// Case-insensitive substring test. `None` haystack never matches.
fn contains_ci(haystack: Option<&str>, needle: &str) -> bool {
    match haystack {
        Some(h) => {
            let needle = needle.to_ascii_lowercase();
            h.to_ascii_lowercase().contains(&needle)
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::LayoutRole;

    #[test]
    fn app_id_match_is_case_insensitive_substring() {
        let r = WindowRule {
            app_id: Some("firefox".into()),
            title: None,
            workspace: None,
            role: None,
            position: None,
            size: None,
            remember: None,
        };
        assert!(r.matches(Some("firefox"), None));
        assert!(r.matches(Some("Firefox"), None));
        assert!(r.matches(Some("org.mozilla.Firefox"), None));
        assert!(!r.matches(Some("chromium"), None));
        assert!(!r.matches(None, None));
    }

    #[test]
    fn title_match_works_alone() {
        let r = WindowRule {
            app_id: None,
            title: Some("Calculator".into()),
            workspace: None,
            role: None,
            position: None,
            size: None,
            remember: None,
        };
        assert!(r.matches(None, Some("GNOME Calculator")));
        assert!(!r.matches(None, Some("Terminal")));
    }

    #[test]
    fn multiple_matchers_are_anded() {
        let r = WindowRule {
            app_id: Some("org.example".into()),
            title: Some("settings".into()),
            workspace: None,
            role: None,
            position: None,
            size: None,
            remember: None,
        };
        assert!(r.matches(Some("org.example.App"), Some("Settings")));
        assert!(!r.matches(Some("org.example.App"), Some("Main"))); // title misses
        assert!(!r.matches(Some("other"), Some("Settings"))); // app_id misses
    }

    #[test]
    fn a_rule_with_no_matchers_matches_nothing() {
        let r = WindowRule {
            app_id: None,
            title: None,
            workspace: Some(2),
            role: Some(LayoutRole::Floating),
            position: None,
            size: None,
            remember: None,
        };
        assert!(!r.matches(Some("anything"), Some("whatever")));
    }
}
