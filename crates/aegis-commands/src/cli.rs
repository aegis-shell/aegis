//! Domain-oriented command structure parsed by `clap` derive.
//!
//! Defining the surface here lets `clap` generate help, version, shell
//! completions, and per-subcommand usage. The dispatcher in [`super`]
//! matches on [`Command`] / [`InteractionDomainCmd`] instead of raw strings.

use std::path::PathBuf;
use std::str::FromStr;

use aegis_model::Rect;

/// Aegis compositor and session management.
#[derive(Debug, clap::Parser)]
#[command(
    name = "aegis",
    version,
    about = "Aegis compositor and session management",
    long_about = None,
)]
pub struct Cli {
    /// Emit query results and receipts as JSON instead of human-readable text.
    #[arg(short, long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// Whether this invocation starts the compositor rather than contacting a
    /// running session.
    pub fn runs_compositor(&self) -> bool {
        self.command
            .as_ref()
            .is_none_or(|command| matches!(command, Command::Run))
    }
}

/// Top-level domains. Transport details stay internal: users address displays,
/// windows, workspaces, notifications, Interaction Domains, and session services directly.
#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Start the compositor explicitly.
    Run,
    /// Inspect displays and capture the focused display; lists by default.
    Display {
        #[command(subcommand)]
        command: Option<DisplayCmd>,
    },
    /// Inspect and control windows; lists by default.
    Window {
        #[command(subcommand)]
        command: Option<WindowCmd>,
    },
    /// Inspect and control workspaces; lists by default.
    Workspace {
        #[command(subcommand)]
        command: Option<WorkspaceCmd>,
    },
    /// Inspect and post notifications; lists by default.
    Notification {
        #[command(subcommand)]
        command: Option<NotificationCmd>,
    },
    /// Inspect or follow the mutation journal; lists by default.
    Journal {
        #[command(subcommand)]
        command: Option<JournalCmd>,
    },
    /// Manage Agent Workspaces and Interaction Domain authority; lists by default.
    #[command(name = "interaction-domain")]
    InteractionDomain {
        #[command(subcommand)]
        command: Option<InteractionDomainCmd>,
    },
    /// Manage agent permissions and grants; lists by default.
    Permissions {
        #[command(subcommand)]
        command: Option<PermissionsCmd>,
    },
    /// Validate or explicitly migrate the local Aegis configuration.
    Config {
        #[command(subcommand)]
        command: Option<ConfigCmd>,
    },
    /// Inspect and manage the durable security-audit history.
    Audit {
        #[command(subcommand)]
        command: Option<AuditCmd>,
    },
    /// Inspect and control immediate live-system state; shows status by default.
    System {
        #[command(subcommand)]
        command: Option<SystemCmd>,
    },
    /// Toggle the window/workspace overview.
    Overview,
    /// Stream coarse compositor state-change events until disconnected.
    Events,
    /// Print shell completions for the given shell to stdout.
    Completions { shell: clap_complete::Shell },
    /// Request compositor shutdown.
    Quit,
}

/// Local configuration operations. They never contact a running compositor.
#[derive(Debug, clap::Subcommand)]
pub enum ConfigCmd {
    /// Validate the current schema and every semantic configuration invariant.
    Validate {
        /// Configuration file; defaults to the XDG Aegis config path.
        path: Option<PathBuf>,
    },
    /// Migrate a supported legacy schema after writing a durable backup.
    Migrate {
        /// Configuration file; defaults to the XDG Aegis config path.
        path: Option<PathBuf>,
    },
}

/// Local durable-audit operations (ADR-0137). They never contact a running
/// compositor; a live compositor holds the store's advisory lock, so run
/// these while the session is stopped.
#[derive(Debug, clap::Subcommand)]
pub enum AuditCmd {
    /// Summarize sealed segments, sizes, and retention state.
    Status,
    /// Verify sealed segments against the authenticated manifest.
    Verify {
        /// Decompress every segment and replay the complete hash chain
        /// instead of the fast compressed-digest check.
        #[arg(long)]
        full: bool,
    },
    /// Record an export acknowledgement in the manifest.
    Export {
        /// Free-form destination description (for example an archive path).
        destination: String,
    },
    /// Delete old sealed segments, keeping at most `keep` (the newest).
    /// Requires every removed segment to be export-acknowledged unless
    /// `--force` is given.
    Prune {
        /// Number of newest sealed segments to keep.
        keep: usize,
        /// Delete without an export acknowledgement (destructive).
        #[arg(long)]
        force: bool,
    },
}

/// Commands grouped under `aegis display`. With no command, displays are
/// listed, matching the inspect-first behavior of every resource domain.
#[derive(Debug, clap::Subcommand)]
pub enum DisplayCmd {
    /// List display modes, scales, transforms, and logical sizes.
    List,
    /// Capture the focused display to a PNG file.
    Capture {
        /// Destination path. Defaults to a timestamped PNG in the screenshot directory.
        path: Option<String>,
        /// Capture a region instead of the full display, formatted `x,y,w,h`.
        #[arg(long, value_name = "X,Y,W,H")]
        region: Option<Region>,
    },
}

