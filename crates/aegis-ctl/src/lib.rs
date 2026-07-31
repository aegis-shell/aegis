//! aegis-ctl: the command-line driver for the aegis IPC.
//!
//! The reference external tool (ADR-0027): it connects to a running
//! compositor's IPC socket and drives it — list windows/workspaces, focus,
//! minimize, close, switch workspace, inspect and control live-system state,
//! post a notification, quit. The [`run`] entry point is unit-testable against
//! a loopback server; the thin binary in `main.rs` parses argv via `clap` and
//! prints the result.

mod cli;
mod error;

pub use cli::{Cli, Command, OnOff, RealmCmd, Region, SwitchDir, SystemCmd};
pub use error::CliError;

use std::path::Path;

use aegis_core::realm::{
    RealmId, RealmMutation, RealmSnapshot, RealmState, SeatCapabilities, VirtualOutput,
};
use aegis_ipc::{Capabilities, Client, Event, RealmAction, RealmActionResult};
use clap::{CommandFactory, Parser};
use serde::Serialize;

use crate::cli::Command as Cmd;

/// Connect to `socket`, parse `args` into a typed [`Cli`] via `clap`, and
/// return the formatted output. Errors are typed ([`CliError`]); the binary
/// maps them to exit codes.
///
/// `args` excludes `argv[0]` (the program name); the test harness passes a
/// slice of strings the same way `std::env::args().skip(1)` does in the
/// binary. `socket` may be an empty path when the parsed command is a
/// local-only invocation (`help`, `--help`, `--version`); those return
/// without touching the filesystem.
pub fn run(socket: &Path, args: &[String]) -> Result<String, CliError> {
    match parse_cli(args)? {
        ParseOutcome::Rendered(text) => Ok(text),
        ParseOutcome::Cli(cli) => dispatch_command(socket, cli),
    }
}

/// Like [`run`], but the caller has already parsed argv. Useful for embedding.
pub fn run_with(socket: &Path, cli: Cli) -> Result<String, CliError> {
    dispatch_command(socket, cli)
}

/// Parse argv via `clap`. Help and version requests are captured as
/// already-rendered text so they can be returned through [`run`] without
/// triggering `process::exit`; all other clap errors become
/// [`CliError::Usage`].
enum ParseOutcome {
    /// A real subcommand to dispatch on.
    Cli(Cli),
    /// Pre-rendered help or version text.
    Rendered(String),
}

fn parse_cli(args: &[String]) -> Result<ParseOutcome, CliError> {
    // `try_parse_from` consumes the first element as the bin name (argv[0]),
    // but the test harness and our public `run` exclude it. Prepend a stable
    // name so clap's "Usage:" line is consistent regardless of how the
    // caller collected `args`.
    let mut full = Vec::with_capacity(args.len() + 1);
    full.push("aegis-ctl".to_string());
    full.extend(args.iter().cloned());
    match Cli::try_parse_from(full) {
        Ok(cli) => Ok(ParseOutcome::Cli(cli)),
        Err(error) => match error.kind() {
            clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                Ok(ParseOutcome::Rendered(error.to_string()))
            }
            _ => Err(CliError::Usage(error)),
        },
    }
}

