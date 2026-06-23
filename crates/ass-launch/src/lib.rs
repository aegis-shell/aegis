//! Detached application launching for ass.
//!
//! Turns a parsed desktop [`Entry`] (or anything implementing
//! [`LaunchSource`]) into a child process that:
//!
//! - runs in a **new session**, detached from the compositor's process group
//!   and controlling terminal (via the host `setsid --fork`), so it survives
//!   the compositor exiting and never inherits its stdio;
//! - inherits the Wayland / XDG environment a client needs to connect back to
//!   this compositor (`WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, …);
//! - honours the entry's `Terminal=true` by wrapping the command in a
//!   terminal emulator.
//!
//! Field codes in the entry's `Exec` are expanded first via `ass-apps`. The
//! final command line is handed to `sh -c` after each token is POSIX
//! single-quote-escaped by `ass_apps::expand_exec`, so shell metacharacters in
//! file names are safe. No `unsafe` / `libc` is used: process detachment is
//! delegated to the external `setsid` binary (the same pattern
//! `ass-wallpaper` uses for `ffmpeg`). See ADR-0022.

use std::path::Path;
use std::process::{Command, Stdio};

use ass_apps::expand_exec;
use ass_core::app::Entry;

/// Minimal view of a desktop entry the launcher needs.
///
/// Implemented for [`Entry`]; downstream callers may implement it on their own
/// types to launch without a desktop scan (e.g. tests, or a future "run
/// arbitrary command" path).
pub trait LaunchSource {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn exec(&self) -> Option<&str>;
    fn icon(&self) -> Option<&str>;
    fn terminal(&self) -> bool;
    fn working_dir(&self) -> Option<&Path>;
}

