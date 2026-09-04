//! tessera — autonomous surface shell.
//!
//! With no subcommand, the process composition root selects a presentation
//! host, creates the Wayland server, renderer, shell, wallpaper,
//! configuration, and IPC surfaces, then runs the compositor loop. Resource
//! subcommands dispatch to a running session without entering that runtime.

use tessera_backend::Backend;
use tessera_backend::drm::DrmError;
use tessera_backend::host::{BackendKind, HardwareCursor, Host, HostError};
use std::os::fd::AsRawFd;
use std::process::ExitCode;

mod cursor;
mod runtime;

fn main() -> ExitCode {
    // Parse before logging or backend initialization so help, version, and
    // client commands never touch compositor runtime state.
    let cli = match tessera_commands::parse_env() {
        Ok(cli) => cli,
        Err(error) => {
            let use_stderr = error.use_stderr();
            error.print().expect("print clap message");
            return ExitCode::from(if use_stderr { 2 } else { 0 });
        }
    };

    if cli.runs_compositor() {
        if cli.json {
            eprintln!("error: --json requires a session-management command");
            return ExitCode::from(2);
        }
        return run_compositor();
    }

    run_session_command(cli)
}

fn run_compositor() -> ExitCode {
    // `RUST_LOG` controls verbosity; compositor bring-up is visible by default.
    tessera_logging::init("info");
    match runtime::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::error!("tessera: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_session_command(cli: tessera_commands::Cli) -> ExitCode {
    // One-shot commands print results, not a log stream.
    tessera_logging::init("warn");
    let local_only = matches!(
        cli.command.as_ref(),
        Some(
            tessera_commands::Command::Completions { .. }
                | tessera_commands::Command::Config { .. }
                | tessera_commands::Command::Audit { .. },
        )
    );
    let socket = if local_only {
        std::path::PathBuf::new()
    } else {
        match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(directory) => std::path::PathBuf::from(directory).join("tessera.sock"),
            None => {
                eprintln!("tessera: $XDG_RUNTIME_DIR is unset; cannot locate the running session");
                return ExitCode::from(2);
            }
        }
    };

    match tessera_commands::run_with(&socket, cli) {
        Ok(output) if !output.is_empty() => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tessera: {error}");
            ExitCode::from(error.exit_code() as u8)
        }
    }
}

/// Persistent (level) input state carried across frames. Per-frame edges
/// (mouse pressed/released, scroll, text, key events) are *not* held here;
/// they are built fresh each frame from backend events and live only for the
/// iteration. This matches lens's contract that the host owns edge derivation
/// (see the `lens::Input` docstring) and mirrors iris's wayland host
/// `drain_input` pattern. Keeping level state separate from per-frame edges
/// guarantees a press/release edge can never leak into the next frame and
/// trigger phantom clicks in immediate-mode widgets.
#[derive(Default)]
struct InputAccumulator {
    cursor: (f32, f32),
    mouse_down: [bool; 3],
    display_size: (f32, f32),
}

impl InputAccumulator {
    /// Mirror of `lens::Input::set_mouse_down` so callers can update the
    /// level state alongside the per-frame snapshot through the same
    /// `lens::MouseButton` key.
    fn set_mouse_down(&mut self, b: lens::MouseButton, down: bool) {
        let idx = match b {
            lens::MouseButton::Left => 0,
            lens::MouseButton::Right => 1,
            lens::MouseButton::Middle => 2,
        };
        self.mouse_down[idx] = down;
    }
}