/// Connect to `socket`, dispatch one command, and return the formatted
/// output (or `Ok("")` for streaming / completion commands, which print
/// directly to stdout).
fn dispatch_command(socket: &Path, cli: Cli) -> Result<String, CliError> {
    let Cli { json, command } = cli;
    match command {
        Cmd::Windows => {
            let mut client = Client::connect_with(socket, query_caps()).map_err(connect_err)?;
            let wins = client.windows().map_err(io_err)?;
            Ok(render(&wins, json, |v| format_windows(v)))
        }
        Cmd::Workspaces => {
            let mut client = Client::connect_with(socket, query_caps()).map_err(connect_err)?;
            let snap = client.workspaces().map_err(io_err)?;
            Ok(render(&snap, json, format_workspaces))
        }
        Cmd::Outputs => {
            let mut client = Client::connect_with(socket, query_caps()).map_err(connect_err)?;
            let outs = client.outputs().map_err(io_err)?;
            Ok(render(&outs, json, |v| format_outputs(v)))
        }
        Cmd::Notifications => {
            let mut client = Client::connect_with(socket, query_caps()).map_err(connect_err)?;
            let notifications = client.notifications().map_err(io_err)?;
            Ok(render(&notifications, json, |v| format_notifications(v)))
        }
        Cmd::Journal { since } => {
            let mut client = Client::connect_with(socket, query_caps()).map_err(connect_err)?;
            let snapshot = client.journal(since.unwrap_or(0)).map_err(io_err)?;
            Ok(render(&snapshot, json, format_journal))
        }
        Cmd::Realm(action) => dispatch_realm(socket, action, json),
        Cmd::System(action) => dispatch_system(socket, action, json),
        Cmd::Focus { id } => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            client
                .command(aegis_ipc::Command::Focus {
                    id: aegis_core::window::WindowId(id),
                })
                .map_err(io_err)?;
            Ok(format!("focused {id}"))
        }
        Cmd::Minimize { id } => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            client
                .command(aegis_ipc::Command::Minimize {
                    id: aegis_core::window::WindowId(id),
                })
                .map_err(io_err)?;
            Ok(format!("minimized {id}"))
        }
        Cmd::AlwaysOnTop { id, state } => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            let on_top = bool::from(state);
            client
                .command(aegis_ipc::Command::SetAlwaysOnTop {
                    id: aegis_core::window::WindowId(id),
                    on_top,
                })
                .map_err(io_err)?;
            Ok(format!("always-on-top {on_top} for {id}"))
        }
        Cmd::Close { id } => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            client
                .command(aegis_ipc::Command::Close {
                    id: aegis_core::window::WindowId(id),
                })
                .map_err(io_err)?;
            Ok(format!("close requested for {id}"))
        }
        Cmd::SetGeometry { id, x, y, w, h } => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            let rect = aegis_core::Rect::new(x, y, w, h);
            client
                .set_window_geometry(aegis_core::window::WindowId(id), rect)
                .map_err(io_err)?;
            Ok(format!("set window {id} geometry to {x},{y} {w}x{h}",))
        }
        Cmd::Switch { direction } => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            let dir: aegis_core::workspace::Switch = direction.into();
            client.switch_workspace(dir).map_err(io_err)?;
            Ok(format!("switched {dir:?}"))
        }
        Cmd::SwitchTo { id } => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            let ws = aegis_core::workspace::WorkspaceId(id);
            client.switch_workspace_to(ws).map_err(io_err)?;
            Ok(format!("switched to workspace {id}"))
        }
        Cmd::MoveTo { window, workspace } => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            client
                .command(aegis_ipc::Command::MoveToWorkspace {
                    window: aegis_core::window::WindowId(window),
                    workspace: aegis_core::workspace::WorkspaceId(workspace),
                })
                .map_err(io_err)?;
            Ok(format!("moved window {window} to workspace {workspace}"))
        }
        Cmd::Tiling => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            client.toggle_tiling().map_err(io_err)?;
            Ok("toggled tiling".into())
        }
        Cmd::Notify { summary, body } => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            client
                .notify(summary, body.unwrap_or_default(), None)
                .map_err(io_err)?;
            Ok("notified".into())
        }
        Cmd::Dismiss { id } => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            client.dismiss_notification(id).map_err(io_err)?;
            Ok(format!("dismissed {id}"))
        }
        Cmd::Screenshot { path, region } => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            let path = match path {
                Some(p) => p,
                None => screenshot_path(&aegis_config::default_screenshot_dir())?,
            };
            client
                .screenshot_region(path.clone(), region.map(Into::into))
                .map_err(io_err)?;
            Ok(format!("screenshot queued → {path}"))
        }
        Cmd::Overview => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            client
                .command(aegis_ipc::Command::ToggleOverview)
                .map_err(io_err)?;
            Ok("toggled overview".into())
        }
        Cmd::Subscribe => {
            run_stream(socket, false)?;
            Ok(String::new())
        }
        Cmd::SubscribeJournal => {
            run_stream(socket, true)?;
            Ok(String::new())
        }
        Cmd::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "aegis-ctl", &mut std::io::stdout());
            Ok(String::new())
        }
        Cmd::Quit => {
            let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
            client.command(aegis_ipc::Command::Quit).map_err(io_err)?;
            Ok("quit requested".into())
        }
    }
}

