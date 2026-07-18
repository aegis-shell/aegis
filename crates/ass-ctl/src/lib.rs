//! ass-ctl: the command-line driver for the ass IPC.
//!
//! The reference external tool (ADR-0027): it connects to a running
//! compositor's IPC socket and drives it — list windows/workspaces, focus,
//! minimize, close, switch workspace, toggle tiling, post a notification,
//! quit. The
//! [`run`] entry point is unit-testable against a loopback server; the thin
//! binary in `main.rs` parses argv and prints the result.

use std::path::Path;

use ass_core::realm::{
    HUMAN_REALM, RealmId, RealmMutation, RealmSnapshot, RealmState, SeatCapabilities, VirtualOutput,
};
use ass_core::workspace::Switch;
use ass_ipc::{Capabilities, Client, Command, Event, RealmAction, RealmActionResult};
use serde::Serialize;

/// Serialize a query result as a JSON string (for `--json`).
fn to_json<T: Serialize>(v: &T) -> Result<String, String> {
    serde_json::to_string(v).map_err(|e| e.to_string())
}

/// Connect to `socket`, dispatch one command (`args`'s first element), and
/// return the formatted output. Errors are human-readable strings the binary
/// prints to stderr before exiting non-zero.
pub fn run(socket: &Path, args: &[String]) -> Result<String, String> {
    let ParsedArgs { args, json, region } = parse_global_options(args)?;
    // Help needs no connection; answer after stripping global flags so
    // `ass-ctl --json --help` is just as local as `ass-ctl help`.
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    if cmd.is_empty() || matches!(cmd, "help" | "--help" | "-h") {
        return Ok(usage().to_string());
    }
    if region.is_some() && !matches!(cmd, "screenshot" | "realm-capture") {
        return Err(format!(
            "--region is valid only for screenshot and realm-capture\n\n{}",
            usage()
        ));
    }
    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: true,
        realm: is_realm_command(cmd),
    };
    let mut client = if is_realm_command(cmd) {
        Client::connect_scoped(socket, requested, ass_ipc::LOCAL_REALM_ADMIN_SCOPE)
    } else {
        Client::connect_with(socket, requested)
    }
    .map_err(|e| format!("connect: {e}"))?;
    dispatch(&mut client, &args, json, region)
}

/// Return the command after removing global flags. The binary uses this to
/// keep help local and route streaming commands without duplicating argv
/// indexing rules.
pub fn command_name(args: &[String]) -> Result<Option<String>, String> {
    Ok(parse_global_options(args)?.args.into_iter().next())
}

