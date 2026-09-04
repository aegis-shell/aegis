//! Validated accessibility trees at the application/compositor trust seam.

use std::collections::{BTreeMap, BTreeSet};

use tessera_model::semantic::{
    SemanticAction, SemanticObject, SemanticObjectId, SemanticRole, SemanticSource, SemanticState,
};
use tessera_model::window::WindowId;
use tessera_model::{Point, Rect, Size};

const MAX_NODES_PER_WINDOW: usize = 4_096;
const MAX_TREE_DEPTH: usize = 64;
const MAX_TEXT_BYTES_PER_FIELD: usize = 16_384;
const MAX_TEXT_BYTES_PER_TREE: usize = 1_048_576;

/// Trusted binding between one compositor-owned Wayland toplevel and the
/// Unix process that owns its still-live Wayland connection. The first-party
/// accessibility adapter uses this to correlate AT-SPI bus credentials; it
/// is never part of the general window-observation API.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccessibilityWindowBinding {
    pub window: tessera_model::window::Window,
    pub process_id: u32,
}

/// Strong identity of the out-of-process accessibility adapter that owns a
/// published tree. It is supplied by the authority broker, not self-asserted
/// application metadata.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct SemanticProviderId(String);

impl SemanticProviderId {
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err("semantic provider id is invalid");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SemanticProviderId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One provider-native, window-local accessibility node.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccessibilityNode {
    /// Non-zero and unique within the owning window.
    pub local_id: u64,
    /// `None` attaches the provider root below the compositor window root.
    pub parent_local_id: Option<u64>,
    pub role: SemanticRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Surface-local logical coordinates.
    pub bounds: Rect,
    pub state: SemanticState,
    pub actions: Vec<SemanticAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccessibilityTreeUpdate {
    pub window: WindowId,
    /// Strictly increasing provider revision for this window.
    pub revision: u64,
    pub nodes: Vec<AccessibilityNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticDispatchTarget {
    pub provider: SemanticProviderId,
    pub window: WindowId,
    pub provider_node_id: u64,
    pub tree_revision: u64,
}

/// One compositor-validated semantic action delivered to the owning adapter.
/// The adapter must verify `tree_revision` against its live AT-SPI cache
/// immediately before invoking the action.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SemanticActionRequest {
    pub request_id: u64,
    pub target: SemanticObjectId,
    pub provider_node_id: u64,
    pub tree_revision: u64,
    pub action: tessera_model::semantic::SemanticActionIntent,
}

struct ValidatedTree {
    provider: SemanticProviderId,
    revision: u64,
    nodes: Vec<AccessibilityNode>,
}

#[derive(Default)]
pub struct SemanticTreeRegistry {
    trees: BTreeMap<WindowId, ValidatedTree>,
}

impl SemanticTreeRegistry {
    pub fn publish(
        &mut self,
        provider: SemanticProviderId,
        update: AccessibilityTreeUpdate,
        surface_size: Size,
    ) -> Result<(), String> {
        validate_update(&update, surface_size)?;
        if let Some(current) = self.trees.get(&update.window) {
            if current.provider != provider {
                return Err("semantic tree is owned by a different provider".into());
            }
            if update.revision <= current.revision {
                return Err("semantic tree revision did not advance".into());
            }
        }
        self.trees.insert(
            update.window,
            ValidatedTree {
                provider,
                revision: update.revision,
                nodes: update.nodes,
            },
        );
        Ok(())
    }

    pub fn objects_for_window(&self, window: WindowId, placement: Rect) -> Vec<SemanticObject> {
        let Some(tree) = self.trees.get(&window) else {
            return Vec::new();
        };
        tree.nodes
            .iter()
            .map(|node| SemanticObject {
                id: SemanticObjectId {
                    window,
                    local: node.local_id,
                },
                parent: Some(match node.parent_local_id {
                    Some(local) => SemanticObjectId { window, local },
                    None => SemanticObjectId::for_window(window),
                }),
                window,
                source: SemanticSource::Accessibility,
                role: node.role,
                name: node.name.clone(),
                description: node.description.clone(),
                value: node.value.clone(),
                app_id: None,
                bounds: Rect {
                    origin: Point {
                        x: placement.origin.x.saturating_add(node.bounds.origin.x),
                        y: placement.origin.y.saturating_add(node.bounds.origin.y),
                    },
                    size: node.bounds.size,
                },
                local_size: node.bounds.size,
                state: node.state,
                actions: node.actions.clone(),
                revision: tree.revision,
            })
            .collect()
    }

    pub fn resolve(&self, id: SemanticObjectId) -> Option<SemanticDispatchTarget> {
        if id.local == 0 {
            return None;
        }
        let tree = self.trees.get(&id.window)?;
        tree.nodes
            .iter()
            .any(|node| node.local_id == id.local)
            .then(|| SemanticDispatchTarget {
                provider: tree.provider.clone(),
                window: id.window,
                provider_node_id: id.local,
                tree_revision: tree.revision,
            })
    }

    pub fn remove_window(&mut self, window: WindowId) {
        self.trees.remove(&window);
    }

    pub fn revoke_provider(&mut self, provider: &SemanticProviderId) {
        self.trees.retain(|_, tree| &tree.provider != provider);
    }
}

fn validate_update(update: &AccessibilityTreeUpdate, surface_size: Size) -> Result<(), String> {
    if update.window.0 == 0 || update.revision == 0 {
        return Err("accessibility tree window or revision is invalid".into());
    }
    if update.nodes.is_empty() || update.nodes.len() > MAX_NODES_PER_WINDOW {
        return Err("accessibility tree node count is out of range".into());
    }
    if surface_size.w <= 0 || surface_size.h <= 0 {
        return Err("accessibility tree surface extent is invalid".into());
    }
    let ids = update
        .nodes
        .iter()
        .map(|node| node.local_id)
        .collect::<BTreeSet<_>>();
    if ids.len() != update.nodes.len() || ids.contains(&0) {
        return Err("accessibility node ids must be unique and non-zero".into());
    }
    let roots = update
        .nodes
        .iter()
        .filter(|node| node.parent_local_id.is_none())
        .count();
    if roots != 1 {
        return Err("accessibility tree must have exactly one provider root".into());
    }
    let mut text_bytes = 0usize;
    let parents = update
        .nodes
        .iter()
        .map(|node| (node.local_id, node.parent_local_id))
        .collect::<BTreeMap<_, _>>();
    for node in &update.nodes {
        if node
            .parent_local_id
            .is_some_and(|parent| !ids.contains(&parent))
        {
            return Err("accessibility node parent does not exist".into());
        }
        if node.bounds.size.w <= 0
            || node.bounds.size.h <= 0
            || node.bounds.origin.x < 0
            || node.bounds.origin.y < 0
            || node.bounds.origin.x.saturating_add(node.bounds.size.w) > surface_size.w
            || node.bounds.origin.y.saturating_add(node.bounds.size.h) > surface_size.h
        {
            return Err("accessibility node bounds escape the owning surface".into());
        }
        for value in [&node.name, &node.description, &node.value]
            .into_iter()
            .flatten()
        {
            if value.len() > MAX_TEXT_BYTES_PER_FIELD || value.contains('\0') {
                return Err("accessibility node text is out of range".into());
            }
            text_bytes = text_bytes.saturating_add(value.len());
        }
        if text_bytes > MAX_TEXT_BYTES_PER_TREE {
            return Err("accessibility tree text exceeds the safety bound".into());
        }
        let mut cursor = Some(node.local_id);
        let mut visited = BTreeSet::new();
        for _ in 0..=MAX_TREE_DEPTH {
            let Some(id) = cursor else { break };
            if !visited.insert(id) {
                return Err("accessibility tree contains a parent cycle".into());
            }
            cursor = parents.get(&id).copied().flatten();
        }
        if cursor.is_some() {
            return Err("accessibility tree exceeds the maximum depth".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(revision: u64) -> AccessibilityTreeUpdate {
        AccessibilityTreeUpdate {
            window: WindowId(7),
            revision,
            nodes: vec![
                AccessibilityNode {
                    local_id: 1,
                    parent_local_id: None,
                    role: SemanticRole::Document,
                    name: Some("Checkout".into()),
                    description: None,
                    value: None,
                    bounds: Rect::new(0, 0, 800, 600),
                    state: SemanticState {
                        visible: true,
                        enabled: true,
                        ..Default::default()
                    },
                    actions: vec![],
                },
                AccessibilityNode {
                    local_id: 2,
                    parent_local_id: Some(1),
                    role: SemanticRole::Button,
                    name: Some("Submit".into()),
                    description: None,
                    value: None,
                    bounds: Rect::new(700, 540, 80, 40),
                    state: SemanticState {
                        visible: true,
                        enabled: true,
                        ..Default::default()
                    },
                    actions: vec![SemanticAction::Invoke],
                },
            ],
        }
    }

    #[test]
    fn validates_namespaces_and_routes_accessibility_nodes() {
        let provider = SemanticProviderId::new("atspi.default").unwrap();
        let mut trees = SemanticTreeRegistry::default();
        trees
            .publish(provider.clone(), update(1), Size { w: 800, h: 600 })
            .unwrap();
        let objects = trees.objects_for_window(WindowId(7), Rect::new(10, 20, 800, 600));
        assert_eq!(objects[1].id.window, WindowId(7));
        assert_eq!(objects[1].bounds, Rect::new(710, 560, 80, 40));
        assert_eq!(trees.resolve(objects[1].id).unwrap().provider, provider);
    }

    #[test]
    fn rejects_cycles_cross_surface_bounds_and_provider_takeover() {
        let mut trees = SemanticTreeRegistry::default();
        let first = SemanticProviderId::new("atspi.one").unwrap();
        trees
            .publish(first, update(1), Size { w: 800, h: 600 })
            .unwrap();
        assert!(
            trees
                .publish(
                    SemanticProviderId::new("atspi.two").unwrap(),
                    update(2),
                    Size { w: 800, h: 600 }
                )
                .is_err()
        );
        let mut invalid = update(3);
        invalid.nodes[0].parent_local_id = Some(2);
        assert!(
            SemanticTreeRegistry::default()
                .publish(
                    SemanticProviderId::new("atspi.one").unwrap(),
                    invalid,
                    Size { w: 800, h: 600 }
                )
                .is_err()
        );
    }
}
