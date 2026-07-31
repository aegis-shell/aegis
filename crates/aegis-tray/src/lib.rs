//! StatusNotifierItem (SNI) system tray service.
//!
//! The compositor acts as both the StatusNotifierWatcher and a
//! StatusNotifierHost on the session bus: a dedicated worker setup owns the
//! `org.kde.StatusNotifierWatcher` name, tracks every registered item, and
//! mirrors them into a plain [`TraySnapshot`] the render thread reads. No
//! async runtime is involved — zbus runs its blocking API on two
//! `std::thread`s (signal tracking and command dispatch) and shares state
//! through `Arc<Mutex<_>>` + `std::sync::mpsc`, per ADR-0021/0044.
//!
//! Failure is silent by design: without a session bus, or when another
//! watcher already owns the name, [`spawn`] returns `None` and shell chrome
//! simply shows no SNI icons. The composition root spawns the service once
//! and shares the handle between the display-only HUD and the
//! interactive command panel (ADR-0080).
//!
//! Items that expose a `com.canonical.dbusmenu` object path (the SNI `Menu`
//! property) also receive host-rendered context menus: the worker fetches the
//! layout via `GetLayout` and stores the parsed [`MenuState`] in the snapshot,
//! the consuming chrome paints a popover from it, and clicks travel back
//! through [`TrayCommand::MenuEvent`]. Items without a menu path keep the
//! legacy `SecondaryActivate` right-click.

mod icon;
mod watcher;

use std::borrow::Borrow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use zbus::zvariant::{OwnedValue, Value};
/// One registered StatusNotifierItem, mirrored for the render thread.
#[derive(Debug, Clone)]
pub struct TrayItem {
    /// Stable identity: `{bus_name}{object_path}` as registered.
    pub key: String,
    /// `Title` property, falling back to `Id`.
    pub title: String,
    /// Preferred icon source: the raw pixmap when the item ships one, else
    /// the theme icon name (the worker resolves these through the freedesktop
    /// icon theme into a pixmap before publishing; a `Name` left in the
    /// snapshot means resolution failed and the shell renders a generic glyph),
    /// else nothing.
    pub icon: TrayIcon,
    pub status: TrayStatus,
    /// Whether the item exposes a dbusmenu `Menu` object path, i.e. whether
    /// right-click should be `ContextMenu` rather than `SecondaryActivate`.
    pub has_menu: bool,
    /// Bumped whenever the pixmap bytes change; together with `key` this is
    /// the shell's texture-cache key.
    pub icon_generation: u64,
}

impl TrayItem {
    /// Items in `Passive` status are not rendered (SNI spec).
    pub fn is_visible(&self) -> bool {
        self.status != TrayStatus::Passive
    }
}

/// The `Status` property, mapped to the spec's three values.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatus {
    #[default]
    Active,
    Passive,
    NeedsAttention,
}