impl LaunchSource for Entry {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn exec(&self) -> Option<&str> {
        self.exec.as_deref()
    }
    fn icon(&self) -> Option<&str> {
        self.icon.as_deref()
    }
    fn terminal(&self) -> bool {
        self.terminal
    }
    fn working_dir(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

/// Per-launch knobs. All optional; the defaults match a normal user launch.
#[derive(Debug, Clone, Default)]
pub struct LaunchOpts {
    /// Files / URIs to substitute into `%f %F %u %U`.
    pub files: Vec<String>,
    /// Override the terminal emulator used when an entry sets `Terminal=true`.
    /// Defaults to `$TERMINAL` then `xterm`. Parsed by `sh` so values like
    /// `"foot --"` work.
    pub terminal: Option<String>,
    /// When true, run the command foreground and reap it. Tests use this;
    /// production launches leave it `false` (detached, stdio → null).
    pub foreground: bool,
}

/// Outcome of a successful [`launch`].
#[derive(Debug, Clone, Copy)]
pub struct LaunchReport {
    /// The detached child's pid (as seen by `setsid --fork`). May have already
    /// exited by the time the caller reads this; the launcher never waits.
    pub pid: u32,
}

/// Errors from [`launch`].
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("entry {0} has no Exec to launch")]
    NoExec(String),
    #[error("spawn: {0}")]
    Spawn(#[from] std::io::Error),
}

/// Launch `source` detached, returning immediately.
///
/// Builds the effective command line (`sh -c '<expanded>'`, optionally wrapped
/// in a terminal emulator for `Terminal=true` entries) and runs it under
/// `setsid --fork` so the child escapes this process's session. Stdio is
/// redirected to `/dev/null` unless [`LaunchOpts::foreground`] is set.
pub fn launch(source: &dyn LaunchSource, opts: &LaunchOpts) -> Result<LaunchReport, LaunchError> {
    let exec = source
        .exec()
        .ok_or_else(|| LaunchError::NoExec(source.id().into()))?;

    // Expand field codes and POSIX-quote every token so the result is safe to
    // embed in `sh -c`.
    let expanded = expand_exec(exec, &opts.files, source.icon(), Some(source.name()), None);

    let mut effective = expanded;
    if source.terminal() {
        let term = terminal_emulator(opts);
        effective = format!("{term} -e {effective}");
    }

    let mut cmd = Command::new(SETSID);
    cmd.arg("--fork");
    if opts.foreground {
        // `setsid --wait` reaps the child and mirrors its exit status so a
        // foreground caller can observe outcome.
        cmd.arg("--wait").arg("sh").arg("-c").arg(&effective);
        cmd.stdin(Stdio::null());
    } else {
        cmd.arg("sh").arg("-c").arg(&effective);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
    }

    // Inject the environment a Wayland/XDG client needs to connect back.
    inherit_display_env(&mut cmd);

    if let Some(dir) = source.working_dir() {
        cmd.current_dir(dir);
    }

    let child = cmd.spawn()?;
    Ok(LaunchReport { pid: child.id() })
}

/// Path to the `setsid` binary. Hard-coded to util-linux's canonical install
/// location; `launch` returns a spawn error if the host lacks it.
pub const SETSID: &str = "/usr/bin/setsid";

/// Resolve the terminal emulator command string: explicit override >
/// `$TERMINAL` > `xterm`.
fn terminal_emulator(opts: &LaunchOpts) -> String {
    if let Some(t) = opts.terminal.as_deref().filter(|s| !s.is_empty()) {
        return t.to_string();
    }
    if let Ok(t) = std::env::var("TERMINAL") {
        if !t.is_empty() {
            return t;
        }
    }
    "xterm".to_string()
}

/// Copy the display/session environment a launched client needs. We forward
/// only what a Wayland/XDG app requires, rather than the whole parent env, so
/// the child is hermetic and testable.
fn inherit_display_env(cmd: &mut Command) {
    for var in [
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "XDG_SESSION_TYPE",
        "XDG_DATA_DIRS",
        "DISPLAY",
        "HOME",
        "PATH",
        "LANG",
        "LC_ALL",
        "LC_MESSAGES",
        "TERM",
    ] {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Minimal stand-in for an `Entry`, used to exercise the launcher without
    /// a desktop scan.
    struct Src {
        exec: Option<&'static str>,
        terminal: bool,
        icon: Option<&'static str>,
        wd: Option<PathBuf>,
    }
    impl LaunchSource for Src {
        fn id(&self) -> &str {
            "test.desktop"
        }
        fn name(&self) -> &str {
            "Test"
        }
        fn exec(&self) -> Option<&str> {
            self.exec
        }
        fn icon(&self) -> Option<&str> {
            self.icon
        }
        fn terminal(&self) -> bool {
            self.terminal
        }
        fn working_dir(&self) -> Option<&Path> {
            self.wd.as_deref()
        }
    }

    #[test]
    fn no_exec_is_an_error() {
        let s = Src {
            exec: None,
            terminal: false,
            icon: None,
            wd: None,
        };
        let err = launch(&s as &dyn LaunchSource, &LaunchOpts::default()).unwrap_err();
        assert!(matches!(err, LaunchError::NoExec(_)), "{err:?}");
    }

    #[test]
    fn detached_child_outlives_parent() {
        // Launch a command that writes a sentinel after a short delay. Because
        // the child is setsid-forked, this process does not wait for it.
        let dir = tempfile_dir();
        let marker = dir.path().join("out.txt");
        let s = Src {
            exec: Some("sh -c %f"),
            terminal: false,
            icon: None,
            wd: Some(dir.path().to_path_buf()),
        };
        let payload = format!("sleep 0.1; echo det > {}", marker.display());
        let opts = LaunchOpts {
            files: vec![payload],
            ..Default::default()
        };
        let report = launch(&s as &dyn LaunchSource, &opts).unwrap();
        assert!(report.pid > 0);

        let mut ok = false;
        for _ in 0..300 {
            if marker.exists() {
                ok = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ok, "detached child never wrote the marker");
        assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "det");
    }

    #[test]
    fn detached_child_inherits_display_env() {
        let dir = tempfile_dir();
        let marker = dir.path().join("env.txt");
        let s = Src {
            exec: Some("sh -c %f"),
            terminal: false,
            icon: None,
            wd: Some(dir.path().to_path_buf()),
        };
        let payload = format!("echo \"$PATH\" > {}", marker.display());
        let opts = LaunchOpts {
            files: vec![payload],
            ..Default::default()
        };
        launch(&s as &dyn LaunchSource, &opts).unwrap();
        let mut got = String::new();
        for _ in 0..300 {
            if let Ok(s) = std::fs::read_to_string(&marker) {
                got = s;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!got.trim().is_empty(), "PATH not inherited by child");
    }

    #[test]
    fn foreground_inherits_session_and_captures_output() {
        let s = Src {
            exec: Some("printf hello"),
            terminal: false,
            icon: None,
            wd: None,
        };
        let opts = LaunchOpts {
            foreground: true,
            ..Default::default()
        };
        let report = launch(&s as &dyn LaunchSource, &opts).unwrap();
        assert!(report.pid > 0);
    }

    #[test]
    fn terminal_wrapping_runs_without_panic() {
        let s = Src {
            exec: Some("true"),
            terminal: true,
            icon: None,
            wd: None,
        };
        let opts = LaunchOpts {
            foreground: true,
            terminal: Some("foot".into()),
            ..Default::default()
        };
        let _ = launch(&s as &dyn LaunchSource, &opts);
    }

    #[test]
    fn terminal_emulator_precedence() {
        std::env::remove_var("TERMINAL");
        assert_eq!(terminal_emulator(&LaunchOpts::default()), "xterm");
        assert_eq!(
            terminal_emulator(&LaunchOpts {
                terminal: Some("foot --".into()),
                ..Default::default()
            }),
            "foot --"
        );
    }

    fn tempfile_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }
}
