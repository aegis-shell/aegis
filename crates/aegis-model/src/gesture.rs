//! Configurable touchpad swipe bindings.
//!
//! Pure: a [`GestureMap`] is an ordered list of [`GestureBinding`]s —
//! (finger count, axis) → [`GestureAction`]. The binary layers the
//! configuration file over the built-in defaults and matches each touchpad
//! swipe against it. Keeping this module in `aegis-model` (no flux, lens, or
//! Wayland dependency) lets the binding table and name resolvers be
//! unit-tested in isolation.
//!
//! Unlike key bindings, swipe actions are directional pairs: the gesture's
//! dominant sign selects the direction inside the action (left/up versus
//! right/down), and one gesture can fire several steps. The runtime owns the
//! per-gesture state (accumulators, axis latch, per-action bookkeeping);
//! this table only answers which action listens to a (fingers, axis) pair.

/// A compositor action a touchpad swipe can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureAction {
    /// Consume the swipe without doing anything. Binding an axis to `None`
    /// shadows a built-in default while keeping the gesture compositor-owned.
    None,
    /// Horizontal swipe: left steps to the next workspace, right to the
    /// previous one (ADR-0025).
    WorkspaceSwitch,
    /// Vertical swipe: up focuses the next window on the current workspace,
    /// down the previous one, through the window switcher held open for the
    /// gesture's duration.
    WindowCycle,
    /// Vertical swipe: down opens the command panel, up closes it
    /// (ADR-0080). Fires at most once per gesture.
    CommandPanel,
    /// Vertical swipe: up opens the window/workspace overview, down closes
    /// it (M9, ADR-0116). Fires at most once per gesture.
    Overview,
}

/// The axis a swipe binding listens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureAxis {
    Horizontal,
    Vertical,
}

/// One binding: a finger count plus an axis, mapped to an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GestureBinding {
    pub fingers: u8,
    pub axis: GestureAxis,
    pub action: GestureAction,
}

/// An ordered set of swipe bindings; the first match on (fingers, axis)
/// wins.
#[derive(Debug, Clone, Default)]
pub struct GestureMap {
    binds: Vec<GestureBinding>,
}

impl GestureMap {
    /// Built-in defaults (ADR-0080, ADR-0082, ADR-0116).
    pub fn defaults() -> GestureMap {
        GestureMap {
            binds: vec![
                gb(3, GestureAxis::Horizontal, GestureAction::WorkspaceSwitch),
                gb(3, GestureAxis::Vertical, GestureAction::WindowCycle),
                gb(4, GestureAxis::Vertical, GestureAction::Overview),
            ],
        }
    }

    /// Prepend `overrides` so user bindings take precedence over the
    /// defaults. The returned map keeps the defaults as a fallback.
    pub fn with_overrides(mut self, overrides: Vec<GestureBinding>) -> GestureMap {
        let mut combined = overrides;
        combined.append(&mut self.binds);
        GestureMap { binds: combined }
    }

    /// Keep only bindings whose actions are available in the consuming
    /// product build. Removed finger counts are no longer claimed, so their
    /// gestures can continue to Wayland clients.
    pub fn retain_actions(mut self, mut keep: impl FnMut(GestureAction) -> bool) -> GestureMap {
        self.binds.retain(|binding| keep(binding.action));
        self
    }

    /// Find the action listening on (fingers, axis). First match wins.
    pub fn lookup(&self, fingers: u8, axis: GestureAxis) -> Option<GestureAction> {
        self.binds
            .iter()
            .find(|b| b.fingers == fingers && b.axis == axis)
            .map(|b| b.action)
    }

    /// Whether any binding listens to this finger count, regardless of axis.
    /// A swipe with a claimed finger count never reaches clients, even when
    /// its latched axis has no binding.
    pub fn claims(&self, fingers: u8) -> bool {
        self.binds.iter().any(|b| b.fingers == fingers)
    }

    /// Number of bindings (for diagnostics / tests).
    pub fn len(&self) -> usize {
        self.binds.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.binds.is_empty()
    }
}

const fn gb(fingers: u8, axis: GestureAxis, action: GestureAction) -> GestureBinding {
    GestureBinding {
        fingers,
        axis,
        action,
    }
}