impl TrayStatus {
    fn parse(status: &str) -> TrayStatus {
        match status {
            "Passive" => TrayStatus::Passive,
            "NeedsAttention" => TrayStatus::NeedsAttention,
            // Unknown values render as active so the item stays reachable.
            _ => TrayStatus::Active,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub enum TrayIcon {
    /// Freedesktop theme icon name (`IconName`).
    Name(String),
    /// Raw pixels (`IconPixmap`), converted to unpremultiplied BGRA8 — the
    /// format flux samples (`FLUX_FORMAT_BGRA8_UNORM`), matching the
    /// compositor's app-icon path.
    Pixmap(TrayPixmap),
    #[default]
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrayPixmap {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, unpremultiplied BGRA8.
    pub bgra: Vec<u8>,
}

/// A parsed `com.canonical.dbusmenu` layout node (the recursive
/// `(id, props, children)` tuple from `GetLayout`).
#[derive(Debug, Clone, Default)]
pub struct MenuNode {
    pub id: i32,
    pub kind: MenuEntryKind,
    /// Mnemonic underscores stripped (see `strip_mnemonic`).
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub toggle: MenuToggle,
    /// `children-display == "submenu"`.
    pub has_submenu: bool,
    pub children: Vec<MenuNode>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MenuEntryKind {
    #[default]
    Standard,
    Separator,
}

/// `toggle-state` per the dbusmenu spec: 0 = unchecked, 1 = checked, <0 =
/// indeterminate (treated as unchecked for display).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MenuToggle {
    #[default]
    None,
    Checkmark(i32),
    Radio(i32),
}

impl MenuToggle {
    /// Whether the toggle reads as on for glyph rendering.
    pub fn is_on(&self) -> bool {
        matches!(self, MenuToggle::Checkmark(1) | MenuToggle::Radio(1))
    }
}

/// Currently open dbusmenu, mirrored for the render thread. Only one menu is
/// open at a time; opening another replaces this.
#[derive(Debug, Clone)]
pub struct MenuState {
    /// Item key this menu belongs to (`TrayItem::key`).
    pub key: String,
    pub root: MenuNode,
    pub revision: u32,
}

/// Render-thread view of the tray, sorted by key for a stable cell order.
#[derive(Debug, Default, Clone)]
pub struct TraySnapshot {
    pub items: Vec<TrayItem>,
    /// The currently open dbusmenu popover, if any.
    pub menu: Option<MenuState>,
    /// Bumped by the worker every time `menu` is set or cleared, so the
    /// render thread can cache the (potentially large) menu tree clone and
    /// only re-clone on change.
    pub menu_revision: u64,
}

/// Click intents handed to the worker thread; `(x, y)` is the cursor
/// position in output coordinates, as the spec's methods expect.
#[derive(Debug)]
pub enum TrayCommand {
    Activate {
        key: String,
        x: i32,
        y: i32,
    },
    SecondaryActivate {
        key: String,
        x: i32,
        y: i32,
    },
    ContextMenu {
        key: String,
        x: i32,
        y: i32,
    },
    /// Open the host-rendered dbusmenu popover for this item (replaces any
    /// menu currently open).
    FetchMenu {
        key: String,
    },
    /// Send `Event("clicked", ...)` for entry `id` on the currently open
    /// menu. Falls back to `SecondaryActivate` if the menu vanished.
    MenuEvent {
        key: String,
        id: i32,
    },
    /// Tell the worker the popover closed; it stops tracking layout updates
    /// and clears the shared [`MenuState`].
    CloseMenu {
        key: String,
    },
}

/// Start the watcher/host on the session bus. Returns the shared snapshot
/// and the command channel, or `None` when the tray is unavailable (no
/// session bus, or another watcher owns the name).
pub fn spawn() -> Option<(Arc<Mutex<TraySnapshot>>, mpsc::Sender<TrayCommand>)> {
    watcher::spawn()
}

/// Convert one `IconPixmap` payload to BGRA8.
///
/// Per the SNI spec each pixmap is ARGB32 in network byte order, so the
/// in-memory byte sequence per pixel is `A, R, G, B`; flux samples BGRA, so
/// the output sequence is `B, G, R, A` (alpha kept unpremultiplied, matching
/// the compositor's app-icon upload path).
pub(crate) fn argb32_to_bgra(data: &[u8]) -> Vec<u8> {
    let mut bgra = Vec::with_capacity(data.len());
    for pixel in data.chunks_exact(4) {
        let [a, r, g, b] = [pixel[0], pixel[1], pixel[2], pixel[3]];
        bgra.extend_from_slice(&[b, g, r, a]);
    }
    bgra
}

/// Pick the `IconPixmap` entry whose largest side is closest to `target`,
/// preferring the larger entry on ties (downscaling beats upscaling).
/// Entries with non-positive dimensions or a payload that does not match
/// `w * h * 4` are skipped. Returns the index into `pixmaps`.
pub(crate) fn select_pixmap(pixmaps: &[(i32, i32, Vec<u8>)], target: i32) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (index, (w, h, data)) in pixmaps.iter().enumerate() {
        if *w <= 0 || *h <= 0 || data.len() != (*w as usize) * (*h as usize) * 4 {
            continue;
        }
        let size = (*w).max(*h);
        best = match best {
            None => Some(index),
            Some(prev) => {
                let prev_size = (pixmaps[prev].0).max(pixmaps[prev].1);
                let (dist, prev_dist) = ((size - target).abs(), (prev_size - target).abs());
                if dist < prev_dist || (dist == prev_dist && size > prev_size) {
                    Some(index)
                } else {
                    Some(prev)
                }
            }
        };
    }
    best
}

// ---- dbusmenu parsing ----------------------------------------------------

/// Shape of the `GetLayout` reply: `(revision, root_layout)` where
/// `root_layout` is the recursive `(id, props, children)` node and each
/// child in `children` is a `Variant` wrapping the same structure.
pub(crate) type LayoutReply = (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>));

/// Property names requested from `GetLayout` so the parser can rely on a
/// stable shape (any unrequested property is simply absent from the dict).
pub(crate) const MENU_PROPERTY_NAMES: &[&str] = &[
    "type",
    "label",
    "enabled",
    "visible",
    "icon-name",
    "toggle-type",
    "toggle-state",
    "children-display",
    "disposition",
];

/// Parse the top-level `GetLayout` reply into a [`MenuNode`] tree plus the
/// reported revision. A malformed root yields `None`; malformed children are
/// dropped silently so a single bad leaf does not abort the whole tree.
pub(crate) fn parse_layout_reply(reply: LayoutReply) -> Option<(MenuNode, u32)> {
    let (revision, (id, props, children)) = reply;
    Some((parse_layout_node(id, &props, &children), revision))
}

fn parse_layout_node(
    id: i32,
    props: &HashMap<String, OwnedValue>,
    children: &[OwnedValue],
) -> MenuNode {
    let kind = match get_string(props, "type").as_deref() {
        Some("separator") => MenuEntryKind::Separator,
        _ => MenuEntryKind::Standard,
    };
    let label = strip_mnemonic(&get_string(props, "label").unwrap_or_default());
    let enabled = get_bool(props, "enabled").unwrap_or(true);
    let visible = get_bool(props, "visible").unwrap_or(true);
    let toggle = match get_string(props, "toggle-type").as_deref() {
        Some("checkmark") => MenuToggle::Checkmark(get_int(props, "toggle-state").unwrap_or(0)),
        Some("radio") => MenuToggle::Radio(get_int(props, "toggle-state").unwrap_or(0)),
        _ => MenuToggle::None,
    };
    let has_submenu = get_string(props, "children-display").as_deref() == Some("submenu");
    let children: Vec<MenuNode> = children
        .iter()
        .filter_map(|child| parse_child_value(child.borrow()))
        .collect();
    MenuNode {
        id,
        kind,
        label,
        enabled,
        visible,
        toggle,
        has_submenu,
        children,
    }
}

/// Walk a single child `Value` from an `av` array: dbusmenu wraps each child
/// layout node in a variant, so we descend through `Value::Value` and then
/// expect a `(i32, a{sv}, av)` structure.
fn parse_child_value(value: &Value<'_>) -> Option<MenuNode> {
    let target = match value {
        // The dbusmenu layout nests each child inside a `v`.
        Value::Value(inner) => &**inner,
        // Some toolkits skip the variant wrapper for the root node.
        Value::Structure(_) => value,
        _ => return None,
    };
    let structure = match target {
        Value::Structure(s) => s,
        _ => return None,
    };
    let fields = structure.fields();
    if fields.len() != 3 {
        return None;
    }
    let id: i32 = fields[0].downcast_ref().ok()?;
    let mut props: HashMap<String, OwnedValue> = HashMap::new();
    if let Value::Dict(dict) = &fields[1] {
        for (key, val) in dict.iter() {
            if let Ok(name) = <&str>::try_from(key)
                && let Ok(owned) = val.try_to_owned()
            {
                props.insert(name.to_string(), owned);
            }
        }
    }
    let children: Vec<OwnedValue> = match &fields[2] {
        Value::Array(array) => array.iter().filter_map(|v| v.try_to_owned().ok()).collect(),
        _ => Vec::new(),
    };
    Some(parse_layout_node(id, &props, &children))
}

fn get_string(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    props.get(key).and_then(|value| {
        Borrow::<Value>::borrow(value)
            .downcast_ref::<&str>()
            .ok()
            .map(str::to_string)
    })
}

fn get_bool(props: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    props
        .get(key)
        .and_then(|value| Borrow::<Value>::borrow(value).downcast_ref::<bool>().ok())
}

fn get_int(props: &HashMap<String, OwnedValue>, key: &str) -> Option<i32> {
    props
        .get(key)
        .and_then(|value| Borrow::<Value>::borrow(value).downcast_ref::<i32>().ok())
}

/// Strip `_` mnemonic prefixes from a dbusmenu label (e.g. `_Open` → `Open`,
/// `Sa_ve As…` → `Save As…`). A lone `_` is left untouched (some items use it
/// as a placeholder for an empty label). Visual underline of the mnemonic
/// character is not rendered in v1 — only the un-prefixed label is needed.
pub(crate) fn strip_mnemonic(label: &str) -> String {
    if label.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '_' {
            if let Some(&next) = chars.peek() {
                if next == '_' {
                    // "__" → keep one literal underscore.
                    out.push('_');
                    chars.next();
                    continue;
                }
                // "_" before another character: drop the underscore.
                continue;
            }
            // Trailing "_" — keep it (some items pad with a lone underscore).
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    out
}

/// Resolve the visible children list at the given breadcrumb. `path[0]` is
/// the root sentinel (always 0 in v1 — the worker stores the layout under the
/// root id); each subsequent id descends into the matching submenu. Returns
/// the root's children when `path` is empty or `[0]`. Returns `None` if a
/// targeted submenu id no longer exists (the shell should pop back to the
/// nearest valid ancestor).
pub fn visible_children<'a>(root: &'a MenuNode, path: &[i32]) -> Option<&'a [MenuNode]> {
    let mut node = root;
    for id in path.iter().skip(1) {
        let next = node
            .children
            .iter()
            .find(|child| child.id == *id && child.has_submenu)?;
        node = next;
    }
    Some(&node.children)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argb_converts_byte_order_and_channels() {
        // One pixel per 4 bytes, network byte order: A, R, G, B.
        let argb = [0x80, 0x11, 0x22, 0x33, 0xff, 0xaa, 0xbb, 0xcc];
        let bgra = argb32_to_bgra(&argb);
        assert_eq!(bgra, vec![0x33, 0x22, 0x11, 0x80, 0xcc, 0xbb, 0xaa, 0xff]);
    }

    #[test]
    fn argb_ignores_trailing_partial_pixel() {
        assert_eq!(argb32_to_bgra(&[1, 2, 3]), Vec::<u8>::new());
    }

    #[test]
    fn select_pixmap_picks_closest_size() {
        let pixmaps = vec![
            (16, 16, vec![0; 16 * 16 * 4]),
            (24, 24, vec![0; 24 * 24 * 4]),
            (64, 64, vec![0; 64 * 64 * 4]),
        ];
        assert_eq!(select_pixmap(&pixmaps, 48), Some(2));
        assert_eq!(select_pixmap(&pixmaps, 24), Some(1));
        assert_eq!(select_pixmap(&pixmaps, 10), Some(0));
    }

    #[test]
    fn select_pixmap_prefers_larger_on_tie() {
        let pixmaps = vec![
            (32, 32, vec![0; 32 * 32 * 4]),
            (64, 64, vec![0; 64 * 64 * 4]),
        ];
        // Both are 16px away from 48; the larger wins.
        assert_eq!(select_pixmap(&pixmaps, 48), Some(1));
    }

    #[test]
    fn select_pixmap_skips_invalid_entries() {
        let pixmaps = vec![
            (24, 24, vec![0; 10]), // payload too short
            (0, 24, vec![]),       // degenerate dimensions
            (-1, -1, vec![]),      // negative dimensions
            (22, 22, vec![0; 22 * 22 * 4]),
        ];
        assert_eq!(select_pixmap(&pixmaps, 48), Some(3));
        assert_eq!(select_pixmap(&[], 48), None);
        assert_eq!(select_pixmap(&pixmaps[..3], 48), None);
    }

    #[test]
    fn status_parsing_and_visibility() {
        assert_eq!(TrayStatus::parse("Active"), TrayStatus::Active);
        assert_eq!(TrayStatus::parse("Passive"), TrayStatus::Passive);
        assert_eq!(
            TrayStatus::parse("NeedsAttention"),
            TrayStatus::NeedsAttention
        );
        // Unknown values stay visible rather than silently dropping the item.
        assert_eq!(TrayStatus::parse("Bogus"), TrayStatus::Active);

        let item = |status| TrayItem {
            key: String::new(),
            title: String::new(),
            icon: TrayIcon::None,
            status,
            has_menu: false,
            icon_generation: 0,
        };
        assert!(item(TrayStatus::Active).is_visible());
        assert!(!item(TrayStatus::Passive).is_visible());
        assert!(item(TrayStatus::NeedsAttention).is_visible());
    }

    fn owned_string(value: &str) -> OwnedValue {
        OwnedValue::from(zbus::zvariant::Str::from(value.to_string()))
    }

    fn owned_bool(value: bool) -> OwnedValue {
        OwnedValue::from(value)
    }

    fn owned_i32(value: i32) -> OwnedValue {
        OwnedValue::from(value)
    }

    #[test]
    fn mnemonic_strip_handles_leading_inner_and_doubles() {
        assert_eq!(strip_mnemonic("_Open"), "Open");
        assert_eq!(strip_mnemonic("Sa_ve As…"), "Save As…");
        assert_eq!(strip_mnemonic("_"), "_");
        assert_eq!(strip_mnemonic(""), "");
        assert_eq!(strip_mnemonic("Plain"), "Plain");
        // Doubled underscores collapse to one literal.
        assert_eq!(strip_mnemonic("F__ile"), "F_ile");
        // Trailing underscore stays.
        assert_eq!(strip_mnemonic("Cut_"), "Cut_");
    }

    #[test]
    fn layout_node_reads_kind_label_toggle_and_submenu() {
        let mut props = HashMap::new();
        props.insert("type".to_string(), owned_string("standard"));
        props.insert("label".to_string(), owned_string("_Quit"));
        props.insert("enabled".to_string(), owned_bool(true));
        props.insert("visible".to_string(), owned_bool(true));
        props.insert("toggle-type".to_string(), owned_string("checkmark"));
        props.insert("toggle-state".to_string(), owned_i32(1));
        let node = parse_layout_node(7, &props, &[]);
        assert_eq!(node.id, 7);
        assert_eq!(node.kind, MenuEntryKind::Standard);
        assert_eq!(node.label, "Quit");
        assert!(node.enabled);
        assert!(node.visible);
        assert_eq!(node.toggle, MenuToggle::Checkmark(1));
        assert!(node.toggle.is_on());
        assert!(!node.has_submenu);
    }

    #[test]
    fn layout_node_separator_and_radio_states() {
        let mut sep_props = HashMap::new();
        sep_props.insert("type".to_string(), owned_string("separator"));
        let sep = parse_layout_node(1, &sep_props, &[]);
        assert_eq!(sep.kind, MenuEntryKind::Separator);

        let mut radio_props = HashMap::new();
        radio_props.insert("toggle-type".to_string(), owned_string("radio"));
        // toggle-state < 0 reads indeterminate → not on.
        radio_props.insert("toggle-state".to_string(), owned_i32(-1));
        let radio = parse_layout_node(2, &radio_props, &[]);
        assert_eq!(radio.toggle, MenuToggle::Radio(-1));
        assert!(!radio.toggle.is_on());

        let mut radio_on = HashMap::new();
        radio_on.insert("toggle-type".to_string(), owned_string("radio"));
        radio_on.insert("toggle-state".to_string(), owned_i32(1));
        assert!(parse_layout_node(3, &radio_on, &[]).toggle.is_on());

        // Absent toggle-type → None.
        assert_eq!(
            parse_layout_node(4, &HashMap::new(), &[]).toggle,
            MenuToggle::None
        );
        // children-display == "submenu" sets the flag.
        let mut sub = HashMap::new();
        sub.insert("children-display".to_string(), owned_string("submenu"));
        assert!(parse_layout_node(5, &sub, &[]).has_submenu);
    }

    #[test]
    fn visible_children_walks_submenus_and_root() {
        // root -> [leaf(1), submenu(2) -> [leaf(3)]]
        let root = MenuNode {
            id: 0,
            kind: MenuEntryKind::Standard,
            label: String::new(),
            enabled: true,
            visible: true,
            toggle: MenuToggle::None,
            has_submenu: false,
            children: vec![
                MenuNode {
                    id: 1,
                    ..MenuNode::default()
                },
                MenuNode {
                    id: 2,
                    has_submenu: true,
                    children: vec![MenuNode {
                        id: 3,
                        ..MenuNode::default()
                    }],
                    ..MenuNode::default()
                },
            ],
        };

        // Root view: path = [0]
        let view = visible_children(&root, &[0]).unwrap();
        assert_eq!(view.len(), 2);
        assert_eq!(view[0].id, 1);

        // Descend into submenu 2: path = [0, 2]
        let sub = visible_children(&root, &[0, 2]).unwrap();
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].id, 3);

