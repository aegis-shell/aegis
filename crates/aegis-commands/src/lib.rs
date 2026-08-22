//! Native domain commands for inspecting and controlling a running Aegis session.
//!
//! Domain commands remain an external IPC client at runtime even though they
//! share the `aegis` executable with the compositor. The [`run`] entry point is
//! unit-testable against a loopback server; the binary selects client or
//! compositor mode before initializing either runtime.

mod cli;
mod error;

pub use cli::{
    AuditCmd, Cli, Command, ConfigCmd, DisplayCmd, InteractionDomainCmd, JournalCmd, LayoutState,
    NotificationCmd, OnOff, PermissionsCmd, Region, SystemCmd, WindowCmd, WorkspaceCmd,
    WorkspaceTarget,
};
pub use error::CliError;

use std::path::{Path, PathBuf};

use aegis_ipc::{
    Client, ConnectionCapabilities, Event, InteractionDomainAction, InteractionDomainActionResult,
};
use aegis_model::interaction_domain::{
    InteractionDomainId, InteractionDomainMutation, InteractionDomainSnapshot,
    InteractionDomainState, SeatCapabilities, VirtualOutput,
};
use clap::{CommandFactory, Parser};
use serde::Serialize;

use self::cli::Command as Cmd;

/// Parse the current process arguments into the unified Aegis command model.
///
/// The executable calls this before initializing logging or either runtime;
/// keeping the `clap` dependency here preserves the command crate's ownership
/// of parsing and generated help.
pub fn parse_env() -> Result<Cli, clap::Error> {
    Cli::try_parse()
}

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
    full.push("aegis".to_string());
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
    let command = command.ok_or_else(client_command_required)?;
    match command {
        Cmd::Run => Err(client_command_required()),
        Cmd::Display { command } => dispatch_display(socket, command, json),
        Cmd::Window { command } => dispatch_window(socket, command, json),
        Cmd::Workspace { command } => dispatch_workspace(socket, command, json),
        Cmd::Notification { command } => dispatch_notification(socket, command, json),
        Cmd::Journal { command } => dispatch_journal(socket, command, json),
        Cmd::InteractionDomain { command } => {
            dispatch_interaction_domain(socket, command.unwrap_or(InteractionDomainCmd::List), json)
        }
        Cmd::Permissions { command } => {
            dispatch_permissions(socket, command.unwrap_or(PermissionsCmd::List), json)
        }
        Cmd::Config { command } => {
            dispatch_config(command.unwrap_or(ConfigCmd::Validate { path: None }), json)
        }
        Cmd::Audit { command } => dispatch_audit(command.unwrap_or(AuditCmd::Status), json),
        Cmd::System { command } => {
            dispatch_system(socket, command.unwrap_or(SystemCmd::Status), json)
        }
        Cmd::Overview => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            client
                .command(aegis_ipc::Command::ToggleOverview)
                .map_err(io_err)?;
            Ok(receipt("toggled overview", json))
        }
        Cmd::Events => {
            run_stream(socket, false, json)?;
            Ok(String::new())
        }
        Cmd::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "aegis", &mut std::io::stdout());
            Ok(String::new())
        }
        Cmd::Quit => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            client.command(aegis_ipc::Command::Quit).map_err(io_err)?;
            Ok(receipt("quit requested", json))
        }
    }
}

fn dispatch_config(command: ConfigCmd, json: bool) -> Result<String, CliError> {
    let resolve_path = |path: Option<std::path::PathBuf>| {
        path.or_else(aegis_config::default_path).ok_or_else(|| {
            CliError::Fs(
                "$XDG_CONFIG_HOME and HOME are unset; provide an explicit config path".into(),
            )
        })
    };
    match command {
        ConfigCmd::Validate { path } => {
            let path = resolve_path(path)?;
            match aegis_config::load(&path).map_err(config_error)? {
                Some(config) => {
                    if json {
                        Ok(serde_json::json!({
                            "path": path,
                            "schema_version": config.schema_version,
                            "valid": true,
                        })
                        .to_string())
                    } else {
                        Ok(format!(
                            "configuration valid: {} (schema {})",
                            path.display(),
                            config.schema_version
                        ))
                    }
                }
                None => Err(CliError::Fs(format!(
                    "configuration does not exist: {}",
                    path.display()
                ))),
            }
        }
        ConfigCmd::Migrate { path } => {
            let path = resolve_path(path)?;
            match aegis_config::migrate_file(&path).map_err(config_error)? {
                aegis_config::MigrationOutcome::AlreadyCurrent { version, .. } => {
                    if json {
                        Ok(serde_json::json!({
                            "path": path,
                            "schema_version": version,
                            "migrated": false,
                        })
                        .to_string())
                    } else {
                        Ok(format!(
                            "configuration already current: {} (schema {version})",
                            path.display()
                        ))
                    }
                }
                aegis_config::MigrationOutcome::Migrated {
                    backup,
                    from_version,
                    to_version,
                    ..
                } => {
                    if json {
                        Ok(serde_json::json!({
                            "path": path,
                            "backup": backup,
                            "from_schema_version": from_version,
                            "to_schema_version": to_version,
                            "migrated": true,
                        })
                        .to_string())
                    } else {
                        Ok(format!(
                            "migrated {} from schema {from_version} to {to_version}; backup: {}",
                            path.display(),
                            backup.display()
                        ))
                    }
                }
            }
        }
    }
}

