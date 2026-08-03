use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use aegis_core::semantic::{SemanticAction, SemanticActionIntent, SemanticRole, SemanticState};
use aegis_core::window::{Window, WindowId};
use aegis_core::{Rect, Size};
use aegis_ipc::{ActorCapability, AgentHello, Client, ConnectionCapabilities};
use aegis_semantic::{AccessibilityNode, AccessibilityTreeUpdate, AccessibilityWindowBinding};
use atspi::connection::P2P as _;
use atspi::proxy::action::ActionProxy;
use atspi::proxy::component::ComponentProxy;
use atspi::proxy::editable_text::EditableTextProxy;
use atspi::proxy::selection::SelectionProxy;
use atspi::proxy::text::TextProxy;
use atspi::proxy::value::ValueProxy;
use atspi::{CoordType, Interface, ObjectRefOwned, Role, State};
use sha2::{Digest as _, Sha256};

const SCAN_INTERVAL: Duration = Duration::from_millis(750);
const ACTION_POLL: Duration = Duration::from_millis(100);
const MAX_NODES: usize = 4_096;
const MAX_DEPTH: usize = 64;

#[derive(Clone)]
struct CachedAccessible {
    object: ObjectRefOwned,
    parent: Option<ObjectRefOwned>,
    child_index: i32,
    revision: u64,
    window_size: Size,
    root: bool,
    precondition_hash: [u8; 32],
}

#[derive(Clone)]
struct PublishedTree {
    content_hash: [u8; 32],
    revision: u64,
}

struct AdapterState {
    objects: BTreeMap<(WindowId, u64), CachedAccessible>,
    published: BTreeMap<WindowId, PublishedTree>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    aegis_logging::init("aegis-atspi");
    let socket = socket_path()?;
    let credential_from_stdin =
        std::env::args_os().any(|argument| argument == "--credential-stdin");
    if !credential_from_stdin {
        return Err(
            "aegis-atspi must be launched by the compositor with --credential-stdin".into(),
        );
    }
    let credential = Some(read_credential_from_stdin()?);
    let requested = vec![
        ActorCapability::ObserveWindows,
        ActorCapability::PublishAccessibilityTree,
        ActorCapability::DispatchAccessibilityAction,
    ];
    let mut client = Client::connect_agent_with_timeout(
        &socket,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
        None,
        AgentHello {
            label: Some("Aegis AT-SPI adapter".into()),
            requested,
            credential,
        },
        Duration::from_secs(120),
    )?;
    client.set_io_timeout(Some(Duration::from_secs(40)))?;
    let accessibility = async_io::block_on(atspi::AccessibilityConnection::new())?;
    let mut state = AdapterState {
        objects: BTreeMap::new(),
        published: BTreeMap::new(),
    };
    let mut next_scan = Instant::now();

    loop {
        if Instant::now() >= next_scan {
            match client.accessibility_windows() {
                Ok(windows) => match async_io::block_on(scan(&accessibility, &windows, &mut state))
                {
                    Ok(updates) => {
                        for update in updates {
                            if let Err(error) = client.publish_accessibility_tree(update) {
                                log::warn!("publish accessibility tree: {error}");
                            }
                        }
                    }
                    Err(error) => log::warn!("scan AT-SPI tree: {error}"),
                },
                Err(error) => log::warn!("query Aegis windows: {error}"),
            }
            next_scan = Instant::now() + SCAN_INTERVAL;
        }

        if let Some(request) = client.next_accessibility_action(ACTION_POLL)? {
            let result = async_io::block_on(execute(&accessibility, &state, &request));
            client.complete_accessibility_action(request.request_id, result)?;
        }
    }
}

fn read_credential_from_stdin() -> Result<String, Box<dyn std::error::Error>> {
    use std::io::{BufRead as _, Read as _};
    let mut credential = String::new();
    std::io::stdin()
        .lock()
        .take(257)
        .read_line(&mut credential)?;
    let credential = credential.trim_end_matches(['\r', '\n']).to_owned();
    if credential.len() < 32
        || credential.len() > 256
        || !credential.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid compositor-issued credential".into());
    }
    Ok(credential)
}