fn dispatch_system(socket: &Path, command: SystemCmd, json: bool) -> Result<String, CliError> {
    if matches!(command, SystemCmd::Status) {
        let mut client = Client::connect_with(socket, query_caps()).map_err(connect_err)?;
        let status = client.system_status().map_err(io_err)?;
        return Ok(render(&status, json, format_system_status));
    }

    let (action, acknowledgement) = match command {
        SystemCmd::Status => unreachable!("handled above"),
        SystemCmd::Mute => (aegis_ipc::SystemAction::ToggleMute, "mute toggled"),
        SystemCmd::StepVolume { delta } => (
            aegis_ipc::SystemAction::StepVolume { delta },
            "volume step queued",
        ),
        SystemCmd::Volume { level } => (
            aegis_ipc::SystemAction::SetVolume { level },
            "volume change queued",
        ),
        SystemCmd::Brightness { level } => (
            aegis_ipc::SystemAction::SetBrightness { level },
            "brightness change queued",
        ),
        SystemCmd::Wifi { state } => (
            aegis_ipc::SystemAction::SetWifi {
                enabled: state.into(),
            },
            "Wi-Fi change queued",
        ),
        SystemCmd::Bluetooth { state } => (
            aegis_ipc::SystemAction::SetBluetooth {
                enabled: state.into(),
            },
            "Bluetooth change queued",
        ),
        SystemCmd::DoNotDisturb { state } => (
            aegis_ipc::SystemAction::SetDoNotDisturb {
                enabled: state.into(),
            },
            "Do Not Disturb change queued",
        ),
        SystemCmd::Tiling { state } => (
            aegis_ipc::SystemAction::SetTiling {
                enabled: state.into(),
            },
            "layout change queued",
        ),
    };
    let mut client = Client::connect_with(socket, control_caps()).map_err(connect_err)?;
    client.apply_system_action(action).map_err(io_err)?;
    Ok(acknowledgement.into())
}