fn dispatch(
    client: &mut Client,
    args: &[String],
    json: bool,
    region: Option<ass_core::Rect>,
) -> Result<String, String> {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    match cmd {
        "windows" => {
            let wins = client.windows().map_err(io_err)?;
            if json {
                to_json(&wins)
            } else {
                Ok(format_windows(&wins))
            }
        }
        "workspaces" => {
            let snap = client.workspaces().map_err(io_err)?;
            if json {
                to_json(&snap)
            } else {
                Ok(format_workspaces(&snap))
            }
        }
        "outputs" => {
            let outs = client.outputs().map_err(io_err)?;
            if json {
                to_json(&outs)
            } else {
                Ok(format_outputs(&outs))
            }
        }
        "notifications" => {
            let notifications = client.notifications().map_err(io_err)?;
            if json {
                to_json(&notifications)
            } else {
                Ok(format_notifications(&notifications))
            }
        }
        "journal" => {
            let since = args
                .get(1)
                .map(|_| parse_u64(args, 1))
                .transpose()?
                .unwrap_or(0);
            let snapshot = client.journal(since).map_err(io_err)?;
            if json {
                to_json(&snapshot)
            } else {
                Ok(format_journal(&snapshot))
            }
        }
        "realms" => {
            let snapshot = client.realms().map_err(io_err)?;
            if json {
                to_json(&snapshot)
            } else {
                Ok(format_realms(&snapshot))
            }
        }
        "realm-create" => {
            let label = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| "AI Workspace".into());
            let result = client
                .realm_action(RealmAction::Create {
                    label,
                    capabilities: SeatCapabilities::POINTER_KEYBOARD,
                    output: Some(VirtualOutput::DEFAULT_AGENT),
                })
                .map_err(io_err)?;
            format_realm_action(result, json)
        }
        "realm-pause" | "realm-resume" => {
            let realm = parse_realm_id(args, 1)?;
            let snapshot = client.realms().map_err(io_err)?;
            let state = if cmd == "realm-pause" {
                RealmState::Paused
            } else {
                RealmState::Active
            };
            let result = client
                .realm_action(RealmAction::Transact {
                    expected_revision: Some(snapshot.revision),
                    mutations: vec![RealmMutation::SetState { realm, state }],
                })
                .map_err(io_err)?;
            format_realm_action(result, json)
        }
        "realm-transfer" => {
            let window = parse_window_id(args, 1)?;
            let target = parse_realm_id(args, 2)?;
            let retain_source_as_observer = !args.iter().any(|arg| arg == "--no-mirror");
            let snapshot = client.realms().map_err(io_err)?;
            let result = client
                .realm_action(RealmAction::Transact {
                    expected_revision: Some(snapshot.revision),
                    mutations: vec![RealmMutation::TransferWindow {
                        window,
                        target,
                        retain_source_as_observer,
                    }],
                })
                .map_err(io_err)?;
            format_realm_action(result, json)
        }
        "realm-revoke" => {
            let realm = parse_realm_id(args, 1)?;
            let fallback = args
                .get(2)
                .map(|_| parse_realm_id(args, 2))
                .transpose()?
                .unwrap_or(HUMAN_REALM);
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
        "realm-launch" => {
            let realm = parse_realm_id(args, 1)?;
            let desktop_id = args
                .get(2)
                .ok_or_else(|| format!("missing desktop id\n\n{}", usage()))?;
            client
                .launch_in_realm(realm, desktop_id.clone())
                .map_err(io_err)?;
            Ok(format!(
                "launch of {desktop_id} queued in Realm {}",
                realm.0
            ))
        }
        "realm-capture" => {
            let realm = parse_realm_id(args, 1)?;
            let capture = client.capture_realm(realm, region).map_err(io_err)?;
            let path = match args.get(2) {
                Some(path) => std::path::PathBuf::from(path),
                None => realm_capture_path(&ass_config::default_screenshot_dir(), realm)?,
            };
            atomic_write(&path, &capture.png)?;
            if json {
                to_json(&serde_json::json!({
                    "realm": capture.realm,
                    "width": capture.width,
                    "height": capture.height,
                    "scale_milli": capture.scale_milli,
                    "region": capture.region,
                    "placements": capture.placements,
                    "revision": capture.revision,
                    "path": path,
                }))
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
        "focus" => {
            let id = parse_window_id(args, 1)?;
            client.command(Command::Focus { id }).map_err(io_err)?;
            Ok(format!("focused {}", id.0))
        }
        "minimize" => {
            let id = parse_window_id(args, 1)?;
            client.command(Command::Minimize { id }).map_err(io_err)?;
            Ok(format!("minimized {}", id.0))
        }
        "close" => {
            let id = parse_window_id(args, 1)?;
            client.command(Command::Close { id }).map_err(io_err)?;
            Ok(format!("close requested for {}", id.0))
        }
        "set-geometry" => {
            let id = parse_window_id(args, 1)?;
            let rect = ass_core::Rect::new(
                parse_i32(args, 2)?,
                parse_i32(args, 3)?,
                parse_i32(args, 4)?,
                parse_i32(args, 5)?,
            );
            client.set_window_geometry(id, rect).map_err(io_err)?;
            Ok(format!(
                "set window {} geometry to {},{} {}x{}",
                id.0, rect.origin.x, rect.origin.y, rect.size.w, rect.size.h
            ))
        }
        "switch" => {
            let dir = parse_switch(args)?;
            client.switch_workspace(dir).map_err(io_err)?;
            Ok(format!("switched {:?}", dir))
        }
        "switch-to" => {
            let id = ass_core::workspace::WorkspaceId(parse_u64(args, 1)?);
            client.switch_workspace_to(id).map_err(io_err)?;
            Ok(format!("switched to workspace {}", id.0))
        }
        "move-to" => {
            let window = parse_window_id(args, 1)?;
            let ws = parse_usize(args, 2)?;
            client
                .command(Command::MoveToWorkspace {
                    window,
                    workspace: ass_core::workspace::WorkspaceId(ws as u64),
                })
                .map_err(io_err)?;
            Ok(format!("moved window {} to workspace {ws}", window.0))
        }
        "tiling" => {
            client.toggle_tiling().map_err(io_err)?;
            Ok("toggled tiling".into())
        }
        "notify" => {
            let (summary, body) = parse_notify(args)?;
            client.notify(summary, body, None).map_err(io_err)?;
            Ok("notified".into())
        }
        "dismiss" => {
            let id = parse_usize(args, 1)?;
            client.dismiss_notification(id as u64).map_err(io_err)?;
            Ok(format!("dismissed {id}"))
        }
        "screenshot" => {
            // Keep the CLI default aligned with the compositor's interactive
            // screenshot destination.
            let path = match args.get(1) {
                Some(path) => path.clone(),
                None => screenshot_path(&ass_config::default_screenshot_dir())?,
            };
            client
                .screenshot_region(path.clone(), region)
                .map_err(io_err)?;
            Ok(format!("screenshot queued → {path}"))
        }
        "overview" => {
            client.command(Command::ToggleOverview).map_err(io_err)?;
            Ok("toggled overview".into())
        }
        "quit" => {
            client.command(Command::Quit).map_err(io_err)?;
            Ok("quit requested".into())
        }
        "help" | "" => Ok(usage().to_string()),
        other => Err(format!("unknown command '{other}'\n\n{}", usage())),
    }
}

fn io_err(e: std::io::Error) -> String {
    e.to_string()
}

fn is_realm_command(command: &str) -> bool {
    command == "realms" || command.starts_with("realm-")
}

fn parse_realm_id(args: &[String], idx: usize) -> Result<RealmId, String> {
    let id = parse_u64(args, idx)?;
    if id == 0 {
        return Err("Realm id zero is invalid".into());
    }
    Ok(RealmId(id))
}

fn format_realm_action(result: RealmActionResult, json: bool) -> Result<String, String> {
    if json {
        return to_json(&result);
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

fn realm_capture_path(dir: &Path, realm: RealmId) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("create screenshot directory {}: {error}", dir.display()))?;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    Ok(dir.join(format!("ass-realm-{}-{ms}.png", realm.0)))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("capture path {} has no file name", path.display()))?
        .to_string_lossy();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary = parent.join(format!(".{file_name}.{}.{}.tmp", std::process::id(), nonce));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("create {}: {error}", temporary.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, path).map_err(|error| {
            format!(
                "commit capture {} → {}: {error}",
                temporary.display(),
                path.display()
            )
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Generate a timestamped screenshot path and ensure its parent exists.
fn screenshot_path(dir: &Path) -> Result<String, String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("create screenshot directory {}: {error}", dir.display()))?;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    Ok(dir
        .join(format!("ass-screenshot-{ms}.png"))
        .to_string_lossy()
        .into_owned())
}

struct ParsedArgs {
    args: Vec<String>,
    json: bool,
    region: Option<ass_core::Rect>,
}

/// Parse global flags in one pass so stripping `--json` cannot shift the index
/// used to remove `--region` and its value. Malformed or duplicate regions are
/// rejected rather than silently broadening a requested crop to a full-output
/// capture.
fn parse_global_options(args: &[String]) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs {
        args: Vec::with_capacity(args.len()),
        json: false,
        region: None,
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" | "-j" => parsed.json = true,
            "--region" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--region requires x,y,w,h".to_owned())?;
                if parsed.region.is_some() {
                    return Err("--region may be specified only once".into());
                }
                parsed.region = Some(parse_region(value)?);
                index += 1;
            }
            value if value.starts_with("--region=") => {
                if parsed.region.is_some() {
                    return Err("--region may be specified only once".into());
                }
                parsed.region = Some(parse_region(&value["--region=".len()..])?);
            }
            _ => parsed.args.push(args[index].clone()),
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_region(value: &str) -> Result<ass_core::Rect, String> {
    let parts = value.split(',').collect::<Vec<_>>();
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
    Ok(ass_core::Rect::new(
        numbers[0], numbers[1], numbers[2], numbers[3],
    ))
}

fn parse_usize(args: &[String], idx: usize) -> Result<usize, String> {
    args.get(idx)
        .ok_or_else(|| format!("missing argument\n\n{}", usage()))
        .and_then(|s| {
            s.parse::<usize>()
                .map_err(|_| format!("'{s}' is not a number"))
        })
}

fn parse_u64(args: &[String], idx: usize) -> Result<u64, String> {
    args.get(idx)
        .ok_or_else(|| format!("missing argument\n\n{}", usage()))
        .and_then(|s| {
            s.parse::<u64>()
                .map_err(|_| format!("'{s}' is not a number"))
        })
}

fn parse_i32(args: &[String], idx: usize) -> Result<i32, String> {
    args.get(idx)
        .ok_or_else(|| format!("missing argument\n\n{}", usage()))
        .and_then(|s| {
            s.parse::<i32>()
                .map_err(|_| format!("'{s}' is not a signed 32-bit number"))
        })
}

/// Parse a window id argument (ADR-0032). The wire encoding is a JSON
/// number, so the CLI accepts any `u64`.
fn parse_window_id(args: &[String], idx: usize) -> Result<ass_core::window::WindowId, String> {
    args.get(idx)
        .ok_or_else(|| format!("missing argument\n\n{}", usage()))
        .and_then(|s| {
            s.parse::<u64>()
                .map(ass_core::window::WindowId)
                .map_err(|_| format!("'{s}' is not a number"))
        })
}

fn parse_switch(args: &[String]) -> Result<Switch, String> {
    match args.get(1).map(String::as_str) {
        Some("next") => Ok(Switch::Next),
        Some("prev") | Some("previous") => Ok(Switch::Prev),
        Some(other) => Err(format!("switch direction must be next/prev, not '{other}'")),
        None => Err(format!("missing direction\n\n{}", usage())),
    }
}

fn parse_notify(args: &[String]) -> Result<(String, String), String> {
    let summary = args
        .get(1)
        .ok_or_else(|| format!("usage: ass-ctl notify <summary> [body]\n\n{}", usage()))?
        .clone();
    let body = args.get(2).cloned().unwrap_or_default();
    Ok((summary, body))
}

fn format_windows(wins: &[ass_core::window::Window]) -> String {
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

fn format_workspaces(snap: &ass_core::workspace::WorkspaceSnapshot) -> String {
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

fn format_outputs(outs: &[ass_core::output::OutputInfo]) -> String {
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
        // Advertised modes are what `mode = "WxH@Hz"` may name in the
        // config; mark the live one. Backends with a continuous size
        // (nested) report none, and the line is omitted.
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

fn format_notifications(notifications: &[ass_core::notify::Notification]) -> String {
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

fn format_journal(snapshot: &ass_ipc::JournalSnapshot) -> String {
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
        Event::RealmDamaged {
            realm,
            sequence,
            revision,
            ..
        } => format!(
            "realm {} damaged {} at authority revision {}",
            realm.0, sequence, revision
        ),
    }
}

/// Subscribe to the event stream and print each event as a line until the
/// connection closes. Returns the error that ended the stream.
pub fn run_subscribe(socket: &Path) -> Result<(), String> {
    run_stream(socket, false)
}

/// Subscribe to the detailed mutation journal and print entries until the
/// connection closes.
pub fn run_subscribe_journal(socket: &Path) -> Result<(), String> {
    run_stream(socket, true)
}

fn run_stream(socket: &Path, journal: bool) -> Result<(), String> {
    let mut client = Client::connect_with(
        socket,
        Capabilities {
            query: true,
            control: false,
            input: false,
            session: false,
            realm: false,
        },
    )
    .map_err(|e| format!("connect: {e}"))?;
    if journal {
        client
            .subscribe_journal()
            .map_err(|e| format!("subscribe journal: {e}"))?;
    } else {
        client.subscribe().map_err(|e| format!("subscribe: {e}"))?;
    }
    loop {
        let ev = client
            .next_event()
            .map_err(|e| format!("event stream ended: {e}"))?;
        println!("{}", format_event(&ev));
    }
}

fn usage() -> &'static str {
    "usage: ass-ctl <command> [args]

commands:
  help, --help, -h         show this help without connecting
  windows                 list visible toplevels
  workspaces              list outputs and their workspaces
  outputs                 list outputs and their geometry
  notifications           list active notifications
  journal [since]         list mutation journal entries after a sequence
  realms                  list authority domains, seats, and controlled windows
  realm-create [label]    create an isolated AI workspace
  realm-pause <realm>     disable a Realm seat and freeze its authority
  realm-resume <realm>    re-enable a paused Realm
  realm-transfer <win> <realm> [--no-mirror]
                           atomically transfer a window interaction group
  realm-launch <realm> <desktop-id>
                           launch an app through the Realm process sandbox
  realm-capture <realm> [path.png]
                           capture a Realm virtual output atomically
  realm-revoke <realm> [fallback]
                           permanently revoke a Realm (fallback defaults to 1)
  focus <id>              focus a toplevel by id
  minimize <id>           minimize a toplevel by id
  close <id>              request a toplevel to close
  set-geometry <id> <x> <y> <w> <h>
                           set floating geometry in logical pixels
  switch <next|prev>      switch workspace on the focused output
  switch-to <ws>          switch directly to a workspace (by id)
  move-to <win> <ws>      move a toplevel to a workspace (by id)
  tiling                  toggle the current workspace tiled/floating
  notify <summary> [body] post a notification
  dismiss <id>            dismiss a notification by id
  screenshot [path.png]   capture the focused output to a PNG file
  screenshot --region x,y,w,h [path.png]
                           capture a region of the focused output
  overview                toggle the window/workspace overview
  subscribe               stream server events until disconnected
  subscribe-journal       stream detailed mutation events
  quit                    ask the compositor to quit

  --json / -j             machine-readable JSON for the query commands
  --region x,y,w,h        capture a region instead of the full output"
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::notify::Notification;

    #[test]
    fn format_event_windows_and_workspace() {
        assert_eq!(format_event(&Event::WindowsChanged), "windows changed");
        assert_eq!(format_event(&Event::WorkspaceChanged), "workspace changed");
    }

    #[test]
    fn format_event_notified_with_and_without_body() {
        let with_body = Event::Notified {
            notification: Notification {
                id: 7,
                summary: "ping".into(),
                body: "hello".into(),
                app_id: None,
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
                at_ms: 0,
            },
        };
        assert_eq!(format_event(&no_body), "notify #8: beep");
    }

    #[test]
    fn global_flags_are_stripped_without_index_aliasing() {
        let args = vec![
            "--json".to_string(),
            "--region".to_string(),
            "10,20,100,80".to_string(),
            "screenshot".to_string(),
            "capture.png".to_string(),
        ];
        let parsed = parse_global_options(&args).unwrap();
        assert!(parsed.json);
        assert_eq!(parsed.region, Some(ass_core::Rect::new(10, 20, 100, 80)));
        assert_eq!(parsed.args, ["screenshot", "capture.png"]);

        assert!(parse_global_options(&["--region".into(), "1,2,3".into()]).is_err());
        assert!(parse_global_options(&["--region".into(), "a,b,c,d".into()]).is_err());
        assert!(parse_global_options(&["--region=1,2,0,4".into()]).is_err());
    }

    #[test]
    fn screenshot_path_uses_lowercase_directory_and_creates_it() {
        let dir = std::env::temp_dir().join(format!(
            "ass-ctl-screenshots-{}-{}",
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
