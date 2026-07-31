//! aegis — autonomous surface shell.
//!
//! The process composition root: selects a presentation host, creates the Wayland server,
//! renderer, shell, wallpaper, configuration, and IPC surfaces, then runs the
//! compositor event and presentation loop.

use aegis_backend::Backend;
use aegis_backend::drm::DrmError;
use aegis_backend::host::{BackendKind, HardwareCursor, Host, HostError};
use std::ffi::OsString;
use std::os::fd::AsRawFd;

mod cursor;
mod runtime;

/// Compositor usage. `aegis` takes no operational arguments; presentation,
/// configuration, and control are driven by environment variables and the
/// configuration file, and a running session is driven through `aegis-ctl`.
const USAGE: &str = "\
Usage: aegis [OPTIONS]

Aegis — autonomous surface shell, a Wayland compositor and desktop shell.

The compositor takes no operational command-line arguments; presentation,
configuration, and session control are driven by environment variables, the
configuration file, and the `aegis-ctl` IPC client.

Options:
  -h, --help     Print this help and exit
  -V, --version  Print version information and exit

Environment:
  AEGIS_BACKEND=auto|drm|nested   Select the presentation backend (default: auto)
  AEGIS_WALLPAPER=PATH            Override the desktop wallpaper image
  RUST_LOG=filter                 Log filter (default: info)

Repository: <https://github.com/ming2k/aegis>";

fn main() {
    // Handle the few informational options before logging so `--help` and
    // `--version` produce clean output and never touch the display backend.
    match parse_args(std::env::args_os().skip(1)) {
        ArgResult::Run => {}
        ArgResult::Help => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        ArgResult::Version => {
            println!("aegis {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        ArgResult::Error(message) => {
            eprintln!("aegis: {message}");
            eprintln!("See `aegis --help`.");
            std::process::exit(2);
        }
    }

    // Initialize before anything logs. `RUST_LOG` controls verbosity; the
    // shared subscriber defaults to `info` so the bring-up sequence is visible
    // without configuration. See aegis-logging (ADR-0079).
    aegis_logging::init("info");

    if let Err(e) = runtime::run() {
        log::error!("aegis: {e}");
        std::process::exit(1);
    }
}

/// Outcome of scanning `argv`. Matches the `aegis-idle` convention: the only
/// accepted arguments are the informational pair, and any other token is an
/// error that points the user at `--help`.
#[derive(Debug)]
enum ArgResult {
    Run,
    Help,
    Version,
    Error(String),
}

fn parse_args(mut args: impl Iterator<Item = OsString>) -> ArgResult {
    // Only the first token matters: the compositor takes no operational
    // arguments, so any leading option is help/version or an error.
    if let Some(argument) = args.next() {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => ArgResult::Help,
            "--version" | "-V" => ArgResult::Version,
            other => ArgResult::Error(format!(
                "unexpected argument `{other}`; aegis takes no operational options \
                 (set AEGIS_BACKEND=auto|drm|nested to select a backend)"
            )),
        }
    } else {
        ArgResult::Run
    }
}

#[cfg(test)]
mod cli_tests {
    use std::ffi::OsString;

    use super::{ArgResult, parse_args};

    fn args<'a>(slice: &'a [&str]) -> impl Iterator<Item = OsString> + 'a {
        slice.iter().map(OsString::from)
    }

    #[test]
    fn no_arguments_runs() {
        assert!(matches!(parse_args(args(&[])), ArgResult::Run));
    }

    #[test]
    fn help_long_and_short() {
        assert!(matches!(parse_args(args(&["--help"])), ArgResult::Help));
        assert!(matches!(parse_args(args(&["-h"])), ArgResult::Help));
    }

    #[test]
    fn version_long_and_short() {
        assert!(matches!(
            parse_args(args(&["--version"])),
            ArgResult::Version
        ));
        assert!(matches!(parse_args(args(&["-V"])), ArgResult::Version));
    }

    #[test]
    fn unknown_argument_is_an_error() {
        match parse_args(args(&["--backend", "drm"])) {
            ArgResult::Error(message) => {
                assert!(message.contains("unexpected argument"));
                assert!(message.contains("AEGIS_BACKEND"));
            }
            other => panic!("expected error, got {other:?}"),
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