        // Pop back to root: path = [0]
        assert_eq!(visible_children(&root, &[0]).unwrap().len(), 2);

        // Missing submenu id → None (bar should pop back).
        assert!(visible_children(&root, &[0, 99]).is_none());

        // Trying to descend into a non-submenu child → None.
        assert!(visible_children(&root, &[0, 1]).is_none());
    }

    #[test]
    fn parse_child_value_descends_through_variant_and_structure() {
        use zbus::zvariant::{Dict, Signature, StructureBuilder, Value};

        // Build the inner props dict for a leaf node manually: a{sv} with one
        // entry "label" -> "_Hello".
        let mut dict = Dict::new(&Signature::Str, &Signature::Variant);
        dict.append(
            Value::Str(zbus::zvariant::Str::from("label".to_string())),
            Value::Value(Box::new(Value::Str(zbus::zvariant::Str::from(
                "_Hello".to_string(),
            )))),
        )
        .unwrap();

        // Empty children array (`av`).
        let empty_arr: zbus::zvariant::Array = Vec::<Value>::new().into();

        // `append_field` pushes the Value as-is; the From<tuple> impl would
        // route through Value::new, which wraps any Value in another Value
        // because Value's Type signature is `v`.
        let structure = StructureBuilder::new()
            .append_field(Value::I32(42))
            .append_field(Value::Dict(dict))
            .append_field(Value::Array(empty_arr))
            .build()
            .unwrap();
        // dbusmenu wraps each child in a variant.
        let wrapped = Value::Value(Box::new(Value::Structure(structure)));

        let node = parse_child_value(&wrapped).expect("node should parse");
        assert_eq!(node.id, 42);
        assert_eq!(node.label, "Hello");
        assert!(node.children.is_empty());
    }
}