async fn scan(
    accessibility: &atspi::AccessibilityConnection,
    windows: &[AccessibilityWindowBinding],
    state: &mut AdapterState,
) -> Result<Vec<AccessibilityTreeUpdate>, Box<dyn std::error::Error>> {
    let root = accessibility.root_accessible_on_registry().await?;
    let applications = root.get_children().await?;
    let dbus = zbus::fdo::DBusProxy::new(accessibility.connection()).await?;
    let mut matched_windows = BTreeSet::new();
    let mut next_objects = BTreeMap::new();
    let mut next_published = BTreeMap::new();
    let mut updates = Vec::new();

    for application_ref in applications {
        let Some(application_name) = application_ref.name().cloned() else {
            continue;
        };
        let application_pid = match dbus
            .get_connection_unix_process_id(zbus::names::BusName::from(application_name))
            .await
        {
            Ok(pid) => pid,
            Err(error) => {
                log::debug!("skip AT-SPI application without process credentials: {error}");
                continue;
            }
        };
        let application = accessibility.object_as_accessible(&application_ref).await?;
        for top_ref in application.get_children().await.unwrap_or_default() {
            let top = accessibility.object_as_accessible(&top_ref).await?;
            let title = top.name().await.unwrap_or_default();
            let candidates = windows
                .iter()
                .filter(|binding| {
                    !matched_windows.contains(&binding.window.id)
                        && accessibility_binding_matches(binding, application_pid, &title)
                })
                .collect::<Vec<_>>();
            if candidates.len() != 1 {
                continue;
            }
            let window = &candidates[0].window;
            let (nodes, objects, content_hash) = build_tree(accessibility, top_ref, window).await?;
            if nodes.is_empty() {
                continue;
            }
            let revision = match state.published.get(&window.id) {
                Some(published) if published.content_hash == content_hash => {
                    next_objects.extend(objects.into_iter().map(|(id, mut object)| {
                        object.revision = published.revision;
                        ((window.id, id), object)
                    }));
                    matched_windows.insert(window.id);
                    next_published.insert(window.id, published.clone());
                    continue;
                }
                Some(published) => published
                    .revision
                    .checked_add(1)
                    .ok_or("accessibility revision exhausted")?,
                None => 1,
            };
            next_objects.extend(objects.into_iter().map(|(id, mut object)| {
                object.revision = revision;
                ((window.id, id), object)
            }));
            next_published.insert(
                window.id,
                PublishedTree {
                    content_hash,
                    revision,
                },
            );
            matched_windows.insert(window.id);
            updates.push(AccessibilityTreeUpdate {
                window: window.id,
                revision,
                nodes,
            });
        }
    }
    next_published.retain(|window, _| matched_windows.contains(window));
    state.published = next_published;
    state.objects = next_objects;
    Ok(updates)
}

fn accessibility_binding_matches(
    binding: &AccessibilityWindowBinding,
    application_pid: u32,
    title: &str,
) -> bool {
    binding.process_id == application_pid
        && !title.is_empty()
        && binding.window.title.as_deref() == Some(title)
}

async fn build_tree(
    accessibility: &atspi::AccessibilityConnection,
    root: ObjectRefOwned,
    window: &Window,
) -> Result<
    (
        Vec<AccessibilityNode>,
        BTreeMap<u64, CachedAccessible>,
        [u8; 32],
    ),
    Box<dyn std::error::Error>,