fn config_error(error: aegis_config::LoadError) -> CliError {
    match error {
        aegis_config::LoadError::Invalid { path, diagnostics } => CliError::Fs(format!(
            "{}: {}",
            path.display(),
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        )),
        other => CliError::Fs(other.to_string()),
    }
}

/// Resolve the durable audit store path from the environment.
fn audit_stream_path() -> Result<PathBuf, CliError> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or_else(|| {
            CliError::Fs("$XDG_DATA_HOME and HOME are unset; cannot locate the audit store".into())
        })?;
    Ok(data_home.join("aegis/audit/events-v2.jsonl"))
}

fn audit_error(error: aegis_security::audit::AuditError) -> CliError {
    CliError::Fs(error.to_string())
}

fn audit_error_json(error: serde_json::Error) -> aegis_security::audit::AuditError {
    aegis_security::audit::AuditError::Decode(error)
}

fn open_audit_store_at(
    path: &Path,
) -> Result<aegis_security::audit::AuditLog<aegis_ipc::JournalEntry>, CliError> {
    if !path.exists() {
        return Err(CliError::Fs(format!(
            "audit store does not exist: {}",
            path.display()
        )));
    }
    aegis_security::audit::AuditLog::open_persistent_with_options(
        aegis_security::audit::DEFAULT_CAPACITY,
        path,
        aegis_security::audit::AuditStoreOptions::default(),
    )
    .map_err(audit_error)
}

fn dispatch_audit(command: AuditCmd, json: bool) -> Result<String, CliError> {
    let path = audit_stream_path()?;
    dispatch_audit_at(&path, command, json)
}

fn dispatch_audit_at(path: &Path, command: AuditCmd, json: bool) -> Result<String, CliError> {
    let open = || open_audit_store_at(path);
    match command {
        AuditCmd::Status => {
            let store = open()?;
            let status = store
                .audit_status()
                .ok_or_else(|| CliError::Fs("audit store is not persistent".into()))?;
            if json {
                Ok(serde_json::json!({
                    "path": path,
                    "next_sequence": status.next_sequence,
                    "tail_hash": status.tail_hash,
                    "sealed_segments": status.sealed_segments,
                    "pruned_segments": status.pruned_segments,
                    "sealed_original_bytes": status.sealed_original_bytes,
                    "sealed_compressed_bytes": status.sealed_compressed_bytes,
                    "active_bytes": status.active_bytes,
                    "total_bytes": status.total_bytes,
                    "last_export_destination": status.last_export_destination,
                })
                .to_string())
            } else {
                let mut lines = vec![format!("audit store: {}", path.display())];
                lines.push(format!("  next sequence: {}", status.next_sequence));
                lines.push(format!("  tail hash: {}", &status.tail_hash[..16]));
                lines.push(format!(
                    "  sealed segments: {} ({:.1} MiB original, {:.1} MiB compressed)",
                    status.sealed_segments,
                    status.sealed_original_bytes as f64 / (1024.0 * 1024.0),
                    status.sealed_compressed_bytes as f64 / (1024.0 * 1024.0),
                ));
                lines.push(format!(
                    "  active stream: {:.1} MiB",
                    status.active_bytes as f64 / (1024.0 * 1024.0)
                ));
                lines.push(format!(
                    "  total on disk: {:.1} MiB",
                    status.total_bytes as f64 / (1024.0 * 1024.0)
                ));
                if status.pruned_segments > 0 {
                    lines.push(format!(
                        "  pruned segments on record: {}",
                        status.pruned_segments
                    ));
                }
                if let Some(destination) = status.last_export_destination {
                    lines.push(format!("  last export: {destination}"));
                }
                Ok(lines.join("\n"))
            }
        }
        AuditCmd::Verify { full } => {
            let store = open()?;
            if full {
                // Full verification decompresses every sealed segment and
                // replays the complete chain, including the active stream.
                let records: Vec<_> = store.sealed_segments().to_vec();
                let segments_dir = path.with_file_name("segments");
                let mut verified = 0usize;
                for record in &records {
                    let mut reader = aegis_security::audit::segments::SealedSegmentReader::open(
                        record,
                        &segments_dir,
                    )
                    .map_err(audit_error)?;
                    reader
                        .for_each_line(|line| {
                            // Structural JSONL check; the chain walk is the
                            // manifest MAC plus this decompression pass.
                            let _envelope: serde_json::Value =
                                serde_json::from_slice(&line[..line.len() - 1])
                                    .map_err(audit_error_json)?;
                            Ok(())
                        })
                        .map_err(audit_error)?;
                    verified += 1;
                }
                let active = store.verify_sealed_segments().map_err(audit_error)?;
                let _ = active;
                Ok(receipt(
                    format!(
                        "verified {verified} sealed segment(s) by full decompression and chain walk"
                    ),
                    json,
                ))
            } else {
                let verified = store.verify_sealed_segments().map_err(audit_error)?;
                Ok(receipt(
                    format!("verified {verified} sealed segment(s) against the manifest"),
                    json,
                ))
            }
        }
        AuditCmd::Export { destination } => {
            let mut store = open()?;
            let count = store
                .mark_segments_exported(&destination)
                .map_err(audit_error)?;
            Ok(receipt(
                format!("recorded export of {count} sealed segment(s) to {destination}"),
                json,
            ))
        }
        AuditCmd::Prune { keep, force } => {
            if keep == 0 {
                return Err(CliError::Usage({
                    <Cli as clap::CommandFactory>::command().error(
                        clap::error::ErrorKind::InvalidValue,
                        "keep must be at least 1; use 0 retention in config to disable pruning",
                    )
                }));
            }
            let mut store = open()?;
            let removed = store.prune_segments(keep, !force).map_err(audit_error)?;
            Ok(receipt(
                format!(
                    "pruned {} sealed segment(s), keeping {}",
                    removed.len(),
                    keep
                ),
                json,
            ))
        }
    }
}

