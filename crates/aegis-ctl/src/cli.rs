//! Command-line structure: typed [`Cli`] parsed by `clap` derive.
//!
//! Defining the surface here lets `clap` generate help, version, shell
//! completions, and per-subcommand usage. The dispatcher in [`crate::lib`]
//! matches on [`Command`] / [`RealmCmd`] instead of raw strings.

use std::str::FromStr;

use aegis_core::Rect;

/// Top-level parser. `--json` is global so any query subcommand accepts it in
/// any position; the dispatcher decides whether the subcommand actually
/// produces structured output.
#[derive(Debug, clap::Parser)]
#[command(
    name = "aegis-ctl",
    version,
    about = "Query and control a running aegis compositor through its IPC socket"
)]
pub struct Cli {
    /// Emit query results and receipts as JSON instead of human-readable text.
    #[arg(short, long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// The top-level subcommands. Realm administration is grouped under
/// [`Command::Realm`] so the capability negotiation and admin-scope lease can
/// be handled in one arm of the dispatcher.
#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// List visible toplevels.
    Windows,
    /// List outputs, their workspaces, and window counts.
    Workspaces,
    /// List output modes, scales, transforms, and logical sizes.
    Outputs,
    /// List active notifications.
    Notifications,
    /// List mutation-journal entries with a sequence greater than `since`.
    Journal { since: Option<u64> },
    /// Manage isolated AI workspaces (Realm lifecycle and capture).
    #[command(subcommand)]
    Realm(RealmCmd),
    /// Inspect and control immediate live-system state.
    #[command(subcommand)]
    System(SystemCmd),
    /// Focus and raise a toplevel by id.
    Focus { id: u64 },
    /// Minimize a toplevel by id.
    Minimize { id: u64 },
    /// Request that a toplevel close.
    Close { id: u64 },
    /// Set floating-window geometry in compositor logical pixels.
    ///
    /// `allow_hyphen_values` is set so negative coordinates (e.g. `-20,30`)
    /// parse positionally instead of being mistaken for flags.
    #[command(allow_hyphen_values = true)]
    SetGeometry {
        id: u64,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
    /// Switch to an adjacent workspace on the focused output.
    Switch { direction: SwitchDir },
    /// Switch directly to a workspace by id.
    SwitchTo { id: u64 },
    /// Move a toplevel to a workspace by id.
    MoveTo { window: u64, workspace: u64 },
    /// Toggle tiling on the current workspace.
    Tiling,
    /// Post a notification.
    Notify {
        summary: String,
        body: Option<String>,
    },
    /// Dismiss an active notification by id.
    Dismiss { id: u64 },
    /// Capture the focused output to a PNG file.
    Screenshot {
        /// Destination path. Defaults to a timestamped PNG in the screenshot directory.
        path: Option<String>,
        /// Capture a region instead of the full output, formatted `x,y,w,h`.
        #[arg(long, value_name = "X,Y,W,H")]
        region: Option<Region>,
    },
    /// Toggle the window/workspace overview.
    Overview,
    /// Stream coarse compositor state-change events until disconnected.
    Subscribe,
    /// Stream detailed mutation-journal events until disconnected.
    SubscribeJournal,
    /// Print shell completions for the given shell to stdout.
    Completions { shell: clap_complete::Shell },
    /// Request compositor shutdown.
    Quit,
}

/// Subcommands grouped under `aegis-ctl system`.
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
    /// Set the current workspace layout mode.
    Tiling { state: OnOff },
}

/// Subcommands grouped under `aegis-ctl realm`. They all share the
/// owner-only admin scope and lease negotiation in the dispatcher.
#[derive(Debug, clap::Subcommand)]
pub enum RealmCmd {
    /// List the authority revision, Realms, states, seats, and controlled-window counts.
    List,
    /// Create an active agent Realm with a virtual output and pointer/keyboard seat.
    Create {
        /// Human-readable label for the new Realm.
        #[arg(default_value = "AI Workspace")]
        label: String,
    },
    /// Atomically pause a Realm and freeze its managed cgroups.
    Pause { realm: u64 },
    /// Resume a paused Realm and its managed cgroups.
    Resume { realm: u64 },
    /// Transfer the window's complete interaction group to another Realm.
    Transfer {
        window: u64,
        realm: u64,
        /// Do not retain the source Realm as a read-only observer.
        #[arg(long)]
        no_mirror: bool,
    },
    /// Launch an enumerated desktop entry through a private mount-scoped portal.
    Launch { realm: u64, desktop_id: String },
    /// Capture the Realm's directed virtual output atomically.
    Capture {
        realm: u64,
        /// Destination path. Defaults to a timestamped PNG in the screenshot directory.
        path: Option<String>,
        /// Capture a region instead of the full virtual output, formatted `x,y,w,h`.
        #[arg(long, value_name = "X,Y,W,H")]
        region: Option<Region>,
    },
    /// Permanently revoke a Realm and return controlled groups to `fallback`.
    Revoke {
        realm: u64,
        /// Realm id to receive the revoked Realm's interaction groups.
        #[arg(default_value = "1")]
        fallback: u64,
    },
}

/// Adjacent-workspace switch direction. `previous` is accepted as an alias
/// for `prev` so abbreviated and spelled-out forms both work.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum SwitchDir {
    Next,
    /// Accept `previous` as a long form.
    #[value(alias = "previous")]
    Prev,
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

impl From<SwitchDir> for aegis_core::workspace::Switch {
    fn from(value: SwitchDir) -> Self {
        match value {
            SwitchDir::Next => aegis_core::workspace::Switch::Next,
            SwitchDir::Prev => aegis_core::workspace::Switch::Prev,
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
