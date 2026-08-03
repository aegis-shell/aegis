//! Compositor-owned semantic observations for actor-scoped automation.
//!
//! These types describe what the compositor can prove from its own model.
//! Application-internal controls require an accessibility or application
//! protocol adapter and must be attached as descendants of these stable
//! window roots; pixels are never treated as semantic authority.

use crate::interaction_domain::InteractionDomainId;
use crate::window::WindowId;
use crate::{Rect, Size};

/// Stable semantic object identity namespaced by its owning window.
///
/// `local == 0` is the compositor-owned window root. Accessibility providers
/// allocate non-zero local ids. This prevents a provider from forging a node
/// in another window and avoids collisions between application trees.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticObjectId {
    pub window: WindowId,
    pub local: u64,
}

impl SemanticObjectId {
    pub fn for_window(window: WindowId) -> Self {
        Self { window, local: 0 }
    }

    pub fn descendant(window: WindowId, local: u64) -> Option<Self> {
        (window.0 != 0 && local != 0).then_some(Self { window, local })
    }

    pub fn is_valid(self) -> bool {
        self.window.0 != 0
    }
}

/// Provenance of one observed semantic object.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticSource {
    Compositor,
    Accessibility,
}

/// Role of a compositor-owned semantic object.
///
/// `Window` is the root currently guaranteed by the Wayland compositor.
/// The remaining roles reserve a stable vocabulary for accessibility-tree
/// adapters without pretending that framebuffer inference is authoritative.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticRole {
    Window,
    Dialog,
    Button,
    TextField,
    List,
    ListItem,
    Document,
    Image,
    CheckBox,
    ComboBox,
    Menu,
    MenuItem,
    Tab,
    Slider,
    Link,
    Heading,
    Paragraph,
    Unknown,
}

/// Actions a semantic object declares it can accept.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SemanticAction {
    Invoke,
    Focus,
    Pointer,
    Scroll,
    TypeText,
    Close,
    SetValue,
    Select,
    Expand,
    Collapse,
}

/// Semantically expressed action. Pointer input remains an explicit fallback
/// rather than the universal action representation.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type", rename_all = "snake_case"))]
#[derive(Debug, Clone, PartialEq)]
pub enum SemanticActionIntent {
    Invoke,
    Focus,
    SetValue {
        value: String,
    },
    TypeText {
        text: String,
    },
    Select {
        selected: bool,
    },
    Expand,
    Collapse,
    /// Target-local low-level fallback for surfaces without an accessibility
    /// action implementation.
    SyntheticInput {
        actions: Vec<crate::input::SyntheticInputAction>,
    },
}

/// State relevant to safe action preconditions.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SemanticState {
    pub visible: bool,
    pub enabled: bool,
    pub focused: bool,
    pub read_only: bool,
    pub minimized: bool,
}

/// One semantic object observed in an Interaction Domain.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticObject {
    pub id: SemanticObjectId,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub parent: Option<SemanticObjectId>,
    /// Owning toplevel. Accessibility descendants retain this value so
    /// authorization never depends on labels.
    pub window: WindowId,
    pub source: SemanticSource,
    pub role: SemanticRole,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub name: Option<String>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub description: Option<String>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub value: Option<String>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub app_id: Option<String>,
    /// Bounds in the Interaction Domain's directed-output logical coordinates.
    pub bounds: Rect,
    /// Target-local extent used by pointer actions.
    pub local_size: Size,
    pub state: SemanticState,
    pub actions: Vec<SemanticAction>,
    /// Compositor-owned content revision. An actor action must be rejected
    /// when this differs from the revision carried by its observation lease.
    pub revision: u64,
}

/// Semantic state captured atomically with an Interaction Domain observation.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticSnapshot {
    pub interaction_domain: InteractionDomainId,
    pub authority_revision: u64,
    pub objects: Vec<SemanticObject>,
}

impl SemanticSnapshot {
    pub fn object(&self, id: SemanticObjectId) -> Option<&SemanticObject> {
        self.objects.iter().find(|object| object.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_root_ids_use_durable_window_ids() {
        let window = WindowId(42);
        let object = SemanticObjectId::for_window(window);
        assert_eq!(object, SemanticObjectId { window, local: 0 });
    }

    #[cfg(feature = "serde")]
    #[test]
    fn semantic_snapshot_round_trips() {
        let snapshot = SemanticSnapshot {
            interaction_domain: InteractionDomainId(7),
            authority_revision: 11,
            objects: vec![SemanticObject {
                id: SemanticObjectId::for_window(WindowId(3)),
                parent: None,
                window: WindowId(3),
                source: SemanticSource::Compositor,
                role: SemanticRole::Window,
                name: Some("Checkout".into()),
                description: None,
                value: None,
                app_id: Some("shop.example".into()),
                bounds: Rect::new(10, 20, 800, 600),
                local_size: Size { w: 800, h: 600 },
                state: SemanticState {
                    visible: true,
                    enabled: true,
                    ..Default::default()
                },
                actions: vec![SemanticAction::Pointer, SemanticAction::TypeText],
                revision: 9,
            }],
        };
        let encoded = serde_json::to_string(&snapshot).unwrap();
        let decoded: SemanticSnapshot = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, snapshot);
    }
}