/// Commands grouped under `aegis window`. With no command, visible windows
/// are listed.
#[derive(Debug, clap::Subcommand)]
pub enum WindowCmd {
    /// List visible windows.
    List,
    /// Focus and raise a window by id.
    Focus { id: u64 },
    /// Minimize a window by id.
    Minimize { id: u64 },
    /// Set or clear always-on-top for a window by id.
    AlwaysOnTop { id: u64, state: OnOff },
    /// Set or clear fullscreen for a window by id.
    Fullscreen { id: u64, state: OnOff },
    /// Request that a window close.
    Close { id: u64 },
    /// Set floating-window geometry in compositor logical pixels.
    #[command(allow_hyphen_values = true)]
    Geometry {
        id: u64,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
}

/// Commands grouped under `aegis workspace`. With no command, workspaces are
/// listed.
#[derive(Debug, clap::Subcommand)]
pub enum WorkspaceCmd {
    /// List displays, their workspaces, and window counts.
    List,
    /// Switch to `next`, `prev`, `previous`, or a workspace id.
    Switch { target: WorkspaceTarget },
    /// Move a window to a workspace id.
    MoveWindow { window: u64, workspace: u64 },
}

/// Commands grouped under `aegis notification`. With no command, active
/// notifications are listed.
#[derive(Debug, clap::Subcommand)]
pub enum NotificationCmd {
    /// List active notifications.
    List,
    /// Post a notification.
    Send {
        summary: String,
        body: Option<String>,
    },
    /// Dismiss an active notification by id.
    Dismiss { id: u64 },
}

/// Mutation-journal commands. With no command, the complete retained journal
/// is listed.
#[derive(Debug, clap::Subcommand)]
pub enum JournalCmd {
    /// List retained entries after an optional sequence number.
    List {
        #[arg(long)]
        since: Option<u64>,
    },
    /// Stream detailed mutation-journal events until disconnected.
    Follow,
}

/// Subcommands grouped under `aegis system`.
#[derive(Debug, clap::Subcommand)]
pub enum SystemCmd {
    /// Show the normalized live-system snapshot.
    Status,
    /// Toggle mute on the default audio sink.
    Mute,
    /// Adjust the default audio sink by a signed percentage step.
    #[command(allow_hyphen_values = true)]
    StepVolume {
        #[arg(value_parser = clap::value_parser!(i8).range(-100..=100))]
        delta: i8,
    },
    /// Set the default audio-sink volume percentage.
    Volume {
        #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
        level: u8,
    },
    /// Set the backlight percentage.
    Brightness {
        #[arg(value_parser = clap::value_parser!(u8).range(1..=100))]
        level: u8,
    },
    /// Enable or disable the Wi-Fi radio.
    Wifi { state: OnOff },
    /// Enable or disable Bluetooth radios.
    Bluetooth { state: OnOff },
    /// Enable or disable notification suppression.
    DoNotDisturb { state: OnOff },
    /// Select the session power mode: balanced, awake, or secure.
    PowerMode { mode: PowerModeArg },
    /// Suspend the host system.
    Suspend,
    /// Reboot the host system.
    Reboot,
    /// Power off / shut down the host system.
    PowerOff,
}

/// The session power mode (ADR-0140), spelled as on the wire.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum PowerModeArg {
    /// Full staged policy: dim, lock, display-off, suspend.
    Balanced,
    /// Keep the session awake and unlocked: only dimming stays armed.
    Awake,
    /// Lock on schedule but never blank the display.
    Secure,
}

impl From<PowerModeArg> for aegis_model::power::PowerMode {
    fn from(value: PowerModeArg) -> Self {
        match value {
            PowerModeArg::Balanced => aegis_model::power::PowerMode::Balanced,
            PowerModeArg::Awake => aegis_model::power::PowerMode::Awake,
            PowerModeArg::Secure => aegis_model::power::PowerMode::Secure,
        }
    }
}

/// Subcommands grouped under `aegis interaction-domain`. They all share the
/// owner-only admin scope and lease negotiation in the dispatcher.
#[derive(Debug, clap::Subcommand)]
pub enum InteractionDomainCmd {
    /// List the authority revision, Interaction Domains, states, seats, and controlled-window counts.
    List,
    /// Create an active agent Interaction Domain with a virtual output and pointer/keyboard seat.
    Create {
        /// Human-readable label for the new Interaction Domain.
        #[arg(default_value = "Agent Workspace")]
        label: String,
    },
    /// Atomically pause an Interaction Domain and freeze its managed cgroups.
    Pause { interaction_domain: u64 },
    /// Resume a paused Interaction Domain and its managed cgroups.
    Resume { interaction_domain: u64 },
    /// Transfer the window's complete interaction group to another Interaction Domain.
    Transfer {
        window: u64,
        interaction_domain: u64,
        /// Do not retain the source Interaction Domain as a read-only observer.
        #[arg(long)]
        no_mirror: bool,
    },
    /// Launch an enumerated desktop entry through a private mount-scoped portal.
    Launch {
        interaction_domain: u64,
        desktop_id: String,
    },
    /// Capture the Interaction Domain's directed virtual output atomically.
    Capture {
        interaction_domain: u64,
        /// Destination path. Defaults to a timestamped PNG in the screenshot directory.
        path: Option<String>,
        /// Capture a region instead of the full virtual output, formatted `x,y,w,h`.
        #[arg(long, value_name = "X,Y,W,H")]
        region: Option<Region>,
    },
    /// Permanently revoke an Interaction Domain and return controlled groups to `fallback`.
    Revoke {
        interaction_domain: u64,
        /// Interaction Domain id to receive the revoked domain's interaction groups.
        #[arg(default_value = "1")]
        fallback: u64,
    },
}