> {
    let mut nodes = Vec::new();
    let mut objects = BTreeMap::new();
    let mut private_state = Vec::new();
    let mut queue = VecDeque::from([(root, None, None, 0, 0)]);
    while let Some((object_ref, semantic_parent, object_parent, child_index, depth)) =
        queue.pop_front()
    {
        if nodes.len() >= MAX_NODES || depth > MAX_DEPTH {
            break;
        }
        let accessible = accessibility.object_as_accessible(&object_ref).await?;
        let children = accessible.get_children().await.unwrap_or_default();
        let role = accessible.get_role().await.unwrap_or(Role::Unknown);
        let states = accessible.get_state().await.unwrap_or_default();
        if states.contains(State::Defunct) {
            continue;
        }
        let interfaces = accessible.get_interfaces().await.unwrap_or_default();
        let local_id = object_id(&object_ref);
        if local_id == 0 || objects.contains_key(&local_id) {
            return Err("AT-SPI object id collision".into());
        }
        let bounds = if depth == 0 {
            Some(Rect::new(0, 0, window.size.w, window.size.h))
        } else if interfaces.contains(Interface::Component) {
            component_extents(accessibility.connection(), &object_ref).await
        } else {
            None
        };
        let bounds = bounds.filter(|bounds| valid_bounds(*bounds, window.size));
        let visible = states.contains(State::Visible) && states.contains(State::Showing);
        let include = depth == 0 || (visible && bounds.is_some());
        let next_parent = if include {
            let (value, private_digest, text_actions_safe) =
                semantic_value(accessibility.connection(), &object_ref, role, interfaces).await;
            private_state.push((local_id, private_digest));
            let mut actions = semantic_actions(interfaces, states);
            if !text_actions_safe {
                actions.retain(|action| {
                    !matches!(action, SemanticAction::SetValue | SemanticAction::TypeText)
                });
            }
            let node = AccessibilityNode {
                local_id,
                parent_local_id: semantic_parent,
                role: semantic_role(role),
                name: nonempty(accessible.name().await.unwrap_or_default()),
                description: nonempty(accessible.description().await.unwrap_or_default()),
                value,
                bounds: bounds.expect("the root and included descendants have bounds"),
                state: semantic_state(role, states),
                actions,
            };
            let precondition_hash = precondition_hash(&node, private_digest)?;
            nodes.push(node);
            objects.insert(
                local_id,
                CachedAccessible {
                    object: object_ref.clone(),
                    parent: object_parent.clone(),
                    child_index,
                    revision: 0,
                    window_size: window.size,
                    root: depth == 0,
                    precondition_hash,
                },
            );
            Some(local_id)
        } else {
            semantic_parent
        };
        for (index, child) in children.into_iter().enumerate() {
            queue.push_back((
                child,
                next_parent,
                Some(object_ref.clone()),
                i32::try_from(index)?,
                depth + 1,
            ));
        }
    }
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&nodes)?);
    for (local_id, private_digest) in private_state {
        digest.update(local_id.to_be_bytes());
        digest.update(private_digest);
    }
    Ok((nodes, objects, digest.finalize().into()))
}

/// Return a bounded public value plus a private-only fingerprint used to
/// advance the tree revision. Password content is never requested from
/// AT-SPI or published, and password text actions remain disabled because a
/// polling adapter cannot prove an unchanged same-length secret.
async fn semantic_value(
    connection: &atspi::zbus::Connection,
    object: &ObjectRefOwned,
    role: Role,
    interfaces: atspi::InterfaceSet,
) -> (Option<String>, [u8; 32], bool) {
    if role == Role::PasswordText {
        let count = if interfaces.contains(Interface::Text) {
            match text_proxy(connection, object).await {
                Ok(proxy) => proxy.character_count().await.ok(),
                Err(_) => None,
            }
        } else {
            None
        };
        return (
            None,
            Sha256::digest(format!("password-length:{count:?}").as_bytes()).into(),
            false,
        );
    }
    if interfaces.contains(Interface::Text)
        && let Ok(proxy) = text_proxy(connection, object).await
        && let Ok(count) = proxy.character_count().await
    {
        if (0..=16_384).contains(&count)
            && let Ok(text) = proxy.get_text(0, count).await
        {
            let digest = Sha256::digest(text.as_bytes()).into();
            let bounded = text.len() <= 16_384 && !text.contains('\0');
            return (bounded.then_some(text), digest, bounded);
        }
        return (
            None,
            Sha256::digest(format!("oversized-text-length:{count}").as_bytes()).into(),
            false,
        );
    }
    if interfaces.contains(Interface::Value)
        && let Ok(proxy) = value_proxy(connection, object).await
        && let Ok(value) = proxy.current_value().await
    {
        let value = value.to_string();
        return (
            Some(value.clone()),
            Sha256::digest(value.as_bytes()).into(),
            true,
        );
    }
    (None, [0; 32], true)
}

