//! ass-ctl: the command-line driver for the ass IPC.
//!
//! The reference external tool (ADR-0027): it connects to a running
//! compositor's IPC socket and drives it — list windows/workspaces, focus,
//! close, switch workspace, toggle tiling, post a notification, quit. The
//! [`run`] entry point is unit-testable against a loopback server; the thin
//! binary in `main.rs` parses argv and prints the result.

use std::path::Path;

use ass_core::workspace::Switch;
use ass_ipc::{Capabilities, Client, Command, Event};
use serde::Serialize;

/// Serialize a query result as a JSON string (for `--json`).
fn to_json<T: Serialize>(v: &T) -> Result<String, String> {
    serde_json::to_string(v).map_err(|e| e.to_string())
}

/// Connect to `socket`, dispatch one command (`args`'s first element), and
/// return the formatted output. Errors are human-readable strings the binary
/// prints to stderr before exiting non-zero.
pub fn run(socket: &Path, args: &[String]) -> Result<String, String> {
    // `help` needs no connection; answer before we try to connect.
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    if cmd.is_empty() || cmd == "help" {
        return Ok(usage().to_string());
    }
    // `--json`/`-j` (anywhere) selects machine-readable output for the query
    // commands; strip it before dispatching.
    let json = args.iter().any(|a| a == "--json" || a == "-j");
    let args: Vec<String> = args
        .iter()
        .filter(|a| a != &"--json" && a != &"-j")
        .cloned()
        .collect();
    let mut client = Client::connect_with(
        socket,
        Capabilities {
            query: true,
            control: true,
            session: true,
        },
    )
    .map_err(|e| format!("connect: {e}"))?;
    dispatch(&mut client, &args, json)
}

fn dispatch(client: &mut Client, args: &[String], json: bool) -> Result<String, String> {
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
        "focus" => {
            let id = parse_window_id(args, 1)?;
            client.command(Command::Focus { id }).map_err(io_err)?;
            Ok(format!("focused {}", id.0))
        }
        "close" => {
            let id = parse_window_id(args, 1)?;
            client.command(Command::Close { id }).map_err(io_err)?;
            Ok(format!("close requested for {}", id.0))
        }
        "switch" => {
            let dir = parse_switch(args)?;
            client.switch_workspace(dir).map_err(io_err)?;
            Ok(format!("switched {:?}", dir))
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
            client
                .dismiss_notification(id as u64)
                .map_err(io_err)?;
            Ok(format!("dismissed {id}"))
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

fn parse_usize(args: &[String], idx: usize) -> Result<usize, String> {
    args.get(idx)
        .ok_or_else(|| format!("missing argument\n\n{}", usage()))
        .and_then(|s| {
            s.parse::<usize>()
                .map_err(|_| format!("'{s}' is not a number"))
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
            format!("journal #{} {:?}: {:?}", entry.seq, entry.origin, entry.cmd)
        }
    }
}

/// Subscribe to the event stream and print each event as a line until the
/// connection closes. Returns the error that ended the stream.
pub fn run_subscribe(socket: &Path) -> Result<(), String> {
    let mut client = Client::connect_with(
        socket,
        Capabilities {
            query: true,
            control: false,
            session: false,
        },
    )
    .map_err(|e| format!("connect: {e}"))?;
    client.subscribe().map_err(|e| format!("subscribe: {e}"))?;
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
  windows                 list visible toplevels
  workspaces              list outputs and their workspaces
  outputs                 list outputs and their geometry
  focus <id>              focus a toplevel by id
  close <id>              request a toplevel to close
  switch <next|prev>      switch workspace on the focused output
  move-to <win> <ws>      move a toplevel to a workspace (by id)
  tiling                  toggle the current workspace tiled/floating
  notify <summary> [body] post a notification
  dismiss <id>            dismiss a notification by id
  subscribe               stream server events until disconnected
  quit                    ask the compositor to quit

  --json / -j             machine-readable JSON for the query commands"
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
}