/// Subcommands grouped under `aegis permissions`: agent principals,
/// ceilings, and recorded runtime grants (ADR-0088).
#[derive(Debug, clap::Subcommand)]
pub enum PermissionsCmd {
    /// List paired principals, their ceilings, and their recorded grants.
    List,
    /// Drop one recorded runtime grant; the next use asks the user again.
    Revoke {
        principal: String,
        #[arg(value_parser = op_class)]
        op: aegis_ipc::ActorCapability,
    },
    /// Forget a principal: its credential dies and its grants are dropped.
    Forget { principal: String },
    /// Rename a principal's display label (omit the label to clear it).
    Rename {
        principal: String,
        label: Option<String>,
    },
    /// Replace a principal's approved ceiling.
    SetCeiling {
        principal: String,
        /// Operations usable immediately (comma-separated names).
        #[arg(long, value_delimiter = ',', value_parser = op_class)]
        pregrant: Vec<aegis_ipc::ActorCapability>,
        /// Operations gated by the interactive runtime grant.
        #[arg(long, value_delimiter = ',', value_parser = op_class)]
        gated: Vec<aegis_ipc::ActorCapability>,
    },
    /// Register a principal ahead of time (administrator pre-provisioning)
    /// and print the credential to plant in the agent's identity store.
    Register {
        label: Option<String>,
        /// Operations usable immediately (comma-separated names).
        #[arg(long, value_delimiter = ',', value_parser = op_class)]
        pregrant: Vec<aegis_ipc::ActorCapability>,
        /// Operations gated by the interactive runtime grant.
        #[arg(long, value_delimiter = ',', value_parser = op_class)]
        gated: Vec<aegis_ipc::ActorCapability>,
    },
}

/// Parse an operation-family name for `permissions` arguments.
fn op_class(value: &str) -> Result<aegis_ipc::ActorCapability, String> {
    aegis_ipc::ActorCapability::from_name(value)
        .ok_or_else(|| format!("unknown operation '{value}'"))
}

/// A workspace switch target: an adjacent direction or a concrete id.
#[derive(Debug, Clone, Copy)]
pub enum WorkspaceTarget {
    Next,
    Previous,
    Id(u64),
}

impl FromStr for WorkspaceTarget {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "next" => Ok(Self::Next),
            "prev" | "previous" => Ok(Self::Previous),
            value => value.parse::<u64>().map(Self::Id).map_err(|_| {
                format!("invalid workspace target '{value}'; expected next, prev, or an id")
            }),
        }
    }
}

/// Explicit boolean state accepted by live-system controls.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

impl From<OnOff> for bool {
    fn from(value: OnOff) -> Self {
        matches!(value, OnOff::On)
    }
}

impl From<WorkspaceTarget> for aegis_model::workspace::Switch {
    fn from(value: WorkspaceTarget) -> Self {
        match value {
            WorkspaceTarget::Next => aegis_model::workspace::Switch::Next,
            WorkspaceTarget::Previous => aegis_model::workspace::Switch::Prev,
            WorkspaceTarget::Id(_) => {
                unreachable!("concrete workspace ids do not use adjacent switching")
            }
        }
    }
}

/// Comma-separated `x,y,w,h` rectangle used by `--region` on capture commands.
///
/// Implemented as a `FromStr` newtype so `clap` parses it directly via
/// `value_parser`, instead of post-processing a raw `String` in each arm of
/// the dispatcher.
#[derive(Debug, Clone, Copy)]
pub struct Region(pub Rect);

impl FromStr for Region {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = value.split(',').collect();
        if parts.len() != 4 {
            return Err(format!("invalid region '{value}'; expected x,y,w,h"));
        }
        let mut numbers = [0i32; 4];
        for (index, part) in parts.into_iter().enumerate() {
            numbers[index] = part
                .parse()
                .map_err(|_| format!("invalid region '{value}'; expected integer x,y,w,h"))?;
        }
        if numbers[2] <= 0 || numbers[3] <= 0 {
            return Err("region width and height must be positive".into());
        }
        Ok(Region(Rect::new(
            numbers[0], numbers[1], numbers[2], numbers[3],
        )))
    }
}

impl From<Region> for Rect {
    fn from(value: Region) -> Self {
        value.0
    }
}