/// Resolve an axis name to its [`GestureAxis`]; unknown names return `None`.
pub fn gesture_axis_from_name(s: &str) -> Option<GestureAxis> {
    Some(match s.to_ascii_lowercase().as_str() {
        "horizontal" | "h" => GestureAxis::Horizontal,
        "vertical" | "v" => GestureAxis::Vertical,
        _ => return None,
    })
}

/// Resolve an action name to its [`GestureAction`]; unknown names return
/// `None`.
pub fn gesture_action_from_name(s: &str) -> Option<GestureAction> {
    Some(match s.to_ascii_lowercase().as_str() {
        "none" | "unbind" | "disabled" => GestureAction::None,
        "workspace_switch" | "workspaces" | "workspace" => GestureAction::WorkspaceSwitch,
        "window_cycle" | "cycle_windows" | "windows" | "switcher" => GestureAction::WindowCycle,
        "command_panel" | "commandpanel" | "panel" => GestureAction::CommandPanel,
        "overview" | "window_overview" | "picker" => GestureAction::Overview,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_documented_gestures() {
        let gm = GestureMap::defaults();
        assert_eq!(
            gm.lookup(3, GestureAxis::Horizontal),
            Some(GestureAction::WorkspaceSwitch)
        );
        assert_eq!(
            gm.lookup(3, GestureAxis::Vertical),
            Some(GestureAction::WindowCycle)
        );
        assert_eq!(
            gm.lookup(4, GestureAxis::Vertical),
            Some(GestureAction::Overview)
        );
        // Unbound axes and finger counts have no listener.
        assert_eq!(gm.lookup(4, GestureAxis::Horizontal), None);
        assert_eq!(gm.lookup(2, GestureAxis::Vertical), None);
        // Claiming is per finger count, regardless of axis.
        assert!(gm.claims(3));
        assert!(gm.claims(4));
        assert!(!gm.claims(2));
        assert!(!gm.claims(5));
    }

    #[test]
    fn override_takes_precedence_and_keeps_defaults() {
        let gm = GestureMap::defaults().with_overrides(vec![gb(
            3,
            GestureAxis::Vertical,
            GestureAction::CommandPanel,
        )]);
        assert_eq!(
            gm.lookup(3, GestureAxis::Vertical),
            Some(GestureAction::CommandPanel)
        );
        assert_eq!(
            gm.lookup(3, GestureAxis::Horizontal),
            Some(GestureAction::WorkspaceSwitch)
        );
        assert!(gm.len() >= 4);
    }

    #[test]
    fn none_action_shadows_a_default_but_still_claims() {
        let gm = GestureMap::defaults().with_overrides(vec![gb(
            4,
            GestureAxis::Vertical,
            GestureAction::None,
        )]);
        assert_eq!(
            gm.lookup(4, GestureAxis::Vertical),
            Some(GestureAction::None)
        );
        assert!(gm.claims(4));
    }

    #[test]
    fn action_filter_releases_removed_finger_count() {
        let gm = GestureMap::defaults().retain_actions(|action| action != GestureAction::Overview);
        assert_eq!(gm.lookup(4, GestureAxis::Vertical), None);
        assert!(!gm.claims(4));
        assert!(gm.claims(3));
    }

    #[test]
    fn name_resolvers_accept_documented_names() {
        assert_eq!(
            gesture_axis_from_name("horizontal"),
            Some(GestureAxis::Horizontal)
        );
        assert_eq!(
            gesture_axis_from_name("Vertical"),
            Some(GestureAxis::Vertical)
        );
        assert_eq!(gesture_axis_from_name("diagonal"), None);
        assert_eq!(
            gesture_action_from_name("workspace_switch"),
            Some(GestureAction::WorkspaceSwitch)
        );
        assert_eq!(
            gesture_action_from_name("window_cycle"),
            Some(GestureAction::WindowCycle)
        );
        assert_eq!(
            gesture_action_from_name("command_panel"),
            Some(GestureAction::CommandPanel)
        );
        assert_eq!(
            gesture_action_from_name("overview"),
            Some(GestureAction::Overview)
        );
        assert_eq!(gesture_action_from_name("none"), Some(GestureAction::None));
        assert_eq!(gesture_action_from_name("nonsense"), None);
    }
}