async fn execute(
    accessibility: &atspi::AccessibilityConnection,
    state: &AdapterState,
    request: &aegis_semantic::SemanticActionRequest,
) -> Result<(), String> {
    let cached = state
        .objects
        .get(&(request.target.window, request.provider_node_id))
        .ok_or_else(|| "AT-SPI target is no longer cached".to_owned())?;
    if cached.revision != request.tree_revision {
        return Err("AT-SPI tree changed after compositor validation".into());
    }
    if live_precondition_hash(accessibility, cached).await? != cached.precondition_hash {
        return Err("AT-SPI target changed immediately before action dispatch".into());
    }
    match &request.action {
        SemanticActionIntent::Invoke => {
            let proxy = action_proxy(accessibility.connection(), &cached.object).await?;
            proxy
                .do_action(0)
                .await
                .map_err(|error| error.to_string())?
                .then_some(())
                .ok_or_else(|| "AT-SPI default action returned false".into())
        }
        SemanticActionIntent::Focus => {
            let proxy = component_proxy(accessibility.connection(), &cached.object).await?;
            proxy
                .grab_focus()
                .await
                .map_err(|error| error.to_string())?
                .then_some(())
                .ok_or_else(|| "AT-SPI focus action returned false".into())
        }
        SemanticActionIntent::SetValue { value } => {
            if let Ok(proxy) = editable_text_proxy(accessibility.connection(), &cached.object).await
            {
                return proxy
                    .set_text_contents(value)
                    .await
                    .map_err(|error| error.to_string())?
                    .then_some(())
                    .ok_or_else(|| "AT-SPI set text returned false".into());
            }
            let number = value
                .parse::<f64>()
                .map_err(|_| "AT-SPI numeric value is invalid".to_owned())?;
            value_proxy(accessibility.connection(), &cached.object)
                .await?
                .set_current_value(number)
                .await
                .map_err(|error| error.to_string())
        }
        SemanticActionIntent::TypeText { text } => {
            let caret = text_proxy(accessibility.connection(), &cached.object)
                .await?
                .caret_offset()
                .await
                .map_err(|error| error.to_string())?;
            editable_text_proxy(accessibility.connection(), &cached.object)
                .await?
                .insert_text(
                    caret,
                    text,
                    i32::try_from(text.chars().count()).map_err(|e| e.to_string())?,
                )
                .await
                .map_err(|error| error.to_string())?
                .then_some(())
                .ok_or_else(|| "AT-SPI insert text returned false".into())
        }
        SemanticActionIntent::Select { selected } => {
            let parent = cached
                .parent
                .as_ref()
                .ok_or_else(|| "AT-SPI selection target has no parent".to_owned())?;
            let proxy = selection_proxy(accessibility.connection(), parent).await?;
            let applied = if *selected {
                proxy.select_child(cached.child_index).await
            } else {
                proxy.deselect_child(cached.child_index).await
            }
            .map_err(|error| error.to_string())?;
            applied
                .then_some(())
                .ok_or_else(|| "AT-SPI selection action returned false".into())
        }
        SemanticActionIntent::Expand | SemanticActionIntent::Collapse => {
            let proxy = action_proxy(accessibility.connection(), &cached.object).await?;
            let needle = if matches!(request.action, SemanticActionIntent::Expand) {
                "expand"
            } else {
                "collapse"
            };
            let index = proxy
                .get_actions()
                .await
                .map_err(|error| error.to_string())?
                .iter()
                .position(|action| action.name.to_ascii_lowercase().contains(needle))
                .ok_or_else(|| format!("AT-SPI target has no {needle} action"))?;
            proxy
                .do_action(i32::try_from(index).map_err(|error| error.to_string())?)
                .await
                .map_err(|error| error.to_string())?
                .then_some(())
                .ok_or_else(|| format!("AT-SPI {needle} action returned false"))
        }
        SemanticActionIntent::SyntheticInput { .. } => {
            Err("synthetic input cannot be dispatched through AT-SPI".into())
        }
    }
}