fn dispatch_realm(socket: &Path, action: RealmCmd, json: bool) -> Result<String, CliError> {
    let caps = Capabilities {
        query: true,
        control: true,
        input: false,
        session: true,
        realm: true,
    };
    let mut client = Client::connect_scoped(socket, caps, aegis_ipc::LOCAL_REALM_ADMIN_SCOPE)
        .map_err(connect_err)?;
    match action {
        RealmCmd::List => {
            let snapshot = client.realms().map_err(io_err)?;
            Ok(render(&snapshot, json, format_realms))
        }
        RealmCmd::Create { label } => {
            let result = client
                .realm_action(RealmAction::Create {
                    label,
                    capabilities: SeatCapabilities::POINTER_KEYBOARD,
                    output: Some(VirtualOutput::DEFAULT_AGENT),
                })
                .map_err(io_err)?;
            format_realm_action(result, json)
        }
        RealmCmd::Pause { realm } => {
            let realm = validate_realm_id(realm)?;
            let snapshot = client.realms().map_err(io_err)?;
            let result = client
                .realm_action(RealmAction::Transact {
                    expected_revision: Some(snapshot.revision),
                    mutations: vec![RealmMutation::SetState {
                        realm,
                        state: RealmState::Paused,
                    }],
                })
                .map_err(io_err)?;
            format_realm_action(result, json)
        }
        RealmCmd::Resume { realm } => {
            let realm = validate_realm_id(realm)?;
            let snapshot = client.realms().map_err(io_err)?;
            let result = client
                .realm_action(RealmAction::Transact {
                    expected_revision: Some(snapshot.revision),
                    mutations: vec![RealmMutation::SetState {
                        realm,
                        state: RealmState::Active,
                    }],
                })
                .map_err(io_err)?;
            format_realm_action(result, json)
        }
        RealmCmd::Transfer {
            window,
            realm,
            no_mirror,
        } => {
            let realm = validate_realm_id(realm)?;
            let window = aegis_core::window::WindowId(window);
            let snapshot = client.realms().map_err(io_err)?;
            let result = client
                .realm_action(RealmAction::Transact {
                    expected_revision: Some(snapshot.revision),
                    mutations: vec![RealmMutation::TransferWindow {
                        window,
                        target: realm,
                        retain_source_as_observer: !no_mirror,
                    }],
                })
                .map_err(io_err)?;
            format_realm_action(result, json)
        }
        RealmCmd::Launch { realm, desktop_id } => {
            let realm = validate_realm_id(realm)?;
            client
                .launch_in_realm(realm, desktop_id.clone())
                .map_err(io_err)?;
            Ok(format!(
                "launch of {desktop_id} queued in Realm {}",
                realm.0
            ))
        }
        RealmCmd::Capture {
            realm,
            path,
            region,
        } => {
            let realm = validate_realm_id(realm)?;
            let capture = client
                .capture_realm(realm, region.map(Into::into))
                .map_err(io_err)?;
            let path = match path {
                Some(p) => std::path::PathBuf::from(p),
                None => realm_capture_path(&aegis_config::default_screenshot_dir(), realm)?,
            };
            atomic_write(&path, &capture.png)?;
            if json {
                serde_json::to_string(&serde_json::json!({
                    "realm": capture.realm,
                    "width": capture.width,
                    "height": capture.height,
                    "scale_milli": capture.scale_milli,
                    "region": capture.region,
                    "placements": capture.placements,
                    "revision": capture.revision,
                    "path": path,
                }))
                .map_err(|e| CliError::Io(e.to_string()))
            } else {
                Ok(format!(
                    "captured Realm {} at {}x{} (r{}) → {}",
                    realm.0,
                    capture.width,
                    capture.height,
                    capture.revision,
                    path.display()
                ))
            }
        }
        RealmCmd::Revoke { realm, fallback } => {
            let realm = validate_realm_id(realm)?;
            let fallback = if fallback == 0 {
                return Err(CliError::InvalidFallbackRealm(fallback));
            } else {
                RealmId(fallback)
            };
            let snapshot = client.realms().map_err(io_err)?;
            let result = client
                .realm_action(RealmAction::Revoke {
                    realm,
                    fallback,
                    expected_revision: Some(snapshot.revision),
                })
                .map_err(io_err)?;
            format_realm_action(result, json)
        }
    }
}

// ---- streaming subscriptions (kept separate: they don't return a string) --

/// Subscribe to the event stream and print each event as a line until the
/// connection closes. Returns the error that ended the stream.
pub fn run_subscribe(socket: &Path) -> Result<(), CliError> {
    run_stream(socket, false)
}

/// Subscribe to the detailed mutation journal and print entries until the
/// connection closes.
pub fn run_subscribe_journal(socket: &Path) -> Result<(), CliError> {
    run_stream(socket, true)
}

fn run_stream(socket: &Path, journal: bool) -> Result<(), CliError> {
    let caps = Capabilities {
        query: true,
        control: false,
        input: false,
        session: false,
        realm: false,
    };
    let mut client = Client::connect_with(socket, caps).map_err(connect_err)?;
    if journal {
        client
            .subscribe_journal()
            .map_err(|e| CliError::Io(format!("subscribe journal: {e}")))?;
    } else {
        client
            .subscribe()
            .map_err(|e| CliError::Io(format!("subscribe: {e}")))?;
    }
    loop {
        let ev = client
            .next_event()
            .map_err(|e| CliError::Io(format!("event stream ended: {e}")))?;
        println!("{}", format_event(&ev));
    }
}

// ---- capability constants ----------------------------------------------

fn query_caps() -> Capabilities {
    Capabilities {
        query: true,
        control: false,
        input: false,
        session: false,
        realm: false,
    }
}

fn control_caps() -> Capabilities {
    Capabilities {
        query: true,
        control: true,
        input: false,
        session: true,
        realm: false,
    }
}

fn connect_err(e: std::io::Error) -> CliError {
    CliError::Connect(e.to_string())
}

fn io_err(e: std::io::Error) -> CliError {
    CliError::Io(e.to_string())
}

fn validate_realm_id(raw: u64) -> Result<RealmId, CliError> {
    if raw == 0 {
        return Err(CliError::ZeroRealmId);
    }
    Ok(RealmId(raw))
}