fn client_command_required() -> CliError {
    CliError::Usage(Cli::command().error(
        clap::error::ErrorKind::MissingSubcommand,
        "a domain command is required when using the IPC client",
    ))
}

fn dispatch_display(
    socket: &Path,
    command: Option<DisplayCmd>,
    json: bool,
) -> Result<String, CliError> {
    match command.unwrap_or(DisplayCmd::List) {
        DisplayCmd::List => {
            let mut client = Client::connect_with(socket, query_caps()).map_err(connect_err)?;
            let displays = client.outputs().map_err(io_err)?;
            Ok(render(&displays, json, |value| format_outputs(value)))
        }
        DisplayCmd::Capture { path, region } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            let path = match path {
                Some(path) => path,
                None => screenshot_path(&aegis_config::default_screenshot_dir())?,
            };
            client
                .screenshot_region(path.clone(), region.map(Into::into))
                .map_err(io_err)?;
            Ok(receipt(format!("display capture queued → {path}"), json))
        }
    }
}

fn dispatch_window(
    socket: &Path,
    command: Option<WindowCmd>,
    json: bool,
) -> Result<String, CliError> {
    match command.unwrap_or(WindowCmd::List) {
        WindowCmd::List => {
            let mut client = Client::connect_with(socket, query_caps()).map_err(connect_err)?;
            let windows = client.windows().map_err(io_err)?;
            Ok(render(&windows, json, |value| format_windows(value)))
        }
        WindowCmd::Focus { id } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            client
                .command(aegis_ipc::Command::Focus {
                    id: aegis_model::window::WindowId(id),
                    reveal: true,
                })
                .map_err(io_err)?;
            Ok(receipt(format!("focused {id}"), json))
        }
        WindowCmd::Minimize { id } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            client
                .command(aegis_ipc::Command::Minimize {
                    id: aegis_model::window::WindowId(id),
                })
                .map_err(io_err)?;
            Ok(receipt(format!("minimized {id}"), json))
        }
        WindowCmd::AlwaysOnTop { id, state } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            let on_top = bool::from(state);
            client
                .command(aegis_ipc::Command::SetAlwaysOnTop {
                    id: aegis_model::window::WindowId(id),
                    on_top,
                })
                .map_err(io_err)?;
            Ok(receipt(format!("always-on-top {on_top} for {id}"), json))
        }
        WindowCmd::Fullscreen { id, state } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            let fullscreen = bool::from(state);
            client
                .command(aegis_ipc::Command::SetFullscreen {
                    id: aegis_model::window::WindowId(id),
                    fullscreen,
                })
                .map_err(io_err)?;
            Ok(receipt(format!("fullscreen {fullscreen} for {id}"), json))
        }
        WindowCmd::Close { id } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            client
                .command(aegis_ipc::Command::Close {
                    id: aegis_model::window::WindowId(id),
                })
                .map_err(io_err)?;
            Ok(receipt(format!("close requested for {id}"), json))
        }
        WindowCmd::Geometry {
            id,
            x,
            y,
            width,
            height,
        } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            let rect = aegis_model::Rect::new(x, y, width, height);
            client
                .set_window_geometry(aegis_model::window::WindowId(id), rect)
                .map_err(io_err)?;
            Ok(receipt(
                format!("set window {id} geometry to {x},{y} {width}x{height}"),
                json,
            ))
        }
    }
}

