//! Shared dock presentation model: the screen edge the dock anchors to.
//!
//! The type lives in the model crate so both the configuration schema
//! (`tessera-config`) and the dock chrome component (`tessera-dock`) speak the
//! same vocabulary without depending on one another — mirroring
//! [`crate::window::DecorationPolicy`].

/// The screen edge the dock panel anchors to.
///
/// The top edge is intentionally absent: the dock supports the left, bottom,
/// and right edges only. Written in kebab-case in the `[dock]` configuration
/// table (`position = "left"`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DockPosition {
    /// A vertical strip anchored to the left edge, centred vertically.
    Left,
    /// A horizontal strip anchored to the bottom edge, centred horizontally.
    #[default]
    Bottom,
    /// A vertical strip anchored to the right edge, centred vertically.
    Right,
}

impl DockPosition {
    /// Whether the dock's tile strip runs along the vertical axis (the left
    /// or right edge) rather than the horizontal one.
    pub fn is_vertical(self) -> bool {
        matches!(self, DockPosition::Left | DockPosition::Right)
    }

    /// The canonical kebab-case name used in configuration.
    pub fn name(self) -> &'static str {
        match self {
            DockPosition::Left => "left",
            DockPosition::Bottom => "bottom",
            DockPosition::Right => "right",
        }
    }
}

/// The animation played when a window minimizes into the dock (and,
/// reversed, when it restores), mirroring the classic macOS effects.
///
/// Written in kebab-case in the `[dock]` configuration table
/// (`minimize_animation = "genie"`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MinimizeAnimationStyle {
    /// The window funnels into the icon: its lower edge pinches toward the
    /// icon first while the top edge follows, like a genie returning to its
    /// bottle. Rendered as a horizontal-strip warp approximation.
    #[default]
    Genie,
    /// The window scales down uniformly into the icon's rectangle.
    Scale,
    /// The window accelerates as it collapses into the icon's centre point,
    /// as if vacuumed in.
    Suck,
}

impl MinimizeAnimationStyle {
    /// The canonical kebab-case name used in configuration.
    pub fn name(self) -> &'static str {
        match self {
            MinimizeAnimationStyle::Genie => "genie",
            MinimizeAnimationStyle::Scale => "scale",
            MinimizeAnimationStyle::Suck => "suck",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bottom_is_horizontal() {
        assert!(DockPosition::Left.is_vertical());
        assert!(!DockPosition::Bottom.is_vertical());
        assert!(DockPosition::Right.is_vertical());
    }

    #[test]
    fn names_are_the_kebab_case_config_spellings() {
        assert_eq!(DockPosition::Left.name(), "left");
        assert_eq!(DockPosition::Bottom.name(), "bottom");
        assert_eq!(DockPosition::Right.name(), "right");
    }

    #[test]
    fn minimize_animation_names_are_the_config_spellings() {
        assert_eq!(MinimizeAnimationStyle::Genie.name(), "genie");
        assert_eq!(MinimizeAnimationStyle::Scale.name(), "scale");
        assert_eq!(MinimizeAnimationStyle::Suck.name(), "suck");
    }
}