// ---- tiny renderer helper: collapse the `if json { } else { }` pattern ---

/// Render a query result as JSON when `json` is set, otherwise hand it to the
/// human-readable formatter. Keeps each dispatcher arm one line of layout.
fn render<T: Serialize>(value: &T, json: bool, human: impl FnOnce(&T) -> String) -> String {
    if json {
        serde_json::to_string(value).unwrap_or_else(|e| format!("{{\"error\":\"serialize: {e}\"}}"))
    } else {
        human(value)
    }
}

// ---- path helpers --------------------------------------------------------

fn realm_capture_path(dir: &Path, realm: RealmId) -> Result<std::path::PathBuf, CliError> {
    std::fs::create_dir_all(dir).map_err(|e| {
        CliError::Fs(format!(
            "create screenshot directory {}: {e}",
            dir.display()
        ))
    })?;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(dir.join(format!("aegis-realm-{}-{ms}.png", realm.0)))
}

/// Generate a timestamped screenshot path and ensure its parent exists.
fn screenshot_path(dir: &Path) -> Result<String, CliError> {
    std::fs::create_dir_all(dir).map_err(|e| {
        CliError::Fs(format!(
            "create screenshot directory {}: {e}",
            dir.display()
        ))
    })?;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(dir
        .join(format!("aegis-screenshot-{ms}.png"))
        .to_string_lossy()
        .into_owned())
}