async fn live_precondition_hash(
    accessibility: &atspi::AccessibilityConnection,
    cached: &CachedAccessible,
) -> Result<[u8; 32], String> {
    let accessible = accessibility
        .object_as_accessible(&cached.object)
        .await
        .map_err(|error| error.to_string())?;
    let role = accessible
        .get_role()
        .await
        .map_err(|error| error.to_string())?;
    let states = accessible
        .get_state()
        .await
        .map_err(|error| error.to_string())?;
    if states.contains(State::Defunct) {
        return Err("AT-SPI target became defunct".into());
    }
    let interfaces = accessible
        .get_interfaces()
        .await
        .map_err(|error| error.to_string())?;
    let bounds = if cached.root {
        Rect::new(0, 0, cached.window_size.w, cached.window_size.h)
    } else {
        component_extents(accessibility.connection(), &cached.object)
            .await
            .filter(|bounds| valid_bounds(*bounds, cached.window_size))
            .ok_or_else(|| "AT-SPI target bounds are no longer valid".to_owned())?
    };
    let visible = states.contains(State::Visible) && states.contains(State::Showing);
    if !cached.root && !visible {
        return Err("AT-SPI target is no longer visible".into());
    }
    let (value, private_digest, text_actions_safe) =
        semantic_value(accessibility.connection(), &cached.object, role, interfaces).await;
    let mut actions = semantic_actions(interfaces, states);
    if !text_actions_safe {
        actions.retain(|action| {
            !matches!(action, SemanticAction::SetValue | SemanticAction::TypeText)
        });
    }
    let node = AccessibilityNode {
        local_id: 1,
        parent_local_id: None,
        role: semantic_role(role),
        name: nonempty(accessible.name().await.unwrap_or_default()),
        description: nonempty(accessible.description().await.unwrap_or_default()),
        value,
        bounds,
        state: semantic_state(role, states),
        actions,
    };
    precondition_hash(&node, private_digest).map_err(|error| error.to_string())
}

fn precondition_hash(
    node: &AccessibilityNode,
    private_digest: [u8; 32],
) -> Result<[u8; 32], serde_json::Error> {
    // Node/parent identifiers express routing, not mutable action state, so
    // normalize them before hashing the fields re-read from the toolkit.
    let mut normalized = node.clone();
    normalized.local_id = 1;
    normalized.parent_local_id = None;
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&normalized)?);
    digest.update(private_digest);
    Ok(digest.finalize().into())
}

fn semantic_state(role: Role, states: atspi::StateSet) -> SemanticState {
    SemanticState {
        visible: states.contains(State::Visible) && states.contains(State::Showing),
        enabled: states.contains(State::Enabled) && states.contains(State::Sensitive),
        focused: states.contains(State::Focused),
        read_only: matches!(role, Role::Entry | Role::PasswordText | Role::Text)
            && !states.contains(State::Editable),
        minimized: states.contains(State::Iconified),
    }
}

fn semantic_actions(
    interfaces: atspi::InterfaceSet,
    states: atspi::StateSet,
) -> Vec<SemanticAction> {
    let mut actions = Vec::new();
    if interfaces.contains(Interface::Action) {
        actions.push(SemanticAction::Invoke);
    }
    if interfaces.contains(Interface::Component) && states.contains(State::Focusable) {
        actions.push(SemanticAction::Focus);
    }
    if interfaces.contains(Interface::EditableText) {
        actions.push(SemanticAction::SetValue);
        actions.push(SemanticAction::TypeText);
    } else if interfaces.contains(Interface::Value) {
        actions.push(SemanticAction::SetValue);
    }
    if states.contains(State::Selectable) {
        actions.push(SemanticAction::Select);
    }
    if states.contains(State::Expandable) {
        if states.contains(State::Expanded) {
            actions.push(SemanticAction::Collapse);
        } else {
            actions.push(SemanticAction::Expand);
        }
    }
    actions.sort();
    actions.dedup();
    actions
}