fn dispatch_workspace(
    socket: &Path,
    command: Option<WorkspaceCmd>,
    json: bool,
) -> Result<String, CliError> {
    match command.unwrap_or(WorkspaceCmd::List) {
        WorkspaceCmd::List => {
            let mut client = Client::connect_with(socket, query_caps()).map_err(connect_err)?;
            let workspaces = client.workspaces().map_err(io_err)?;
            Ok(render(&workspaces, json, format_workspaces))
        }
        WorkspaceCmd::Switch {
            target: WorkspaceTarget::Id(id),
        } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            client
                .switch_workspace_to(aegis_model::workspace::WorkspaceId(id))
                .map_err(io_err)?;
            Ok(receipt(format!("switched to workspace {id}"), json))
        }
        WorkspaceCmd::Switch { target } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            let direction: aegis_model::workspace::Switch = target.into();
            client.switch_workspace(direction).map_err(io_err)?;
            Ok(receipt(format!("switched {direction:?}"), json))
        }
        WorkspaceCmd::MoveWindow { window, workspace } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            client
                .command(aegis_ipc::Command::MoveToWorkspace {
                    window: aegis_model::window::WindowId(window),
                    workspace: aegis_model::workspace::WorkspaceId(workspace),
                })
                .map_err(io_err)?;
            Ok(receipt(
                format!("moved window {window} to workspace {workspace}"),
                json,
            ))
        }
        WorkspaceCmd::Layout {
            state: LayoutState::Toggle,
        } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            client.toggle_tiling().map_err(io_err)?;
            Ok(receipt("toggled workspace layout", json))
        }
        WorkspaceCmd::Layout { state } => {
            let enabled = matches!(state, LayoutState::Tiled);
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            client
                .apply_system_action(aegis_ipc::SystemAction::SetTiling { enabled })
                .map_err(io_err)?;
            Ok(receipt(
                format!(
                    "workspace layout changed to {}",
                    if enabled { "tiled" } else { "floating" }
                ),
                json,
            ))
        }
    }
}

fn dispatch_notification(
    socket: &Path,
    command: Option<NotificationCmd>,
    json: bool,
) -> Result<String, CliError> {
    match command.unwrap_or(NotificationCmd::List) {
        NotificationCmd::List => {
            let mut client = Client::connect_with(socket, query_caps()).map_err(connect_err)?;
            let notifications = client.notifications().map_err(io_err)?;
            Ok(render(&notifications, json, |value| {
                format_notifications(value)
            }))
        }
        NotificationCmd::Send { summary, body } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            client
                .notify(summary, body.unwrap_or_default(), None)
                .map_err(io_err)?;
            Ok(receipt("notification sent", json))
        }
        NotificationCmd::Dismiss { id } => {
            let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
            client.dismiss_notification(id).map_err(io_err)?;
            Ok(receipt(format!("dismissed {id}"), json))
        }
    }
}

