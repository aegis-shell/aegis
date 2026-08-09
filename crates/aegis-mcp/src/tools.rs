use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aegis_ipc::{
    ActorCapability, Client, Command, ConnectionCapabilities, Effect, InteractionDomainAction,
    InteractionDomainActionResult, JournalMutation, ObservationToken, Scope,
};
use aegis_model::input::SyntheticInputAction;
use aegis_model::interaction_domain::{
    HUMAN_INTERACTION_DOMAIN, InteractionDomainId, InteractionDomainMutation,
    InteractionDomainState,
};
use aegis_model::semantic::{SemanticActionIntent, SemanticObjectId};
use aegis_model::window::WindowId;
use aegis_model::workspace::{LaunchPlacement, Switch, WorkspaceId};
use aegis_model::{Point, Rect};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::BridgeConfig;
use crate::interaction_domain::{
    InteractionDomainSession, InteractionDomainSessionError, ManagedInteractionDomain,
};

const MAX_JOURNAL_ENTRIES: usize = 200;
const MAX_APP_RESULTS: usize = 200;
const MAX_INPUT_ACTIONS: usize = 64;
const MAX_INLINE_MCP_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PENDING_OBSERVATIONS: usize = 64;

struct PendingObservation {
    expires_at: Instant,
    client: Client,
}

/// ConnectionCapabilities and resource/operation allowlists observed at startup.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolGrant {
    pub capabilities: ConnectionCapabilities,
    pub scope: Scope,
}

/// Evidence returned by the live, low-risk compositor smoke test.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SmokeReport {
    pub status: &'static str,
    pub mode: &'static str,
    pub label: String,
    pub notification: SmokeNotificationReport,
    pub interaction_domain: SmokeInteractionDomainReport,
    pub visual: SmokeVisualReport,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SmokeNotificationReport {
    pub started_id: u64,
    pub id: u64,
    pub summary: String,
    pub observed_in_compositor_state: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SmokeInteractionDomainReport {
    pub id: u64,
    pub created_by_smoke: bool,
    pub lifecycle: Vec<String>,
    pub cleanup: &'static str,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SmokeVisualReport {
    pub status_indicator: &'static str,
    pub details_surface: &'static str,
    pub observation_millis: u128,
    pub input_probe: Option<SmokeInputReport>,
}

/// Evidence for the opt-in, non-clicking Agent input smoke probe.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SmokeInputReport {
    pub window_id: u64,
    pub action: &'static str,
    pub local_position: Point,
    pub journal_sequence: u64,
    pub applied: bool,
    pub window_restored_to_human: bool,
}

/// One MCP-advertised tool definition.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub read_only: bool,
    pub destructive: bool,
}

impl ToolDefinition {
    pub fn to_mcp(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "idempotentHint": self.read_only,
                "openWorldHint": false
            }
        })
    }
}

/// Per-request bound for mutation calls that may block on an interactive
/// runtime grant (ADR-0088): the compositor's interaction timeout is 300 s,
/// plus margin.
const GRANT_TIMEOUT: Duration = Duration::from_secs(360);

/// Successful platform call plus optional MCP image content.
#[derive(Debug)]
pub(crate) struct ToolCallResult {
    pub value: Value,
    pub image_png: Option<Vec<u8>>,
}

impl ToolCallResult {
    pub(crate) fn json(value: Value) -> Self {
        Self {
            value,
            image_png: None,
        }
    }
}

/// Scoped Aegis platform service consumed by the MCP transport.
pub struct AegisPlatform {
    config: BridgeConfig,
    grant: ToolGrant,
    interaction_domain: InteractionDomainSession,
    identity: crate::identity::IdentityStore,
    /// Keep the exact authenticated IPC connection that received each
    /// observation alive until `interaction_domain_input` consumes it. Observation leases
    /// are connection-bound by the compositor; opening a fresh connection for
    /// the next MCP tool call would correctly invalidate the token.
    pending_observations: std::collections::BTreeMap<String, PendingObservation>,
}

mod platform;