fn semantic_role(role: Role) -> SemanticRole {
    match role {
        Role::Window | Role::Frame => SemanticRole::Window,
        Role::Dialog | Role::Alert => SemanticRole::Dialog,
        Role::Button | Role::ToggleButton | Role::RadioButton => SemanticRole::Button,
        Role::Entry | Role::PasswordText | Role::Text => SemanticRole::TextField,
        Role::List => SemanticRole::List,
        Role::ListItem => SemanticRole::ListItem,
        Role::DocumentFrame | Role::DocumentWeb | Role::DocumentText | Role::HTMLContainer => {
            SemanticRole::Document
        }
        Role::Image | Role::Icon => SemanticRole::Image,
        Role::CheckBox | Role::CheckMenuItem => SemanticRole::CheckBox,
        Role::ComboBox => SemanticRole::ComboBox,
        Role::Menu | Role::MenuBar | Role::PopupMenu => SemanticRole::Menu,
        Role::MenuItem | Role::RadioMenuItem | Role::TearoffMenuItem => SemanticRole::MenuItem,
        Role::PageTab | Role::PageTabList => SemanticRole::Tab,
        Role::Slider | Role::ScrollBar | Role::SpinButton => SemanticRole::Slider,
        Role::Link => SemanticRole::Link,
        Role::Heading | Role::Header => SemanticRole::Heading,
        Role::Paragraph => SemanticRole::Paragraph,
        _ => SemanticRole::Unknown,
    }
}

fn object_id(object: &ObjectRefOwned) -> u64 {
    let digest = Sha256::digest(
        format!(
            "{}\0{}",
            object.name_as_str().unwrap_or_default(),
            object.path_as_str()
        )
        .as_bytes(),
    );
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes).max(1)
}

fn valid_bounds(bounds: Rect, surface: Size) -> bool {
    bounds.origin.x >= 0
        && bounds.origin.y >= 0
        && bounds.size.w > 0
        && bounds.size.h > 0
        && bounds.origin.x.saturating_add(bounds.size.w) <= surface.w
        && bounds.origin.y.saturating_add(bounds.size.h) <= surface.h
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

async fn component_extents(
    connection: &atspi::zbus::Connection,
    object: &ObjectRefOwned,
) -> Option<Rect> {
    let (x, y, width, height) = component_proxy(connection, object)
        .await
        .ok()?
        .get_extents(CoordType::Window)
        .await
        .ok()?;
    Some(Rect::new(x, y, width, height))
}

macro_rules! proxy_builder {
    ($name:ident, $proxy:ty) => {
        async fn $name<'a>(
            connection: &'a atspi::zbus::Connection,
            object: &ObjectRefOwned,
        ) -> Result<$proxy, String> {
            let destination = object
                .name()
                .cloned()
                .ok_or_else(|| "AT-SPI object has no bus name".to_owned())?;
            <$proxy>::builder(connection)
                .destination(destination)
                .map_err(|error| error.to_string())?
                .path(object.path().clone())
                .map_err(|error| error.to_string())?
                .cache_properties(atspi::zbus::proxy::CacheProperties::No)
                .build()
                .await
                .map_err(|error| error.to_string())
        }
    };
}

proxy_builder!(component_proxy, ComponentProxy<'a>);
proxy_builder!(action_proxy, ActionProxy<'a>);
proxy_builder!(editable_text_proxy, EditableTextProxy<'a>);
proxy_builder!(text_proxy, TextProxy<'a>);
proxy_builder!(value_proxy, ValueProxy<'a>);
proxy_builder!(selection_proxy, SelectionProxy<'a>);

fn socket_path() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("AEGIS_ATSPI_SOCKET") {
        return Ok(path.into());
    }
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .map(|path| path.join("aegis.sock"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessibility_binding_requires_both_kernel_pid_and_exact_title() {
        let binding = AccessibilityWindowBinding {
            window: Window {
                id: WindowId(7),
                title: Some("Checkout".into()),
                ..Window::default()
            },
            process_id: 42,
        };
        assert!(accessibility_binding_matches(&binding, 42, "Checkout"));
        assert!(!accessibility_binding_matches(&binding, 43, "Checkout"));
        assert!(!accessibility_binding_matches(&binding, 42, "Sign in"));
        assert!(!accessibility_binding_matches(&binding, 42, ""));
    }
}