fn dispatch_journal(
    socket: &Path,
    command: Option<JournalCmd>,
    json: bool,
) -> Result<String, CliError> {
    match command.unwrap_or(JournalCmd::List { since: None }) {
        JournalCmd::List { since } => {
            let mut client = Client::connect_with(socket, query_caps()).map_err(connect_err)?;
            let snapshot = client.journal(since.unwrap_or(0)).map_err(io_err)?;
            Ok(render(&snapshot, json, format_journal))
        }
        JournalCmd::Follow => {
            run_stream(socket, true, json)?;
            Ok(String::new())
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
    };
    let mut client = owner_client(socket, control_caps()).map_err(connect_err)?;
    client.apply_system_action(action).map_err(io_err)?;
    Ok(receipt(acknowledgement, json))
}

fn dispatch_permissions(
    socket: &Path,
    action: PermissionsCmd,
    json: bool,
) -> Result<String, CliError> {
    match action {
        PermissionsCmd::List => {
            let mut client =
                Client::connect_scoped(socket, query_caps(), aegis_ipc::LOCAL_AGENT_ADMIN_SCOPE)
                    .map_err(connect_err)?;
            let principals = client.agent_principals().map_err(io_err)?;
            let grants = client.agent_grants(None).map_err(io_err)?;
            #[derive(Serialize)]
            struct PermissionsView {
                principals: Vec<aegis_ipc::AgentPrincipalInfo>,
                grants: Vec<aegis_ipc::AgentGrantInfo>,
            }
            let view = PermissionsView { principals, grants };
            Ok(render(&view, json, |view| {
                format_permissions(&view.principals, &view.grants)
            }))
        }
        PermissionsCmd::Revoke { principal, op } => {
            let mut client =
                Client::connect_scoped(socket, control_caps(), aegis_ipc::LOCAL_AGENT_ADMIN_SCOPE)
                    .map_err(connect_err)?;
            client.revoke_agent_grant(&principal, op).map_err(io_err)?;
            Ok(receipt(
                format!("revoked the {op:?} grant for {principal}"),
                json,
            ))
        }
        PermissionsCmd::Forget { principal } => {
            let mut client =
                Client::connect_scoped(socket, control_caps(), aegis_ipc::LOCAL_AGENT_ADMIN_SCOPE)
                    .map_err(connect_err)?;
            client.forget_agent_principal(&principal).map_err(io_err)?;
            Ok(receipt(format!("forgot principal {principal}"), json))
        }
        PermissionsCmd::Rename { principal, label } => {
            let mut client =
                Client::connect_scoped(socket, control_caps(), aegis_ipc::LOCAL_AGENT_ADMIN_SCOPE)
                    .map_err(connect_err)?;
            client
                .rename_agent_principal(&principal, label.as_deref())
                .map_err(io_err)?;
            let message = match label {
                Some(label) => format!("renamed {principal} to '{label}'"),
                None => format!("cleared the label of {principal}"),
            };
            Ok(receipt(message, json))
        }
        PermissionsCmd::SetCeiling {
            principal,
            pregrant,
            gated,
        } => {
            let mut client =
                Client::connect_scoped(socket, control_caps(), aegis_ipc::LOCAL_AGENT_ADMIN_SCOPE)
                    .map_err(connect_err)?;
            client
                .set_agent_ceiling(&principal, pregrant, gated)
                .map_err(io_err)?;
            Ok(receipt(
                format!("replaced the ceiling of {principal}"),
                json,
            ))
        }
        PermissionsCmd::Register {
            label,
            pregrant,
            gated,
        } => {
            let mut client =
                Client::connect_scoped(socket, control_caps(), aegis_ipc::LOCAL_AGENT_ADMIN_SCOPE)
                    .map_err(connect_err)?;
            let (principal, credential) = client
                .register_agent(label.as_deref(), pregrant, gated)
                .map_err(io_err)?;
            #[derive(Serialize)]
            struct Registered {
                principal: String,
                credential: String,
            }
            impl Drop for Registered {
                fn drop(&mut self) {
                    use zeroize::Zeroize as _;
                    self.credential.zeroize();
                }
            }
            let registered = Registered {
                principal,
                credential,
            };
            Ok(render(&registered, json, |registered| {
                format!(
                    "registered {}\ncredential: {}\nplant it in the agent identity store",
                    registered.principal, registered.credential
                )
            }))
        }
    }
}

fn format_permissions(
    principals: &[aegis_ipc::AgentPrincipalInfo],
    grants: &[aegis_ipc::AgentGrantInfo],
) -> String {
    if principals.is_empty() {
        return "no paired agents".into();
    }
    let mut out = String::new();
    for principal in principals {
        match &principal.label {
            Some(label) => out.push_str(&format!("{label} ({})\n", principal.principal)),
            None => out.push_str(&format!("{}\n", principal.principal)),
        }
        let pregranted = op_names(&principal.pregranted);
        let gated = op_names(&principal.gated);
        out.push_str(&format!(
            "  ceiling: {pregranted}{}\n",
            if gated.is_empty() {
                String::new()
            } else {
                format!(" (gated: {gated})")
            }
        ));
        let own = grants
            .iter()
            .filter(|grant| grant.principal == principal.principal)
            .collect::<Vec<_>>();
        if own.is_empty() {
            out.push_str("  grants: none\n");
        } else {
            out.push_str("  grants:\n");
            for grant in own {
                out.push_str(&format!("    {:?}: {:?}\n", grant.op, grant.decision));
            }
        }
    }
    out.trim_end().to_string()
}

fn op_names(ops: &[aegis_ipc::ActorCapability]) -> String {
    ops.iter()
        .map(|op| format!("{op:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn dispatch_interaction_domain(
    socket: &Path,
    action: InteractionDomainCmd,
    json: bool,
) -> Result<String, CliError> {
    let caps = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: true,
        interaction_domain: true,
    };
    let mut client = Client::connect_scoped(
        socket,
        caps,
        aegis_ipc::LOCAL_INTERACTION_DOMAIN_ADMIN_SCOPE,
    )
    .map_err(connect_err)?;
    match action {
        InteractionDomainCmd::List => {
            let snapshot = client.interaction_domains().map_err(io_err)?;
            Ok(render(&snapshot, json, format_interaction_domains))
        }
        InteractionDomainCmd::Create { label } => {
            let result = client
                .interaction_domain_action(InteractionDomainAction::Create {
                    label,
                    capabilities: SeatCapabilities::POINTER_KEYBOARD,
                    output: Some(VirtualOutput::DEFAULT_AGENT),
                })
                .map_err(io_err)?;
            format_interaction_domain_action(result, json)
        }
        InteractionDomainCmd::Pause { interaction_domain } => {
            let interaction_domain = validate_interaction_domain_id(interaction_domain)?;
            let snapshot = client.interaction_domains().map_err(io_err)?;
            let result = client
                .interaction_domain_action(InteractionDomainAction::Transact {
                    expected_revision: Some(snapshot.revision),
                    mutations: vec![InteractionDomainMutation::SetState {
                        interaction_domain,
                        state: InteractionDomainState::Paused,
                    }],
                })
                .map_err(io_err)?;
            format_interaction_domain_action(result, json)
        }
        InteractionDomainCmd::Resume { interaction_domain } => {
            let interaction_domain = validate_interaction_domain_id(interaction_domain)?;
            let snapshot = client.interaction_domains().map_err(io_err)?;
            let result = client
                .interaction_domain_action(InteractionDomainAction::Transact {
                    expected_revision: Some(snapshot.revision),
                    mutations: vec![InteractionDomainMutation::SetState {
                        interaction_domain,
                        state: InteractionDomainState::Active,
                    }],
                })
                .map_err(io_err)?;
            format_interaction_domain_action(result, json)
        }
        InteractionDomainCmd::Transfer {
            window,
            interaction_domain,
            no_mirror,
        } => {
            let interaction_domain = validate_interaction_domain_id(interaction_domain)?;
            let window = aegis_model::window::WindowId(window);
            let snapshot = client.interaction_domains().map_err(io_err)?;
            let result = client
                .interaction_domain_action(InteractionDomainAction::Transact {
                    expected_revision: Some(snapshot.revision),
                    mutations: vec![InteractionDomainMutation::TransferWindow {
                        window,
                        target: interaction_domain,
                        retain_source_as_observer: !no_mirror,
                    }],
                })
                .map_err(io_err)?;
            format_interaction_domain_action(result, json)
        }
        InteractionDomainCmd::Launch {
            interaction_domain,
            desktop_id,
        } => {
            let interaction_domain = validate_interaction_domain_id(interaction_domain)?;
            client
                .launch_in_interaction_domain(interaction_domain, desktop_id.clone())
                .map_err(io_err)?;
            Ok(receipt(
                format!(
                    "launch of {desktop_id} queued in Interaction Domain {}",
                    interaction_domain.0
                ),
                json,
            ))
        }
        InteractionDomainCmd::Capture {
            interaction_domain,
            path,
            region,
        } => {
            let interaction_domain = validate_interaction_domain_id(interaction_domain)?;
            let capture = client
                .capture_interaction_domain(interaction_domain, region.map(Into::into))
                .map_err(io_err)?;
            let path = match path {
                Some(p) => std::path::PathBuf::from(p),
                None => interaction_domain_capture_path(
                    &aegis_config::default_screenshot_dir(),
                    interaction_domain,
                )?,
            };
            atomic_write(&path, &capture.png)?;
            if json {
                serde_json::to_string(&serde_json::json!({
                    "interaction_domain": capture.interaction_domain,
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
                    "captured Interaction Domain {} at {}x{} (r{}) → {}",
                    interaction_domain.0,
                    capture.width,
                    capture.height,
                    capture.revision,
                    path.display()
                ))
            }
        }
        InteractionDomainCmd::Revoke {
            interaction_domain,
            fallback,
        } => {
            let interaction_domain = validate_interaction_domain_id(interaction_domain)?;
            let fallback = if fallback == 0 {
                return Err(CliError::InvalidFallbackInteractionDomain(fallback));
            } else {
                InteractionDomainId(fallback)
            };
            let snapshot = client.interaction_domains().map_err(io_err)?;
            let result = client
                .interaction_domain_action(InteractionDomainAction::Revoke {
                    interaction_domain,
                    fallback,
                    expected_revision: Some(snapshot.revision),
                })
                .map_err(io_err)?;
            format_interaction_domain_action(result, json)
        }
    }
}

// ---- streaming subscriptions (kept separate: they don't return a string) --

/// Subscribe to the event stream and print each event as a line until the
/// connection closes. Returns the error that ended the stream.
pub fn run_subscribe(socket: &Path) -> Result<(), CliError> {
    run_stream(socket, false, false)
}

/// Subscribe to the detailed mutation journal and print entries until the
/// connection closes.
pub fn run_subscribe_journal(socket: &Path) -> Result<(), CliError> {
    run_stream(socket, true, false)
}

fn run_stream(socket: &Path, journal: bool, json: bool) -> Result<(), CliError> {
    let caps = ConnectionCapabilities {
        query: true,
        control: false,
        input: false,
        session: false,
        interaction_domain: false,
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
        if json {
            println!(
                "{}",
                serde_json::to_string(&ev).map_err(|error| CliError::Io(error.to_string()))?
            );
        } else {
            println!("{}", format_event(&ev));
        }
    }
}

// ---- capability constants ----------------------------------------------

fn query_caps() -> ConnectionCapabilities {
    ConnectionCapabilities {
        query: true,
        control: false,
        input: false,
        session: false,
        interaction_domain: false,
    }
}

fn control_caps() -> ConnectionCapabilities {
    ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: true,
        interaction_domain: false,
    }
}

fn owner_client(socket: &Path, caps: ConnectionCapabilities) -> std::io::Result<Client> {
    Client::connect_scoped(socket, caps, aegis_ipc::LOCAL_OWNER_ADMIN_SCOPE)
}

fn connect_err(e: std::io::Error) -> CliError {
    CliError::Connect(e.to_string())
}

fn io_err(e: std::io::Error) -> CliError {
    CliError::Io(e.to_string())
}

fn validate_interaction_domain_id(raw: u64) -> Result<InteractionDomainId, CliError> {
    if raw == 0 {
        return Err(CliError::ZeroInteractionDomainId);
    }
    Ok(InteractionDomainId(raw))
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

/// Render a successful mutation in the same global output mode as queries.
/// A small stable envelope keeps script output machine-readable while the
/// human acknowledgement remains concise.
fn receipt(message: impl Into<String>, json: bool) -> String {
    let message = message.into();
    if json {
        serde_json::json!({ "ok": true, "message": message }).to_string()
    } else {
        message
    }
}

// ---- path helpers --------------------------------------------------------

fn interaction_domain_capture_path(
    dir: &Path,
    interaction_domain: InteractionDomainId,
) -> Result<std::path::PathBuf, CliError> {
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
    Ok(dir.join(format!(
        "aegis-interaction-domain-{}-{ms}.png",
        interaction_domain.0
    )))
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

fn format_interaction_domain_action(
    result: InteractionDomainActionResult,
    json: bool,
) -> Result<String, CliError> {
    if json {
        return serde_json::to_string(&result).map_err(|e| CliError::Io(e.to_string()));
    }
    Ok(match result {
        InteractionDomainActionResult::Created { bundle } => format!(
            "created Interaction Domain {} with seat {} (r{}); launches use private mount-scoped portals",
            bundle.interaction_domain.0, bundle.seat.0, bundle.revision
        ),
        InteractionDomainActionResult::TransactionCommitted { receipt } => format!(
            "committed {} Interaction Domain mutation(s), r{} → r{}",
            receipt.results.len(),
            receipt.before_revision,
            receipt.after_revision
        ),
        InteractionDomainActionResult::Revoked { receipt } => format!(
            "revoked Interaction Domain {}; {} interaction group(s) returned to Interaction Domain {} (r{})",
            receipt.interaction_domain.0,
            receipt.transferred_groups.len(),
            receipt.fallback.0,
            receipt.revision
        ),
    })
}

fn format_interaction_domains(snapshot: &InteractionDomainSnapshot) -> String {
    let mut out = format!("authority revision {}\n", snapshot.revision);
    for interaction_domain in &snapshot.interaction_domains {
        let seats = snapshot
            .seats
            .iter()
            .filter(|seat| seat.interaction_domain == interaction_domain.id)
            .map(|seat| seat.name.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let windows = snapshot
            .interaction_groups
            .iter()
            .filter(|group| group.control_interaction_domain == interaction_domain.id)
            .map(|group| group.windows.len())
            .sum::<usize>();
        out.push_str(&format!(
            "{:<5} {:<20} {:?} {:?} seats=[{}] windows={}\n",
            interaction_domain.id.0,
            interaction_domain.label,
            interaction_domain.kind,
            interaction_domain.state,
            seats,
            windows
        ));
    }
    out
}

fn format_windows(wins: &[aegis_model::window::Window]) -> String {
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

fn format_workspaces(snap: &aegis_model::workspace::WorkspaceSnapshot) -> String {
    if snap.outputs.is_empty() {
        return "no displays".into();
    }
    let mut out = String::new();
    for o in &snap.outputs {
        out.push_str(&format!("display {} ({})\n", o.id.0, o.connector));
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

fn format_outputs(outs: &[aegis_model::output::OutputInfo]) -> String {
    if outs.is_empty() {
        return "no displays".into();
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

fn format_notifications(notifications: &[aegis_model::notify::Notification]) -> String {
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
        aegis_model::system::NetworkState::Offline => "offline",
        aegis_model::system::NetworkState::Wifi => "wifi",
        aegis_model::system::NetworkState::Wired => "wired",
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
            "#{:<6} {:<8} {:?} {} => {:?}\n",
            entry.seq,
            format!("{}ms", entry.ts_mono_ms),
            entry.origin,
            format_mutation(&entry.mutation),
            entry.effect,
        ));
    }
    out
}

/// Human-readable one-liner for a journal mutation. Agent-authorization
/// lifecycle events (ADR-0088) get a readable summary; everything else
/// keeps the debug form.
fn format_mutation(mutation: &aegis_ipc::JournalMutation) -> String {
    match mutation {
        aegis_ipc::JournalMutation::AgentAuth { principal, action } => {
            let action = match action {
                aegis_ipc::AgentAuthAction::Paired => "paired".to_owned(),
                aegis_ipc::AgentAuthAction::Granted { op, persistence } => {
                    let scope = match persistence {
                        aegis_ipc::GrantPersistence::Once => "once",
                        aegis_ipc::GrantPersistence::Session => "for this session",
                        aegis_ipc::GrantPersistence::Always => "always",
                        aegis_ipc::GrantPersistence::DeniedSession => {
                            return format!(
                                "agent {principal}: denied \"{}\" for this session",
                                op.label()
                            );
                        }
                    };
                    format!("granted \"{}\" {scope}", op.label())
                }
                aegis_ipc::AgentAuthAction::GrantRevoked { op } => {
                    format!("revoked grant for \"{}\"", op.label())
                }
                aegis_ipc::AgentAuthAction::Forgotten => "forgotten".to_owned(),
                aegis_ipc::AgentAuthAction::Renamed => "renamed".to_owned(),
                aegis_ipc::AgentAuthAction::CeilingChanged => "ceiling changed".to_owned(),
            };
            format!("agent {principal}: {action}")
        }
        other => format!("{other:?}"),
    }
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
                "journal #{} {:?}: {}",
                entry.seq,
                entry.origin,
                format_mutation(&entry.mutation)
            )
        }
        Event::InteractionDomainsChanged { revision } => {
            format!("interaction_domains changed r{revision}")
        }
        Event::SettingsChanged { revision } => format!("settings changed r{revision}"),
        Event::SystemStatusChanged => "system status changed".into(),
        Event::InteractionDomainDamaged {
            interaction_domain,
            sequence,
            revision,
            ..
        } => format!(
            "interaction_domain {} damaged {} at authority revision {}",
            interaction_domain.0, sequence, revision
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
        Event::StreamGeometryChanged {
            stream_id,
            width,
            height,
        } => format!("stream {stream_id} geometry changed to {width}x{height}; restart it"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_model::notify::Notification;
    use std::str::FromStr;

    #[test]
    fn format_event_windows_and_workspace() {
        assert_eq!(format_event(&Event::WindowsChanged), "windows changed");
        assert_eq!(
            format_event(&Event::SpaceUseChanged {
                state: aegis_model::window::SpaceUse::Maximized,
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
            aegis_model::Rect::new(10, 20, 100, 80)
        );
    }

    #[test]
    fn screenshot_path_uses_lowercase_directory_and_creates_it() {
        let dir = std::env::temp_dir().join(format!(
            "aegis-command-screenshots-{}-{}",
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

    #[test]
    fn audit_cli_status_verify_export_and_prune_round_trip() {
        // Build a small real store with tiny segments via the security layer,
        // then drive the CLI dispatch over it. The dispatch functions resolve
        // the store through XDG_DATA_HOME, so scope that variable per test.
        let dir = tempfile::tempdir().unwrap();
        let data_home = dir.path().join("data");
        std::fs::create_dir_all(data_home.join("aegis/audit")).unwrap();
        let stream = data_home.join("aegis/audit/events-v2.jsonl");
        let options = aegis_security::audit::AuditStoreOptions {
            min_free_bytes: 0,
            checkpoint_interval_events: u64::MAX,
            checkpoint_interval_bytes: u64::MAX,
            segment_max_bytes: 1,
            retain_segments: 0,
            ..aegis_security::audit::AuditStoreOptions::default()
        };
        {
            let mut log = aegis_security::audit::AuditLog::<aegis_ipc::JournalEntry>::open_persistent_with_options(8, &stream, options).unwrap();
            for sequence in 1..=12u64 {
                log.try_append(
                    sequence,
                    aegis_ipc::Origin::Internal,
                    aegis_ipc::JournalMutation::ScopeClaim {
                        scope: format!("scope-{sequence}"),
                    },
                    aegis_ipc::Effect::Applied,
                )
                .unwrap();
            }
        }
        // The dispatch functions resolve the store via XDG_DATA_HOME; the
        // test drives the path-scoped variant directly.
        let dispatch = |command| dispatch_audit_at(&stream, command, false);
        let status = dispatch(AuditCmd::Status).unwrap();
        assert!(status.contains("sealed segments: "), "{status}");
        assert!(status.contains("next sequence"), "{status}");

        let json = dispatch_audit_at(&stream, AuditCmd::Status, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["sealed_segments"].as_u64().unwrap() >= 1);

        let verify = dispatch(AuditCmd::Verify { full: false }).unwrap();
        assert!(verify.contains("verified"), "{verify}");
        let full = dispatch(AuditCmd::Verify { full: true }).unwrap();
        assert!(full.contains("full decompression"), "{full}");

        // Prune refuses without an export acknowledgement.
        assert!(
            dispatch(AuditCmd::Prune {
                keep: 1,
                force: false
            })
            .is_err()
        );
        dispatch(AuditCmd::Export {
            destination: "/tmp/audit-archive".into(),
        })
        .unwrap();
        let pruned = dispatch(AuditCmd::Prune {
            keep: 1,
            force: false,
        })
        .unwrap();
        assert!(pruned.contains("pruned"), "{pruned}");
    }
}