/// The operation families this bridge can ever request: the catalog the
/// pairing prompt shows and the compositor-approved ceiling is checked
/// against (ADR-0088).
fn catalog_ops() -> Vec<ActorCapability> {
    vec![
        ActorCapability::ObserveWindows,
        ActorCapability::ObserveWorkspaces,
        ActorCapability::ObserveOutputs,
        ActorCapability::ObserveNotifications,
        ActorCapability::ObserveJournal,
        ActorCapability::ObserveInteractionDomains,
        ActorCapability::Focus,
        ActorCapability::Minimize,
        ActorCapability::Close,
        ActorCapability::MoveToWorkspace,
        ActorCapability::SwitchWorkspace,
        ActorCapability::SwitchWorkspaceTo,
        ActorCapability::SetWindowGeometry,
        ActorCapability::ToggleTiling,
        ActorCapability::ToggleOverview,
        ActorCapability::Notify,
        ActorCapability::CreateInteractionDomain,
        ActorCapability::TransactInteractionDomain,
        ActorCapability::RevokeInteractionDomain,
        ActorCapability::CaptureInteractionDomain,
        ActorCapability::CaptureWindow,
        ActorCapability::ObserveInteractionDomain,
        ActorCapability::LaunchInInteractionDomain,
        ActorCapability::LaunchApp,
        ActorCapability::InjectInteractionDomainInput,
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    DesktopSnapshot,
    DesktopJournal,
    AppsList,
    LaunchApp,
    FocusWindow,
    MinimizeWindow,
    CloseWindow,
    MoveWindowToWorkspace,
    SwitchWorkspace,
    SwitchWorkspaceTo,
    SetWindowGeometry,
    ToggleTiling,
    ToggleOverview,
    PostNotification,
    InteractionDomainStatus,
    InteractionDomainEnsure,
    InteractionDomainLaunchApp,
    InteractionDomainTransferWindow,
    InteractionDomainSetState,
    InteractionDomainObserve,
    InteractionDomainCapture,
    InteractionDomainInput,
    InteractionDomainReset,
    WindowCapture,
}

impl ToolKind {
    const ALL: [Self; 24] = [
        Self::DesktopSnapshot,
        Self::DesktopJournal,
        Self::AppsList,
        Self::LaunchApp,
        Self::FocusWindow,
        Self::MinimizeWindow,
        Self::CloseWindow,
        Self::MoveWindowToWorkspace,
        Self::SwitchWorkspace,
        Self::SwitchWorkspaceTo,
        Self::SetWindowGeometry,
        Self::ToggleTiling,
        Self::ToggleOverview,
        Self::PostNotification,
        Self::InteractionDomainStatus,
        Self::InteractionDomainEnsure,
        Self::InteractionDomainLaunchApp,
        Self::InteractionDomainTransferWindow,
        Self::InteractionDomainSetState,
        Self::InteractionDomainObserve,
        Self::InteractionDomainCapture,
        Self::InteractionDomainInput,
        Self::InteractionDomainReset,
        Self::WindowCapture,
    ];

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.definition().name == name)
    }

    fn allowed(self, grant: &ToolGrant) -> bool {
        let observes = |op| {
            grant
                .scope
                .ops
                .as_ref()
                .is_some_and(|operations| operations.contains(&op))
        };
        match self {
            Self::DesktopSnapshot => {
                return grant.capabilities.query
                    && [
                        ActorCapability::ObserveWindows,
                        ActorCapability::ObserveWorkspaces,
                        ActorCapability::ObserveOutputs,
                        ActorCapability::ObserveInteractionDomains,
                    ]
                    .into_iter()
                    .all(observes);
            }
            Self::DesktopJournal => {
                return grant.capabilities.query && observes(ActorCapability::ObserveJournal);
            }
            Self::InteractionDomainStatus => {
                return grant.capabilities.query
                    && observes(ActorCapability::ObserveInteractionDomains);
            }
            Self::AppsList => return grant.capabilities.query,
            _ => {}
        }
        let (capability, op) = match self {
            Self::LaunchApp => (grant.capabilities.control, ActorCapability::LaunchApp),
            Self::FocusWindow => (grant.capabilities.control, ActorCapability::Focus),
            Self::MinimizeWindow => (grant.capabilities.control, ActorCapability::Minimize),
            Self::CloseWindow => (grant.capabilities.control, ActorCapability::Close),
            Self::MoveWindowToWorkspace => {
                (grant.capabilities.control, ActorCapability::MoveToWorkspace)
            }
            Self::SwitchWorkspace => (grant.capabilities.control, ActorCapability::SwitchWorkspace),
            Self::SwitchWorkspaceTo => (
                grant.capabilities.control,
                ActorCapability::SwitchWorkspaceTo,
            ),
            Self::SetWindowGeometry => (
                grant.capabilities.control,
                ActorCapability::SetWindowGeometry,
            ),
            Self::ToggleTiling => (grant.capabilities.control, ActorCapability::ToggleTiling),
            Self::ToggleOverview => (grant.capabilities.control, ActorCapability::ToggleOverview),
            Self::PostNotification => (grant.capabilities.control, ActorCapability::Notify),
            Self::InteractionDomainEnsure => (
                grant.capabilities.interaction_domain,
                ActorCapability::CreateInteractionDomain,
            ),
            Self::InteractionDomainLaunchApp => (
                grant.capabilities.interaction_domain,
                ActorCapability::LaunchInInteractionDomain,
            ),
            Self::InteractionDomainTransferWindow | Self::InteractionDomainSetState => (
                grant.capabilities.interaction_domain,
                ActorCapability::TransactInteractionDomain,
            ),
            Self::InteractionDomainCapture => (
                grant.capabilities.interaction_domain,
                ActorCapability::CaptureInteractionDomain,
            ),
            Self::InteractionDomainObserve => (
                grant.capabilities.query,
                ActorCapability::ObserveInteractionDomain,
            ),
            Self::InteractionDomainInput => (
                grant.capabilities.input,
                ActorCapability::InjectInteractionDomainInput,
            ),
            Self::InteractionDomainReset => (
                grant.capabilities.interaction_domain,
                ActorCapability::RevokeInteractionDomain,
            ),
            Self::WindowCapture => (grant.capabilities.control, ActorCapability::CaptureWindow),
            Self::DesktopSnapshot
            | Self::DesktopJournal
            | Self::AppsList
            | Self::InteractionDomainStatus => {
                unreachable!("query tools returned above")
            }
        };
        let listed =
            |ops: &Option<Vec<ActorCapability>>| ops.as_ref().is_some_and(|ops| ops.contains(&op));
        let requestable = listed(&grant.scope.ops) || listed(&grant.scope.ask_ops);
        capability
            && if matches!(
                self,
                Self::InteractionDomainEnsure
                    | Self::InteractionDomainLaunchApp
                    | Self::InteractionDomainTransferWindow
                    | Self::InteractionDomainSetState
                    | Self::InteractionDomainObserve
                    | Self::InteractionDomainCapture
                    | Self::InteractionDomainInput
                    | Self::InteractionDomainReset
                    | Self::WindowCapture
            ) {
                // Interaction Domain, input, and pixel-capture operations are never
                // inherited from an omitted op allowlist; mirror the compositor's
                // fail-closed rule. Runtime-gated operations stay advertised: calling
                // them asks the user first (ADR-0088).
                requestable
            } else {
                grant.scope.ops.is_none() || requestable
            }
    }

    #[allow(clippy::too_many_lines)]
    fn definition(self) -> ToolDefinition {
        let empty = || json!({"type": "object", "properties": {}, "additionalProperties": false});
        match self {
            Self::DesktopSnapshot => definition(
                "desktop_snapshot",
                "Read current Aegis windows, workspaces, outputs, all Interaction Domains, and this connector's granted scope. Call before addressing desktop objects by id.",
                empty(),
                true,
                false,
            ),
            Self::DesktopJournal => definition(
                "desktop_journal",
                "Read ordered compositor mutations after a sequence number. Use this to verify queued desktop commands.",
                json!({"type":"object","properties":{"since":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":MAX_JOURNAL_ENTRIES}},"additionalProperties":false}),
                true,
                false,
            ),
            Self::AppsList => definition(
                "apps_list",
                "Search the host XDG application catalog. Use the returned desktop_id with launch_app or interaction_domain_launch_app; never invent desktop ids.",
                json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":MAX_APP_RESULTS}},"additionalProperties":false}),
                true,
                false,
            ),
            Self::LaunchApp => definition(
                "launch_app",
                "Launch one catalogued desktop application directly on the desktop (outside any Interaction Domain). Without placement the window opens on the user's current workspace and may take focus. Pass workspace_id to open it on an existing workspace, or new_workspace to open it on a fresh workspace created right after the current one; with either placement the user's view never switches and the window waits on its workspace until the user switches to it. Call apps_list first; never invent desktop ids.",
                json!({"type":"object","properties":{"desktop_id":{"type":"string","minLength":1,"maxLength":512},"workspace_id":{"type":"integer","minimum":1},"new_workspace":{"type":"boolean"},"workspace_label":{"type":"string","maxLength":128}},"required":["desktop_id"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::FocusWindow => definition(
                "focus_window",
                "Queue focus for a live window id. By default the view switches to the window's workspace; pass switch_workspace=false to avoid moving the user's view.",
                json!({"type":"object","properties":{"window_id":{"type":"integer","minimum":1},"switch_workspace":{"type":"boolean","description":"Bring the window's workspace into view before focusing; default true. Set false to activate the window without leaving the current workspace (a hidden window is only raised within its own workspace, never focused)."}},"required":["window_id"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::MinimizeWindow => definition(
                "minimize_window",
                "Queue minimization for a live window id.",
                id_schema("window_id"),
                false,
                false,
            ),
            Self::CloseWindow => definition(
                "close_window",
                "Request that a live window close. Use only when the user explicitly asks to close it.",
                id_schema("window_id"),
                false,
                true,
            ),
            Self::MoveWindowToWorkspace => definition(
                "move_window_to_workspace",
                "Queue moving a window to a workspace by durable ids.",
                json!({"type":"object","properties":{"window_id":{"type":"integer","minimum":1},"workspace_id":{"type":"integer","minimum":1}},"required":["window_id","workspace_id"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::SwitchWorkspace => definition(
                "switch_workspace",
                "Queue switching to the next or previous workspace.",
                json!({"type":"object","properties":{"direction":{"type":"string","enum":["next","previous"]}},"required":["direction"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::SwitchWorkspaceTo => definition(
                "switch_workspace_to",
                "Queue switching to a workspace by durable id.",
                id_schema("workspace_id"),
                false,
                false,
            ),
            Self::SetWindowGeometry => definition(
                "set_window_geometry",
                "Queue floating-window geometry in compositor logical coordinates.",
                json!({"type":"object","properties":{"window_id":{"type":"integer","minimum":1},"x":{"type":"integer"},"y":{"type":"integer"},"width":{"type":"integer","minimum":1},"height":{"type":"integer","minimum":1}},"required":["window_id","x","y","width","height"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::ToggleTiling => definition(
                "toggle_tiling",
                "Toggle the current workspace between tiled and floating layout.",
                empty(),
                false,
                false,
            ),
            Self::ToggleOverview => definition(
                "toggle_overview",
                "Toggle the Aegis workspace overview.",
                empty(),
                false,
                false,
            ),
            Self::PostNotification => definition(
                "post_notification",
                "Post a user-visible notification from the agent.",
                json!({"type":"object","properties":{"summary":{"type":"string","minLength":1},"body":{"type":"string"}},"required":["summary"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::InteractionDomainStatus => definition(
                "interaction_domain_status",
                "Inspect only the Agent Interaction Domain managed by this bridge connector and its controlled or observed interaction groups. Does not create an Interaction Domain.",
                empty(),
                true,
                false,
            ),
            Self::InteractionDomainEnsure => definition(
                "interaction_domain_ensure",
                "Create or recover this connector's private Agent Interaction Domain. The Interaction Domain id is managed internally and is never caller-selected.",
                empty(),
                false,
                false,
            ),
            Self::InteractionDomainLaunchApp => definition(
                "interaction_domain_launch_app",
                "Launch one catalogued desktop application inside the bridge-managed private Agent Interaction Domain and sandbox. Call apps_list first.",
                json!({"type":"object","properties":{"desktop_id":{"type":"string","minLength":1,"maxLength":512}},"required":["desktop_id"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::InteractionDomainTransferWindow => definition(
                "interaction_domain_transfer_window",
                "Atomically transfer interaction authority for a window into the Agent Interaction Domain or back to the human Interaction Domain. Human observation is retained by default when transferring to the Agent Interaction Domain.",
                json!({"type":"object","properties":{"window_id":{"type":"integer","minimum":1},"target":{"type":"string","enum":["agent","human"]},"retain_source_as_observer":{"type":"boolean"}},"required":["window_id","target"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::InteractionDomainSetState => definition(
                "interaction_domain_set_state",
                "Pause or resume the bridge-managed Interaction Domain using an optimistic Interaction Domain transaction.",
                json!({"type":"object","properties":{"state":{"type":"string","enum":["active","paused"]}},"required":["state"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::InteractionDomainObserve => definition(
                "interaction_domain_observe",
                "Read compositor-owned semantic objects for the Agent Interaction Domain without receiving pixels. Returns a short-lived, single-use observation token for interaction_domain_input.",
                empty(),
                true,
                false,
            ),
            Self::InteractionDomainCapture => definition(
                "interaction_domain_capture",
                "Capture only the Agent Interaction Domain's directed virtual output. Returns layout metadata, an owner-only PNG path, and an attached image when it is within the inline limit; never captures compositor chrome or another Interaction Domain.",
                json!({"type":"object","properties":{"region":{"type":"object","properties":{"x":{"type":"integer"},"y":{"type":"integer"},"width":{"type":"integer","minimum":1},"height":{"type":"integer","minimum":1}},"required":["x","y","width","height"],"additionalProperties":false}},"additionalProperties":false}),
                false,
                false,
            ),
            Self::InteractionDomainInput => definition(
                "interaction_domain_input",
                "Atomically validate and commit a bounded target-local input batch through the Agent Interaction Domain's independent seat. Pass the single-use observation token returned by interaction_domain_observe or interaction_domain_capture; state changes abort instead of clicking stale coordinates.",
                input_schema(),
                false,
                false,
            ),
            Self::InteractionDomainReset => definition(
                "interaction_domain_reset",
                "Permanently revoke the bridge-managed Interaction Domain and atomically return its controlled windows to the human Interaction Domain. Use only when the user explicitly requests reset or shutdown.",
                empty(),
                false,
                true,
            ),
            Self::WindowCapture => definition(
                "window_capture",
                "Capture one window's real content by id, on any workspace and whether visible, occluded, or minimized; popups extending past the toplevel bounds are clipped. Returns geometry metadata, an owner-only PNG path, and an attached image when it is within the inline limit. First use asks the user for a runtime grant. Get window ids from desktop_snapshot.",
                id_schema("window_id"),
                false,
                false,
            ),
        }
    }
}

fn definition(
    name: &'static str,
    description: &'static str,
    input_schema: Value,
    read_only: bool,
    destructive: bool,
) -> ToolDefinition {
    ToolDefinition {
        name,
        description,
        input_schema,
        read_only,
        destructive,
    }
}

fn id_schema(name: &str) -> Value {
    json!({
        "type": "object",
        "properties": {name: {"type": "integer", "minimum": 1}},
        "required": [name],
        "additionalProperties": false
    })
}

fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "target_window_id": {"type": "integer", "minimum": 1, "description":"Owning window id from semantic.id.window"},
            "target_local_id": {"type": "integer", "minimum": 0, "description":"Window-scoped semantic id; 0 denotes the window root"},
            "observation_token": {"type": "string", "minLength": 32, "maxLength": 128},
            "actions": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_INPUT_ACTIONS,
                "items": {
                    "oneOf": [
                        {"type":"object","properties":{"type":{"const":"invoke"}},"required":["type"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"focus"}},"required":["type"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"set_value"},"value":{"type":"string","maxLength":16384}},"required":["type","value"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"type_text"},"text":{"type":"string","maxLength":16384}},"required":["type","text"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"select"},"selected":{"type":"boolean"}},"required":["type","selected"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"expand"}},"required":["type"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"collapse"}},"required":["type"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"pointer_move"},"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0}},"required":["type","x","y"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"click"},"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"button":{"type":"string","enum":["left","right","middle","side","extra"]}},"required":["type","x","y","button"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"scroll"},"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"dx":{"type":"number","minimum":-1000,"maximum":1000},"dy":{"type":"number","minimum":-1000,"maximum":1000}},"required":["type","x","y","dx","dy"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"key_press"},"code":{"type":"integer","minimum":0,"maximum":767,"description":"Linux evdev key code"}},"required":["type","code"],"additionalProperties":false}
                    ]
                }
            }
        },
        "required": ["target_window_id", "observation_token", "actions"],
        "additionalProperties": false
    })
}

fn parse<T: DeserializeOwned>(arguments: Value) -> Result<T, PlatformError> {
    let arguments = if arguments.is_null() {
        json!({})
    } else {
        arguments
    };
    serde_json::from_value(arguments).map_err(|source| invalid(source.to_string()))
}

fn invalid(message: impl Into<String>) -> PlatformError {
    PlatformError::InvalidArguments(message.into())
}

fn window_id(id: u64) -> Result<WindowId, PlatformError> {
    (id != 0)
        .then_some(WindowId(id))
        .ok_or_else(|| invalid("window_id must be greater than zero"))
}

fn workspace_id(id: u64) -> Result<WorkspaceId, PlatformError> {
    (id != 0)
        .then_some(WorkspaceId(id))
        .ok_or_else(|| invalid("workspace_id must be greater than zero"))
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> Result<Rect, PlatformError> {
    if width <= 0 || height <= 0 {
        return Err(invalid("width and height must be positive"));
    }
    Ok(Rect::new(x, y, width, height))
}

fn window_command(
    arguments: Value,
    command: impl FnOnce(WindowId) -> Command,
) -> Result<Command, PlatformError> {
    let args: WindowArgs = parse(arguments)?;
    Ok(command(window_id(args.window_id)?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalArgs {
    #[serde(default)]
    since: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppsArgs {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowArgs {
    window_id: u64,
    #[serde(default)]
    switch_workspace: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchAppArgs {
    desktop_id: String,
    #[serde(default)]
    workspace_id: Option<u64>,
    #[serde(default)]
    new_workspace: Option<bool>,
    #[serde(default)]
    workspace_label: Option<String>,
}

/// Build the `focus_window` command; `switch_workspace` defaults to reveal.
fn focus_command(args: WindowArgs) -> Result<Command, PlatformError> {
    Ok(Command::Focus {
        id: window_id(args.window_id)?,
        reveal: args.switch_workspace.unwrap_or(true),
    })
}

/// Build the `launch_app` command from validated args (ADR-0118). A
/// placement never switches the user's view.
fn launch_app_command(args: LaunchAppArgs) -> Result<Command, PlatformError> {
    let desktop_id = args.desktop_id.trim();
    if desktop_id.is_empty() {
        return Err(invalid("desktop_id must not be empty"));
    }
    if desktop_id.len() > 512 {
        return Err(invalid("desktop_id must be at most 512 bytes"));
    }
    if args.workspace_id.is_some()
        && (args.new_workspace == Some(true) || args.workspace_label.is_some())
    {
        return Err(invalid(
            "workspace_id cannot be combined with new_workspace or workspace_label",
        ));
    }
    let label = match args.workspace_label {
        Some(label) => {
            if args.new_workspace != Some(true) {
                return Err(invalid("workspace_label requires new_workspace"));
            }
            let label = label.trim();
            if label.is_empty() {
                return Err(invalid("workspace_label must not be empty"));
            }
            if label.len() > 128 {
                return Err(invalid("workspace_label must be at most 128 bytes"));
            }
            if label.contains('\0') {
                return Err(invalid("workspace_label must not contain NUL bytes"));
            }
            Some(label.to_string())
        }
        None => None,
    };
    let placement = if let Some(id) = args.workspace_id {
        Some(LaunchPlacement::Workspace {
            id: workspace_id(id)?,
        })
    } else if args.new_workspace == Some(true) {
        Some(LaunchPlacement::FreshWorkspace { label })
    } else {
        None
    };
    Ok(Command::LaunchApp {
        desktop_id: desktop_id.to_string(),
        placement,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceArgs {
    workspace_id: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    window_id: u64,
    workspace_id: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectionArgs {
    direction: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GeometryArgs {
    window_id: u64,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NotificationArgs {
    summary: String,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchArgs {
    desktop_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferArgs {
    window_id: u64,
    target: String,
    #[serde(default)]
    retain_source_as_observer: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateArgs {
    state: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureArgs {
    #[serde(default)]
    region: Option<RegionArgs>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegionArgs {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl TryFrom<RegionArgs> for Rect {
    type Error = PlatformError;

    fn try_from(value: RegionArgs) -> Result<Self, Self::Error> {
        rect(value.x, value.y, value.width, value.height)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputArgs {
    target_window_id: u64,
    #[serde(default)]
    target_local_id: u64,
    observation_token: String,
    actions: Vec<InputActionArgs>,
}

fn semantic_object_id(window: u64, local: u64) -> Result<SemanticObjectId, PlatformError> {
    let window = (window != 0).then_some(window).ok_or_else(|| {
        invalid("semantic target_window_id must identify a non-zero owning window")
    })?;
    Ok(SemanticObjectId {
        window: aegis_model::window::WindowId(window),
        local,
    })
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InputActionArgs {
    Invoke,
    Focus,
    SetValue { value: String },
    TypeText { text: String },
    Select { selected: bool },
    Expand,
    Collapse,
    PointerMove { x: i32, y: i32 },
    Click { x: i32, y: i32, button: String },
    Scroll { x: i32, y: i32, dx: f32, dy: f32 },
    KeyPress { code: u32 },
}

fn semantic_action(value: InputActionArgs) -> Result<SemanticActionIntent, PlatformError> {
    Ok(match value {
        InputActionArgs::Invoke => SemanticActionIntent::Invoke,
        InputActionArgs::Focus => SemanticActionIntent::Focus,
        InputActionArgs::SetValue { value } => SemanticActionIntent::SetValue { value },
        InputActionArgs::TypeText { text } => SemanticActionIntent::TypeText { text },
        InputActionArgs::Select { selected } => SemanticActionIntent::Select { selected },
        InputActionArgs::Expand => SemanticActionIntent::Expand,
        InputActionArgs::Collapse => SemanticActionIntent::Collapse,
        synthetic => SemanticActionIntent::SyntheticInput {
            actions: vec![SyntheticInputAction::try_from(synthetic)?],
        },
    })
}

impl TryFrom<InputActionArgs> for SyntheticInputAction {
    type Error = PlatformError;

    fn try_from(value: InputActionArgs) -> Result<Self, Self::Error> {
        let position = |x, y| {
            if x < 0 || y < 0 {
                Err(invalid("input coordinates must be non-negative"))
            } else {
                Ok(Point { x, y })
            }
        };
        match value {
            InputActionArgs::PointerMove { x, y } => Ok(Self::PointerMove {
                position: position(x, y)?,
            }),
            InputActionArgs::Click { x, y, button } => {
                let button = match button.as_str() {
                    "left" => 0x110,
                    "right" => 0x111,
                    "middle" => 0x112,
                    "side" => 0x113,
                    "extra" => 0x114,
                    _ => {
                        return Err(invalid(
                            "button must be left, right, middle, side, or extra",
                        ));
                    }
                };
                Ok(Self::Click {
                    position: position(x, y)?,
                    button,
                })
            }
            InputActionArgs::Scroll { x, y, dx, dy } => {
                if !dx.is_finite() || !dy.is_finite() || dx.abs() > 1_000.0 || dy.abs() > 1_000.0 {
                    return Err(invalid("scroll deltas must be finite and within ±1000"));
                }
                Ok(Self::Scroll {
                    position: position(x, y)?,
                    dx,
                    dy,
                })
            }
            InputActionArgs::KeyPress { code } => {
                if code > 0x2ff {
                    return Err(invalid("evdev key code must be at most 767"));
                }
                Ok(Self::KeyPress { code })
            }
            _ => Err(invalid("semantic action is not a synthetic input fallback")),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("cannot connect to Aegis IPC socket {socket:?} as agent {label:?}: {source}")]
    Connect {
        socket: PathBuf,
        label: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Aegis IPC operation failed: {0}")]
    Ipc(#[from] std::io::Error),
    #[error("invalid tool arguments: {0}")]
    InvalidArguments(String),
    #[error("unknown tool {0:?}")]
    UnknownTool(String),
    #[error("tool {0:?} is not present in the compositor's granted named scope")]
    NotGranted(String),
    #[error("the managed Agent Interaction Domain does not exist")]
    NoManagedInteractionDomain,
    #[error(
        "the observation token is unknown, expired, already consumed, or belongs to another MCP process"
    )]
    UnknownObservation,
    #[error(
        "creating the managed Agent Interaction Domain requires CreateInteractionDomain in the named scope"
    )]
    InteractionDomainCreationNotGranted,
    #[error(
        "graceful Interaction Domain cleanup requires RevokeInteractionDomain in the named scope"
    )]
    InteractionDomainCleanupNotGranted,
    #[error("Aegis returned an unexpected Interaction Domain action response")]
    UnexpectedResponse,
    #[error("the compositor handshake did not bind an authenticated agent identity")]
    MissingAuthenticatedIdentity,
    #[error("agent identity continuity failed: {0}")]
    Identity(String),
    #[error("live smoke verification failed: {0}")]
    SmokeVerification(String),
    #[error(transparent)]
    Config(#[from] crate::ConfigError),
    #[error("managed Interaction Domain lifecycle failed: {0}")]
    InteractionDomain(String),
}

impl From<InteractionDomainSessionError> for PlatformError {
    fn from(error: InteractionDomainSessionError) -> Self {
        Self::InteractionDomain(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(ops: Option<Vec<ActorCapability>>) -> ToolGrant {
        ToolGrant {
            capabilities: ConnectionCapabilities {
                query: true,
                control: true,
                input: true,
                session: false,
                interaction_domain: true,
            },
            scope: Scope {
                ops,
                ..Scope::default()
            },
        }
    }

    #[test]
    fn interaction_domain_tools_require_explicit_high_risk_operations() {
        let unscoped = grant(None);
        assert!(ToolKind::FocusWindow.allowed(&unscoped));
        assert!(ToolKind::LaunchApp.allowed(&unscoped));
        assert!(!ToolKind::InteractionDomainObserve.allowed(&unscoped));
        assert!(!ToolKind::InteractionDomainCapture.allowed(&unscoped));
        assert!(!ToolKind::InteractionDomainInput.allowed(&unscoped));
        assert!(!ToolKind::WindowCapture.allowed(&unscoped));

        let scoped = grant(Some(vec![
            ActorCapability::ObserveInteractionDomain,
            ActorCapability::CaptureInteractionDomain,
            ActorCapability::InjectInteractionDomainInput,
        ]));
        assert!(ToolKind::InteractionDomainObserve.allowed(&scoped));
        assert!(ToolKind::InteractionDomainCapture.allowed(&scoped));
        assert!(ToolKind::InteractionDomainInput.allowed(&scoped));
        assert!(!ToolKind::InteractionDomainReset.allowed(&scoped));
        assert!(!ToolKind::WindowCapture.allowed(&scoped));
        assert!(!ToolKind::LaunchApp.allowed(&scoped));

        let launch = grant(Some(vec![ActorCapability::LaunchApp]));
        assert!(ToolKind::LaunchApp.allowed(&launch));

        let mut no_control = grant(None);
        no_control.capabilities.control = false;
        assert!(!ToolKind::LaunchApp.allowed(&no_control));
        assert!(!ToolKind::FocusWindow.allowed(&no_control));

        let window_capture = grant(Some(vec![ActorCapability::CaptureWindow]));
        assert!(ToolKind::WindowCapture.allowed(&window_capture));

        let askable = ToolGrant {
            scope: Scope {
                ask_ops: Some(vec![ActorCapability::CaptureWindow]),
                ..Scope::default()
            },
            ..window_capture.clone()
        };
        assert!(ToolKind::WindowCapture.allowed(&askable));
    }

    #[test]
    fn input_translation_is_bounded_and_semantic() {
        let action = InputActionArgs::Click {
            x: 10,
            y: 20,
            button: "left".into(),
        };
        assert_eq!(
            SyntheticInputAction::try_from(action).expect("action"),
            SyntheticInputAction::Click {
                position: Point { x: 10, y: 20 },
                button: 0x110
            }
        );
        assert!(
            SyntheticInputAction::try_from(InputActionArgs::PointerMove { x: -1, y: 0 }).is_err()
        );
    }

    #[test]
    fn launch_app_arguments_validate_placement() {
        let args = LaunchAppArgs {
            desktop_id: "org.example.App.desktop".into(),
            workspace_id: None,
            new_workspace: None,
            workspace_label: None,
        };
        assert_eq!(
            launch_app_command(args).expect("command"),
            Command::LaunchApp {
                desktop_id: "org.example.App.desktop".into(),
                placement: None,
            }
        );

        let args = LaunchAppArgs {
            desktop_id: "org.example.App.desktop".into(),
            workspace_id: Some(3),
            new_workspace: None,
            workspace_label: None,
        };
        assert_eq!(
            launch_app_command(args).expect("command"),
            Command::LaunchApp {
                desktop_id: "org.example.App.desktop".into(),
                placement: Some(LaunchPlacement::Workspace { id: WorkspaceId(3) }),
            }
        );

        let args = LaunchAppArgs {
            desktop_id: "org.example.App.desktop".into(),
            workspace_id: None,
            new_workspace: Some(true),
            workspace_label: Some(" agent run ".into()),
        };
        assert_eq!(
            launch_app_command(args).expect("command"),
            Command::LaunchApp {
                desktop_id: "org.example.App.desktop".into(),
                placement: Some(LaunchPlacement::FreshWorkspace {
                    label: Some("agent run".into())
                }),
            }
        );

        let mut args = LaunchAppArgs {
            desktop_id: "org.example.App.desktop".into(),
            workspace_id: Some(3),
            new_workspace: Some(true),
            workspace_label: None,
        };
        assert!(launch_app_command(args).is_err());
        args = LaunchAppArgs {
            desktop_id: "org.example.App.desktop".into(),
            workspace_id: Some(3),
            new_workspace: None,
            workspace_label: Some("agent run".into()),
        };
        assert!(launch_app_command(args).is_err());
        args = LaunchAppArgs {
            desktop_id: "org.example.App.desktop".into(),
            workspace_id: None,
            new_workspace: None,
            workspace_label: Some("agent run".into()),
        };
        assert!(launch_app_command(args).is_err());
        args = LaunchAppArgs {
            desktop_id: "  ".into(),
            workspace_id: None,
            new_workspace: None,
            workspace_label: None,
        };
        assert!(launch_app_command(args).is_err());
        args = LaunchAppArgs {
            desktop_id: "org.example.App.desktop".into(),
            workspace_id: None,
            new_workspace: Some(true),
            workspace_label: Some("bad\0label".into()),
        };
        assert!(launch_app_command(args).is_err());
    }

    #[test]
    fn focus_window_switch_workspace_defaults_to_reveal() {
        let args = WindowArgs {
            window_id: 7,
            switch_workspace: None,
        };
        assert_eq!(
            focus_command(args).expect("command"),
            Command::Focus {
                id: WindowId(7),
                reveal: true,
            }
        );
        let args = WindowArgs {
            window_id: 7,
            switch_workspace: Some(false),
        };
        assert_eq!(
            focus_command(args).expect("command"),
            Command::Focus {
                id: WindowId(7),
                reveal: false,
            }
        );
    }

    #[test]
    fn tool_names_are_connector_local_and_stable() {
        let names = ToolKind::ALL
            .iter()
            .map(|kind| kind.definition().name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"interaction_domain_capture"));
        assert!(names.contains(&"interaction_domain_observe"));
        assert!(names.contains(&"window_capture"));
        assert!(names.contains(&"apps_list"));
        assert!(names.contains(&"launch_app"));
        assert_eq!(names.len(), 24);
    }

    #[test]
    fn legacy_realm_tool_names_are_rejected_and_not_advertised() {
        assert_eq!(ToolKind::from_name("realm_observe"), None);
        let names = ToolKind::ALL
            .iter()
            .map(|kind| kind.definition().name)
            .collect::<Vec<_>>();
        assert!(!names.iter().any(|name| name.starts_with("realm_")));
    }

    #[test]
    fn id_schema_uses_the_requested_property_name() {
        let schema = id_schema("window_id");
        assert!(schema["properties"].get("window_id").is_some());
        assert!(schema["properties"].get("name").is_none());
    }
}
