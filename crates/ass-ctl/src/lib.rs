//! ass-ctl: the command-line driver for the ass IPC.
//!
//! The reference external tool (ADR-0027): it connects to a running
//! compositor's IPC socket and drives it — list windows/workspaces, focus,
//! close, switch workspace, toggle tiling, post a notification, quit. The
//! [`run`] entry point is unit-testable against a loopback server; the thin
//! binary in `main.rs` parses argv and prints the result.

use std::path::Path;

use ass_core::workspace::Switch;
use ass_ipc::{Capabilities, Client, Command};

/// Connect to `socket`, dispatch one command (`args`'s first element), and
/// return the formatted output. Errors are human-readable strings the binary
/// prints to stderr before exiting non-zero.
pub fn run(socket: &Path, args: &[String]) -> Result<String, String> {
    // `help` needs no connection; answer before we try to connect.
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    if cmd.is_empty() || cmd == "help" {
        return Ok(usage().to_string());
    }
    let mut client = Client::connect_with(
        socket,
        Capabilities {
            query: true,
            control: true,
            session: true,
        },
    )
    .map_err(|e| format!("connect: {e}"))?;
    dispatch(&mut client, args)
}

fn dispatch(client: &mut Client, args: &[String]) -> Result<String, String> {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    match cmd {
        "windows" => {
            let wins = client.windows().map_err(io_err)?;
            Ok(format_windows(&wins))
        }
        "workspaces" => {
            let snap = client.workspaces().map_err(io_err)?;
            Ok(format_workspaces(&snap))
        }
        "focus" => {
            let id = parse_usize(args, 1)?;
            client.command(Command::Focus { id }).map_err(io_err)?;
            Ok(format!("focused {id}"))
        }
        "close" => {
            let id = parse_usize(args, 1)?;
            client.command(Command::Close { id }).map_err(io_err)?;
            Ok(format!("close requested for {id}"))
        }
        "switch" => {
            let dir = parse_switch(args)?;
            client.switch_workspace(dir).map_err(io_err)?;
            Ok(format!("switched {:?}", dir))
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
        out.push_str(&format!("{mark}{:<14} {} ({})\n", w.id, title, app));
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

fn usage() -> &'static str {
    "usage: ass-ctl <command> [args]

commands:
  windows                 list visible toplevels
  workspaces              list outputs and their workspaces
  focus <id>              focus a toplevel by id
  close <id>              request a toplevel to close
  switch <next|prev>      switch workspace on the focused output
  tiling                  toggle the current workspace tiled/floating
  notify <summary> [body] post a notification
  quit                    ask the compositor to quit"
}