/// Atomically write a capture file: create a mode-`0600` temp file, sync it,
/// then rename. A failed write removes the temp file and never leaves a
/// partial PNG at `path`.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| CliError::Fs(format!("create {}: {e}", parent.display())))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| CliError::Fs(format!("capture path {} has no file name", path.display())))?
        .to_string_lossy();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temporary = parent.join(format!(".{file_name}.{}.{}.tmp", std::process::id(), nonce));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| CliError::Fs(format!("create {}: {e}", temporary.display())))?;
        file.write_all(bytes)
            .map_err(|e| CliError::Fs(format!("write {}: {e}", temporary.display())))?;
        file.sync_all()
            .map_err(|e| CliError::Fs(format!("sync {}: {e}", temporary.display())))?;
        std::fs::rename(&temporary, path).map_err(|e| {
            CliError::Fs(format!(
                "commit capture {} → {}: {e}",
                temporary.display(),
                path.display()
            ))
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

// ---- human-readable formatters ------------------------------------------

fn format_realm_action(result: RealmActionResult, json: bool) -> Result<String, CliError> {
    if json {
        return serde_json::to_string(&result).map_err(|e| CliError::Io(e.to_string()));
    }
    Ok(match result {
        RealmActionResult::Created { bundle } => format!(
            "created Realm {} with seat {} (r{}); launches use private mount-scoped portals",
            bundle.realm.0, bundle.seat.0, bundle.revision
        ),
        RealmActionResult::TransactionCommitted { receipt } => format!(
            "committed {} Realm mutation(s), r{} → r{}",
            receipt.results.len(),
            receipt.before_revision,
            receipt.after_revision
        ),
        RealmActionResult::Revoked { receipt } => format!(
            "revoked Realm {}; {} interaction group(s) returned to Realm {} (r{})",
            receipt.realm.0,
            receipt.transferred_groups.len(),
            receipt.fallback.0,
            receipt.revision
        ),
    })
}

fn format_realms(snapshot: &RealmSnapshot) -> String {
    let mut out = format!("authority revision {}\n", snapshot.revision);
    for realm in &snapshot.realms {
        let seats = snapshot
            .seats
            .iter()
            .filter(|seat| seat.realm == realm.id)
            .map(|seat| seat.name.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let windows = snapshot
            .interaction_groups
            .iter()
            .filter(|group| group.control_realm == realm.id)
            .map(|group| group.windows.len())
            .sum::<usize>();
        out.push_str(&format!(
            "{:<5} {:<20} {:?} {:?} seats=[{}] windows={}\n",
            realm.id.0, realm.label, realm.kind, realm.state, seats, windows
        ));
    }
    out
}

fn format_windows(wins: &[aegis_core::window::Window]) -> String {
    if wins.is_empty() {
        return "no windows".into();
    }
    let mut out = String::new();
    for w in wins {
        let title = w.title.as_deref().unwrap_or("<untitled>");
        let app = w.app_id.as_deref().unwrap_or("-");
        let mark = if w.state.activated { "*" } else { " " };
        out.push_str(&format!("{mark}{:<14} {} ({})\n", w.id.0, title, app));
    }
    out
}

fn format_workspaces(snap: &aegis_core::workspace::WorkspaceSnapshot) -> String {
    if snap.outputs.is_empty() {
        return "no outputs".into();
    }
    let mut out = String::new();
    for o in &snap.outputs {
        out.push_str(&format!("output {} ({})\n", o.id.0, o.connector));
        for (i, ws) in o.workspaces.iter().enumerate() {
            let cur = if o.current == Some(ws.id) { "*" } else { " " };
            out.push_str(&format!(
                "  {cur}{} ws {} ({} window(s))\n",
                i + 1,
                ws.id.0,
                ws.toplevels.len()
            ));
        }
    }
    out
}

fn format_outputs(outs: &[aegis_core::output::OutputInfo]) -> String {
    if outs.is_empty() {
        return "no outputs".into();
    }
    let mut out = String::new();
    for o in outs {
        let g = &o.geometry;
        let logical = g.logical_size();
        out.push_str(&format!(
            "{} {}x{}@{}.{:03}Hz scale {:.2} {:?} → logical {}x{}\n",
            o.connector,
            g.mode.width,
            g.mode.height,
            g.mode.refresh_mhz / 1000,
            g.mode.refresh_mhz % 1000,
            g.scale.as_f32(),
            g.transform,
            logical.w,
            logical.h,
        ));
        if !o.available_modes.is_empty() {
            let modes = o
                .available_modes
                .iter()
                .map(|m| {
                    let base = format!(
                        "{}x{}@{}.{:03}Hz",
                        m.width,
                        m.height,
                        m.refresh_mhz / 1000,
                        m.refresh_mhz % 1000,
                    );
                    if m == &g.mode {
                        format!("{base} (current)")
                    } else {
                        base
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("  modes: {modes}\n"));
        }
    }
    out
}

fn format_notifications(notifications: &[aegis_core::notify::Notification]) -> String {
    if notifications.is_empty() {
        return "no notifications".into();
    }
    let mut out = String::new();
    for n in notifications {
        let app = n.app_id.as_deref().unwrap_or("-");
        if n.body.is_empty() {
            out.push_str(&format!("{:<6} {} ({app})\n", n.id, n.summary));
        } else {
            out.push_str(&format!("{:<6} {} — {} ({app})\n", n.id, n.summary, n.body));
        }
    }
    out
}

fn format_system_status(status: &aegis_ipc::SystemStatus) -> String {
    let volume = status
        .volume
        .map(|level| format!("{level}%"))
        .unwrap_or_else(|| "unavailable".into());
    let network = match status.network {
        aegis_core::system::NetworkState::Offline => "offline",
        aegis_core::system::NetworkState::Wifi => "wifi",
        aegis_core::system::NetworkState::Wired => "wired",
    };
    let battery = status
        .battery
        .map(|battery| {
            format!(
                "{}%{}",
                battery.percent,
                if battery.charging { " charging" } else { "" }
            )
        })
        .unwrap_or_else(|| "unavailable".into());
    let brightness = status
        .brightness
        .map(|level| format!("{level}%"))
        .unwrap_or_else(|| "unavailable".into());
    format!(
        "audio: {volume} ({})\nnetwork: {network}; wifi: {}; bluetooth: {}\n\
         battery: {battery}; brightness: {brightness}\n\
         do not disturb: {}; layout: {}",
        if status.muted { "muted" } else { "unmuted" },
        format_optional_switch(status.wifi_enabled),
        format_optional_switch(status.bluetooth_enabled),
        if status.do_not_disturb { "on" } else { "off" },
        if status.tiled { "tiled" } else { "floating" },
    )
}

fn format_optional_switch(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "on",
        Some(false) => "off",
        None => "unavailable",
    }
}

fn format_journal(snapshot: &aegis_ipc::JournalSnapshot) -> String {
    if snapshot.entries.is_empty() {
        return format!(
            "no journal entries (oldest {}, latest {})",
            snapshot.oldest_seq, snapshot.latest_seq
        );
    }
    let mut out = String::new();
    for entry in &snapshot.entries {
        out.push_str(&format!(
            "#{:<6} {:<8} {:?} {:?} => {:?}\n",
            entry.seq,
            format!("{}ms", entry.ts_mono_ms),
            entry.origin,
            entry.mutation,
            entry.effect,
        ));
    }
    out
}

/// Format one server-pushed event as a single line for `subscribe`.
pub fn format_event(ev: &Event) -> String {
    match ev {
        Event::WindowsChanged => "windows changed".into(),
        Event::SpaceUseChanged { state } => format!("space use changed: {state:?}"),
        Event::WorkspaceChanged => "workspace changed".into(),
        Event::Notified { notification } => {
            let n = notification;
            match (&n.summary, n.body.as_str()) {
                (s, "") => format!("notify #{}: {s}", n.id),
                (s, b) => format!("notify #{}: {s} — {b}", n.id),
            }
        }
        Event::Journal { entry } => {
            format!(
                "journal #{} {:?}: {:?}",
                entry.seq, entry.origin, entry.mutation
            )
        }
        Event::RealmsChanged { revision } => format!("realms changed r{revision}"),
        Event::SettingsChanged { revision } => format!("settings changed r{revision}"),
        Event::SystemStatusChanged => "system status changed".into(),
        Event::RealmDamaged {
            realm,
            sequence,
            revision,
            ..
        } => format!(
            "realm {} damaged {} at authority revision {}",
            realm.0, sequence, revision
        ),
        Event::StreamFrame {
            stream_id,
            sequence,
            width,
            height,
            dropped,
            ..
        } => format!("stream {stream_id} frame #{sequence} {width}x{height} ({dropped} dropped)"),
        Event::StreamEnded { stream_id, reason } => format!("stream {stream_id} ended: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_core::notify::Notification;
    use std::str::FromStr;

    #[test]
    fn format_event_windows_and_workspace() {
        assert_eq!(format_event(&Event::WindowsChanged), "windows changed");
        assert_eq!(
            format_event(&Event::SpaceUseChanged {
                state: aegis_core::window::SpaceUse::Maximized,
            }),
            "space use changed: Maximized"
        );
        assert_eq!(format_event(&Event::WorkspaceChanged), "workspace changed");
        assert_eq!(
            format_event(&Event::SettingsChanged { revision: 9 }),
            "settings changed r9"
        );
        assert_eq!(
            format_event(&Event::SystemStatusChanged),
            "system status changed"
        );
    }

    #[test]
    fn format_event_notified_with_and_without_body() {
        let with_body = Event::Notified {
            notification: Notification {
                id: 7,
                summary: "ping".into(),
                body: "hello".into(),
                app_id: None,
                external_id: None,
                at_ms: 0,
            },
        };
        assert_eq!(format_event(&with_body), "notify #7: ping — hello");

        let no_body = Event::Notified {
            notification: Notification {
                id: 8,
                summary: "beep".into(),
                body: String::new(),
                app_id: None,
                external_id: None,
                at_ms: 0,
            },
        };
        assert_eq!(format_event(&no_body), "notify #8: beep");
    }

    #[test]
    fn region_parses_four_ints_and_rejects_bad_input() {
        assert!(Region::from_str("1,2,3").is_err());
        assert!(Region::from_str("a,b,c,d").is_err());
        assert!(Region::from_str("1,2,0,4").is_err());
        assert_eq!(
            Region::from_str("10,20,100,80").unwrap().0,
            aegis_core::Rect::new(10, 20, 100, 80)
        );
    }

    #[test]
    fn screenshot_path_uses_lowercase_directory_and_creates_it() {
        let dir = std::env::temp_dir().join(format!(
            "aegis-ctl-screenshots-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = std::path::PathBuf::from(screenshot_path(&dir).unwrap());
        assert_eq!(path.parent(), Some(dir.as_path()));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".png")
        );
        assert!(dir.is_dir());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
