//! Detached application launching for aegis.
//!
//! Turns a parsed desktop [`Entry`] (or anything implementing
//! [`LaunchSource`]) into a child process that:
//!
//! - runs ordinary desktop launches in a **new session** through
//!   `setsid --fork`, detached from compositor lifetime and stdio;
//! - runs Interaction Domain launches as directly tracked bubblewrap process trees that
//!   pause, resume, and terminate with Interaction Domain authority;
//! - inherits the Wayland / XDG environment a client needs to connect back to
//!   this compositor (`WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, …);
//! - honours the entry's `Terminal=true` by wrapping the command in a
//!   terminal emulator.
//!
//! Field codes in the entry's `Exec` are expanded first via `aegis-apps`. The
//! final command line is handed to `sh -c` after each token is POSIX
//! single-quote-escaped by `aegis_desktop_entries::expand_exec`, so shell metacharacters in
//! file names are safe. Ordinary process detachment is delegated to the
//! external `setsid` binary. The managed Interaction Domain path uses a delegated cgroup v2
//! subtree to control the complete sandbox process tree. See ADR-0022.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use aegis_core::app::Entry;
use aegis_desktop_entries::expand_exec;

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
#[derive(Debug, Default)]
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
    /// Explicit Wayland socket name for ordinary launches. Compositors should
    /// set this instead of mutating the process environment after worker
    /// threads have started. When absent, the current `WAYLAND_DISPLAY` is
    /// inherited for compatibility with standalone callers.
    pub wayland_display: Option<String>,
    /// Optional fail-closed Linux namespace sandbox. Interaction Domain launches should
    /// always set this; ordinary user launches retain the compatibility
    /// default of no process sandbox.
    pub sandbox: Option<InteractionDomainSandbox>,
}

/// Process-isolation policy for an application launched into a compositor
/// Interaction Domain. Bubblewrap is used as the small, audited namespace/mount broker;
/// absence or setup failure rejects the launch instead of falling back to an
/// unsandboxed process.
#[derive(Debug)]
pub struct InteractionDomainSandbox {
    pub interaction_domain_id: u64,
    /// Compositor-created listener whose socket inode is bind-mounted into the
    /// sandbox. The short-lived host path is unlinked before application code
    /// is released, so only this mount namespace retains reachability.
    pub wayland_listener: UnixListener,
    pub wayland_socket_path: PathBuf,
    pub app_id: String,
    /// Kernel-enforced resource budget for this launch.
    pub limits: InteractionDomainResourceLimits,
}

/// Default-deny resource budget installed on the sandbox's delegated cgroup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionDomainResourceLimits {
    pub memory_max_bytes: u64,
    pub pids_max: u32,
    pub cpu_weight: u16,
}

impl Default for InteractionDomainResourceLimits {
    fn default() -> Self {
        Self {
            memory_max_bytes: 8 * 1024 * 1024 * 1024,
            pids_max: 1024,
            cpu_weight: 100,
        }
    }
}

/// Outcome of a successful [`launch`].
#[derive(Debug, Clone, Copy)]
pub struct LaunchReport {
    /// Spawned supervisor pid. For [`launch`] this is the `setsid` child and may
    /// already have exited; for [`launch_managed`] it is the directly tracked
    /// bubblewrap supervisor.
    pub pid: u32,
    pub sandboxed: bool,
    /// Whether the requested memory, PID, and CPU hard budget was installed.
    /// This is always true for a successful managed Interaction Domain launch; ordinary
    /// compatibility launches report false because they have no Interaction Domain budget.
    pub resource_limits_enforced: bool,
}

/// An Interaction Domain sandbox process tree owned by the compositor.
///
/// The child is bubblewrap's namespace supervisor and cgroup root process.
/// Dropping the handle kills and reaps the complete sandbox; callers can also
/// suspend/resume it in lockstep with Interaction Domain authority.
pub struct ManagedLaunch {
    report: LaunchReport,
    child: std::process::Child,
    cgroup: ProcessCgroup,
}

impl ManagedLaunch {
    pub fn report(&self) -> LaunchReport {
        self.report
    }

    pub fn is_running(&mut self) -> std::io::Result<bool> {
        Ok(self.child.try_wait()?.is_none() || self.cgroup.populated()?)
    }

    pub fn pause(&mut self) -> std::io::Result<()> {
        self.cgroup.freeze(true)
    }

    pub fn resume(&mut self) -> std::io::Result<()> {
        self.cgroup.freeze(false)
    }

    /// Fail-closed termination used by Interaction Domain revocation and compositor
    /// shutdown. SIGKILL is deliberate: revocation is an authority boundary,
    /// not a graceful application-close request.
    pub fn terminate(&mut self) -> std::io::Result<()> {
        self.cgroup.kill_all()?;
        if self.child.try_wait()?.is_none() {
            let _ = self.child.wait()?;
        }
        self.cgroup.wait_empty(std::time::Duration::from_secs(1))
    }
}

impl Drop for ManagedLaunch {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

struct ProcessCgroup {
    path: PathBuf,
    procs: std::fs::File,
}

static INTERACTION_DOMAIN_CGROUP_ROOT: std::sync::OnceLock<Result<PathBuf, String>> =
    std::sync::OnceLock::new();

/// Prepare the compositor's delegated cgroup v2 topology before it launches
/// ordinary applications.
///
/// systemd delegates controller authority at the service cgroup, but cgroup v2
/// forbids enabling domain controllers while that same node contains a
/// process. Aegis therefore moves itself into an `aegis-host-*` leaf and keeps
/// Interaction Domain sandboxes as sibling children of the delegated service root.
pub fn prepare_interaction_domain_host() -> Result<PathBuf, LaunchError> {
    INTERACTION_DOMAIN_CGROUP_ROOT
        .get_or_init(initialize_interaction_domain_cgroup_root)
        .clone()
        .map_err(LaunchError::CgroupUnavailable)
}

fn initialize_interaction_domain_cgroup_root() -> Result<PathBuf, String> {
    let membership = std::fs::read_to_string("/proc/self/cgroup")
        .map_err(|error| format!("read /proc/self/cgroup: {error}"))?;
    let relative = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| "unified cgroup v2 entry is missing".to_owned())?;
    let relative = relative.strip_prefix('/').unwrap_or(relative);
    let root = Path::new("/sys/fs/cgroup")
        .join(relative)
        .canonicalize()
        .map_err(|error| format!("resolve current cgroup: {error}"))?;
    if !root.starts_with("/sys/fs/cgroup") {
        return Err("current cgroup resolved outside the cgroup2 mount".into());
    }

    let required = ["cpu", "memory", "pids"];
    let available = std::fs::read_to_string(root.join("cgroup.controllers"))
        .map_err(|error| format!("read delegated controllers: {error}"))?;
    if required
        .iter()
        .any(|controller| !available.split_whitespace().any(|item| item == *controller))
    {
        return Err(
            "systemd must delegate cpu, memory, and pids controllers to the Aegis service".into(),
        );
    }
    let enabled = std::fs::read_to_string(root.join("cgroup.subtree_control"))
        .map_err(|error| format!("read cgroup.subtree_control: {error}"))?;
    if required
        .iter()
        .all(|controller| enabled.split_whitespace().any(|item| item == *controller))
    {
        return Ok(root);
    }

    let pid = std::process::id().to_string();
    let members = std::fs::read_to_string(root.join("cgroup.procs"))
        .map_err(|error| format!("read cgroup.procs: {error}"))?;
    if members.lines().any(|member| member != pid) {
        return Err(
            "Aegis must run in its own delegated systemd service before Interaction Domain controllers can be enabled"
                .into(),
        );
    }

    let host = root.join(format!("aegis-host-{}", std::process::id()));
    std::fs::create_dir(&host).map_err(|error| format!("create Aegis host cgroup: {error}"))?;
    if let Err(error) = std::fs::write(host.join("cgroup.procs"), b"0") {
        let _ = std::fs::remove_dir(&host);
        return Err(format!("move compositor into host cgroup: {error}"));
    }
    if let Err(error) = std::fs::write(root.join("cgroup.subtree_control"), b"+cpu +memory +pids") {
        let _ = std::fs::write(root.join("cgroup.procs"), b"0");
        let _ = std::fs::remove_dir(&host);
        return Err(format!(
            "enable delegated cpu/memory/pids controllers: {error}"
        ));
    }
    Ok(root)
}

impl ProcessCgroup {
    fn create(
        interaction_domain: u64,
        limits: InteractionDomainResourceLimits,
    ) -> Result<Self, LaunchError> {
        static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let root = prepare_interaction_domain_host()?;
        let nonce = NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = root.join(format!(
            "aegis-interaction-domain-{interaction_domain}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).map_err(cgroup_error)?;
        let setup = (|| -> std::io::Result<std::fs::File> {
            // Lifecycle and resource controls are all security boundaries.
            // Missing controller delegation rejects the launch instead of
            // silently admitting an unbounded agent process tree.
            for required in [
                "cgroup.freeze",
                "cgroup.kill",
                "cgroup.events",
                "memory.max",
                "memory.oom.group",
                "memory.swap.max",
                "pids.max",
                "cpu.weight",
            ] {
                if !path.join(required).exists() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        format!("required cgroup v2 control {required} is unavailable"),
                    ));
                }
            }
            std::fs::write(path.join("memory.max"), limits.memory_max_bytes.to_string())?;
            std::fs::write(path.join("memory.oom.group"), b"1")?;
            std::fs::write(path.join("memory.swap.max"), b"0")?;
            std::fs::write(path.join("pids.max"), limits.pids_max.to_string())?;
            std::fs::write(path.join("cpu.weight"), limits.cpu_weight.to_string())?;
            let procs = std::fs::OpenOptions::new()
                .write(true)
                .open(path.join("cgroup.procs"))?;
            Ok(procs)
        })();
        match setup {
            Ok(procs) => Ok(Self { path, procs }),
            Err(error) => {
                let _ = std::fs::remove_dir(&path);
                Err(cgroup_error(error))
            }
        }
    }

    fn attach_on_exec(&self, command: &mut Command) {
        let procs = self.procs.as_raw_fd();
        // SAFETY: `pre_exec` runs after fork. The closure performs only one
        // async-signal-safe `write(2)` to a pre-opened cgroup.procs fd. Writing
        // "0" moves the calling child itself before it can exec or fork.
        unsafe {
            command.pre_exec(move || {
                let attached = libc::write(procs, b"0".as_ptr().cast(), 1);
                if attached == 1 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
    }

    fn verify_member(&self, pid: u32) -> Result<(), LaunchError> {
        let members =
            std::fs::read_to_string(self.path.join("cgroup.procs")).map_err(cgroup_error)?;
        if members.lines().any(|member| member == pid.to_string()) {
            Ok(())
        } else {
            Err(LaunchError::CgroupUnavailable(format!(
                "sandbox supervisor {pid} did not enter its delegated cgroup"
            )))
        }
    }

    fn freeze(&self, frozen: bool) -> std::io::Result<()> {
        std::fs::write(
            self.path.join("cgroup.freeze"),
            if frozen { b"1" } else { b"0" },
        )?;
        let expected = if frozen { "1" } else { "0" };
        for _ in 0..200 {
            let events = std::fs::read_to_string(self.path.join("cgroup.events"))?;
            if events
                .lines()
                .any(|line| line.strip_prefix("frozen ") == Some(expected))
            {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "cgroup freezer did not acknowledge the requested state",
        ))
    }

    fn kill_all(&self) -> std::io::Result<()> {
        std::fs::write(self.path.join("cgroup.kill"), b"1")
    }

    fn populated(&self) -> std::io::Result<bool> {
        let events = std::fs::read_to_string(self.path.join("cgroup.events"))?;
        Ok(events
            .lines()
            .any(|line| line.strip_prefix("populated ") == Some("1")))
    }

    fn wait_empty(&self, timeout: std::time::Duration) -> std::io::Result<()> {
        let started = std::time::Instant::now();
        while self.populated()? {
            if started.elapsed() >= timeout {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "sandbox cgroup remained populated after cgroup.kill",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(())
    }
}

impl Drop for ProcessCgroup {
    fn drop(&mut self) {
        for _ in 0..20 {
            if self.populated().is_ok_and(|populated| !populated)
                && std::fs::remove_dir(&self.path).is_ok()
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        log::warn!(
            "could not remove sandbox cgroup {} after termination",
            self.path.display()
        );
    }
}

fn cgroup_error(error: std::io::Error) -> LaunchError {
    LaunchError::CgroupUnavailable(error.to_string())
}

fn validate_resource_limits(limits: InteractionDomainResourceLimits) -> Result<(), LaunchError> {
    if !(256 * 1024 * 1024..=1024_u64 * 1024 * 1024 * 1024).contains(&limits.memory_max_bytes)
        || !(16..=65_536).contains(&limits.pids_max)
        || !(1..=10_000).contains(&limits.cpu_weight)
    {
        return Err(LaunchError::InvalidSandbox(
            "memory, process, or CPU cgroup limit is outside the supported range".into(),
        ));
    }
    Ok(())
}

/// Errors from [`launch`].
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("entry {0} has no Exec to launch")]
    NoExec(String),
    #[error("spawn: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("entry {entry} exited with {status}")]
    Exit {
        entry: String,
        status: std::process::ExitStatus,
    },
    #[error("Interaction Domain sandbox is unavailable: {0}")]
    SandboxUnavailable(String),
    #[error("Interaction Domain cgroup isolation is unavailable: {0}")]
    CgroupUnavailable(String),
    #[error("invalid Interaction Domain sandbox policy: {0}")]
    InvalidSandbox(String),
}

/// Launch `source` detached, returning immediately.
///
/// Builds the effective command line (`sh -c '<expanded>'`, optionally wrapped
/// in a terminal emulator for `Terminal=true` entries) and runs it under
/// `setsid --fork` so the child escapes this process's session. Stdio is
/// redirected to `/dev/null` unless [`LaunchOpts::foreground`] is set.
pub fn launch(source: &dyn LaunchSource, opts: &LaunchOpts) -> Result<LaunchReport, LaunchError> {
    if opts.sandbox.is_some() {
        return Err(LaunchError::InvalidSandbox(
            "Interaction Domain sandboxes require launch_managed so their portal and cgroup stay supervised"
                .into(),
        ));
    }
    let effective = effective_command(source, opts)?;

    let mut cmd = Command::new(SETSID);
    cmd.arg("--fork");
    if opts.foreground {
        // `setsid --wait` reaps the child and mirrors its exit status so a
        // foreground caller can observe outcome.
        cmd.arg("--wait");
        append_effective_command(&mut cmd, &effective, opts)?;
    } else {
        append_effective_command(&mut cmd, &effective, opts)?;
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
    }
    cmd.stdin(Stdio::null());

    // Inject the environment a Wayland/XDG client needs to connect back.
    inherit_display_env(&mut cmd, opts.wayland_display.as_deref());

    if let Some(dir) = source.working_dir() {
        cmd.current_dir(dir);
    }

    let mut child = cmd.spawn()?;
    let pid = child.id();
    if opts.foreground {
        let status = child.wait()?;
        if !status.success() {
            return Err(LaunchError::Exit {
                entry: source.id().into(),
                status,
            });
        }
    } else {
        std::thread::Builder::new()
            .name("aegis-launch-reaper".into())
            .spawn(move || {
                let _ = child.wait();
            })?;
    }
    Ok(LaunchReport {
        pid,
        sandboxed: false,
        resource_limits_enforced: false,
    })
}

/// Launch one Interaction Domain application under a compositor-owned process supervisor.
///
/// Unlike ordinary detached launches this never uses the compatibility
/// `setsid --fork` wrapper: bubblewrap is the directly tracked child, enters a
/// dedicated cgroup, and dies automatically if the compositor exits.
pub fn launch_managed(
    source: &dyn LaunchSource,
    opts: &LaunchOpts,
) -> Result<ManagedLaunch, LaunchError> {
    let sandbox = opts.sandbox.as_ref().ok_or_else(|| {
        LaunchError::InvalidSandbox("managed launch requires an Interaction Domain sandbox".into())
    })?;
    if opts.foreground {
        return Err(LaunchError::InvalidSandbox(
            "managed launch cannot run in foreground mode".into(),
        ));
    }
    let effective = effective_command(source, opts)?;
    validate_resource_limits(sandbox.limits)?;
    validate_interaction_domain_portal(sandbox)?;
    let _portal_cleanup = PortalPathCleanup(&sandbox.wayland_socket_path);
    let cgroup = ProcessCgroup::create(sandbox.interaction_domain_id, sandbox.limits)?;
    let mut command = Command::new(BWRAP);
    append_bubblewrap_args(&mut command, &effective, sandbox, true)?;
    command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cgroup.attach_on_exec(&mut command);
    let child = command.spawn()?;
    let report = LaunchReport {
        pid: child.id(),
        sandboxed: true,
        resource_limits_enforced: true,
    };
    let mut launch = ManagedLaunch {
        report,
        child,
        cgroup,
    };
    launch.cgroup.verify_member(report.pid)?;
    wait_for_sandbox_portal_gate(&mut launch.child)?;
    unlink_and_drain_interaction_domain_portal(sandbox)?;
    release_sandbox_application(&mut launch.child)?;
    Ok(launch)
}

fn effective_command(source: &dyn LaunchSource, opts: &LaunchOpts) -> Result<String, LaunchError> {
    let exec = source
        .exec()
        .ok_or_else(|| LaunchError::NoExec(source.id().into()))?;
    // Expand field codes and POSIX-quote every token so the result is safe to
    // embed in `sh -c`.
    let expanded = expand_exec(exec, &opts.files, source.icon(), Some(source.name()), None);
    if source.terminal() {
        Ok(format!("{} -e {expanded}", terminal_emulator(opts)))
    } else {
        Ok(expanded)
    }
}

/// Path to the `setsid` binary. Hard-coded to util-linux's canonical install
/// location; `launch` returns a spawn error if the host lacks it.
pub const SETSID: &str = "/usr/bin/setsid";
pub const BWRAP: &str = "/usr/bin/bwrap";

fn append_effective_command(
    command: &mut Command,
    effective: &str,
    _opts: &LaunchOpts,
) -> Result<(), LaunchError> {
    command.arg("sh").arg("-c").arg(effective);
    Ok(())
}

fn append_bubblewrap_args(
    command: &mut Command,
    effective: &str,
    sandbox: &InteractionDomainSandbox,
    die_with_parent: bool,
) -> Result<(), LaunchError> {
    if sandbox.interaction_domain_id == 0
        || sandbox.app_id.trim().is_empty()
        || sandbox.app_id.len() > 256
        || !sandbox
            .app_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(LaunchError::InvalidSandbox(
            "interaction_domain id or app id is invalid".into(),
        ));
    }
    validate_interaction_domain_portal(sandbox)?;
    if !Path::new(BWRAP).is_file() {
        return Err(LaunchError::SandboxUnavailable(format!(
            "{BWRAP} is not installed"
        )));
    }
    let runtime = "/run/aegis";
    let home = format!("/tmp/aegis-home-{}", sandbox.interaction_domain_id);

    command.args([
        "--unshare-all",
        // Bubblewrap 0.11 requires this explicit spelling when
        // `--disable-userns` is used, even though `--unshare-all` already
        // includes the user namespace semantically.
        "--unshare-user",
        "--disable-userns",
        "--assert-userns-disabled",
        "--new-session",
        "--cap-drop",
        "ALL",
        "--clearenv",
    ]);
    if die_with_parent {
        command.arg("--die-with-parent");
    }
    command
        .args([
            "--proc", "/proc", "--dev", "/dev", "--dir", "/dev/dri", "--tmpfs", "/dev/shm",
            "--tmpfs", "/tmp", "--dir", "/run", "--dir", "/sys", "--dir",
        ])
        .arg(runtime)
        .arg("--dir")
        .arg(&home)
        .arg("--setenv")
        .arg("HOME")
        .arg(&home)
        .arg("--setenv")
        .arg("XDG_RUNTIME_DIR")
        .arg(runtime)
        .arg("--setenv")
        .arg("WAYLAND_DISPLAY")
        .arg("wayland-0")
        .arg("--setenv")
        .arg("XDG_SESSION_TYPE")
        .arg("wayland")
        .arg("--setenv")
        .arg("AEGIS_INTERACTION_DOMAIN_ID")
        .arg(sandbox.interaction_domain_id.to_string())
        .arg("--setenv")
        .arg("AEGIS_SANDBOX_APP_ID")
        .arg(&sandbox.app_id)
        .arg("--setenv")
        .arg("PATH")
        .arg("/usr/local/bin:/usr/bin:/bin")
        .arg("--setenv")
        .arg("XDG_DATA_DIRS")
        .arg("/usr/local/share:/usr/share");

    command
        .arg("--bind")
        .arg(&sandbox.wayland_socket_path)
        .arg("/run/aegis/wayland-0");

    for variable in ["LANG", "LC_ALL", "LC_MESSAGES", "TERM"] {
        if let Ok(value) = std::env::var(variable) {
            command.arg("--setenv").arg(variable).arg(value);
        }
    }

    for path in ["/usr", "/bin", "/lib", "/lib64"] {
        if Path::new(path).exists() {
            command.arg("--ro-bind").arg(path).arg(path);
        }
    }
    // GPU and device discovery need these read-only views. Do not expose
    // `/sys/fs`, `/sys/kernel`, or the host cgroup filesystem.
    for path in ["/sys/bus", "/sys/class", "/sys/dev", "/sys/devices"] {
        if Path::new(path).exists() {
            command.arg("--ro-bind").arg(path).arg(path);
        }
    }
    for path in [
        "/etc/alternatives",
        "/etc/fonts",
        "/etc/group",
        "/etc/ld.so.cache",
        "/etc/ld.so.conf",
        "/etc/ld.so.conf.d",
        "/etc/localtime",
        "/etc/machine-id",
        "/etc/nsswitch.conf",
        "/etc/passwd",
        "/etc/pki",
        "/etc/ssl",
    ] {
        if Path::new(path).exists() {
            command.arg("--ro-bind").arg(path).arg(path);
        }
    }
    // Render nodes provide GPU acceleration without exposing KMS card nodes.
    if let Ok(entries) = std::fs::read_dir("/dev/dri") {
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with("renderD") {
                command.arg("--dev-bind").arg(&path).arg(&path);
            }
        }
    }
    // The marker is emitted only after bubblewrap has finished its namespace
    // and mount setup. The parent then unlinks the host portal path, drops any
    // connection queued before that unlink, and releases this read gate.
    command
        .arg("--")
        .arg("sh")
        .arg("-c")
        .arg("printf '\\036'; exec >/dev/null 2>&1; IFS= read -r _; exec sh -c \"$1\"")
        .arg("aegis-interaction-domain-launch")
        .arg(effective);
    Ok(())
}

struct PortalPathCleanup<'a>(&'a Path);

impl Drop for PortalPathCleanup<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
        if let Some(parent) = self.0.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

fn validate_interaction_domain_portal(
    sandbox: &InteractionDomainSandbox,
) -> Result<(), LaunchError> {
    let path = &sandbox.wayland_socket_path;
    if !path.is_absolute() {
        return Err(LaunchError::InvalidSandbox(
            "Wayland portal path must be absolute".into(),
        ));
    }
    sandbox
        .wayland_listener
        .set_nonblocking(true)
        .map_err(|error| {
            LaunchError::InvalidSandbox(format!(
                "could not make Wayland portal listener non-blocking: {error}"
            ))
        })?;
    let local = sandbox.wayland_listener.local_addr().map_err(|error| {
        LaunchError::InvalidSandbox(format!("inspect Wayland portal listener: {error}"))
    })?;
    if local.as_pathname() != Some(path.as_path()) {
        return Err(LaunchError::InvalidSandbox(
            "Wayland portal listener and mount path do not match".into(),
        ));
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        LaunchError::InvalidSandbox(format!("inspect Wayland portal path: {error}"))
    })?;
    use std::os::unix::fs::FileTypeExt as _;
    if !metadata.file_type().is_socket() {
        return Err(LaunchError::InvalidSandbox(
            "Wayland portal path is not a Unix socket".into(),
        ));
    }

    let mut accepting: libc::c_int = 0;
    let mut length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: the output pointer and length describe a live `c_int`; the live
    // UnixListener owns the descriptor for the duration of the call.
    let result = unsafe {
        libc::getsockopt(
            sandbox.wayland_listener.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_ACCEPTCONN,
            (&mut accepting as *mut libc::c_int).cast(),
            &mut length,
        )
    };
    if result != 0 || accepting != 1 {
        return Err(LaunchError::InvalidSandbox(
            "Wayland portal is not a listening Unix socket".into(),
        ));
    }
    Ok(())
}

fn wait_for_sandbox_portal_gate(child: &mut std::process::Child) -> Result<(), LaunchError> {
    let stdout = child.stdout.as_mut().ok_or_else(|| {
        LaunchError::SandboxUnavailable("sandbox setup channel is missing".into())
    })?;
    let mut marker = [0u8; 1];
    stdout.read_exact(&mut marker).map_err(|error| {
        LaunchError::SandboxUnavailable(format!(
            "sandbox exited before installing its private Wayland portal: {error}"
        ))
    })?;
    if marker != [0x1e] {
        return Err(LaunchError::SandboxUnavailable(
            "sandbox emitted an invalid portal-ready marker".into(),
        ));
    }
    Ok(())
}

fn unlink_and_drain_interaction_domain_portal(
    sandbox: &InteractionDomainSandbox,
) -> Result<(), LaunchError> {
    std::fs::remove_file(&sandbox.wayland_socket_path).map_err(|error| {
        LaunchError::SandboxUnavailable(format!(
            "could not remove ambient Wayland portal path: {error}"
        ))
    })?;
    if let Some(parent) = sandbox.wayland_socket_path.parent() {
        std::fs::remove_dir(parent).map_err(|error| {
            LaunchError::SandboxUnavailable(format!(
                "could not remove ambient Wayland portal directory: {error}"
            ))
        })?;
    }
    loop {
        match sandbox.wayland_listener.accept() {
            Ok((connection, _)) => drop(connection),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => {
                return Err(LaunchError::SandboxUnavailable(format!(
                    "could not clear pre-gate Wayland portal connections: {error}"
                )));
            }
        }
    }
}

fn release_sandbox_application(child: &mut std::process::Child) -> Result<(), LaunchError> {
    let mut stdin = child.stdin.take().ok_or_else(|| {
        LaunchError::SandboxUnavailable("sandbox release channel is missing".into())
    })?;
    stdin.write_all(b"\n").map_err(|error| {
        LaunchError::SandboxUnavailable(format!("could not release sandbox application: {error}"))
    })?;
    stdin.flush().map_err(|error| {
        LaunchError::SandboxUnavailable(format!("could not flush sandbox release gate: {error}"))
    })?;
    drop(stdin);
    drop(child.stdout.take());
    Ok(())
}

/// Resolve the terminal emulator command string: explicit override >
/// `$TERMINAL` > `xterm`.
fn terminal_emulator(opts: &LaunchOpts) -> String {
    let env_terminal = std::env::var("TERMINAL").ok();
    terminal_emulator_with_env(opts, env_terminal.as_deref())
}

fn terminal_emulator_with_env(opts: &LaunchOpts, env_terminal: Option<&str>) -> String {
    if let Some(t) = opts.terminal.as_deref().filter(|s| !s.is_empty()) {
        return t.to_string();
    }
    if let Some(t) = env_terminal.filter(|t| !t.is_empty()) {
        return t.to_string();
    }
    "xterm".to_string()
}

/// Copy the display/session environment a launched client needs. We forward
/// only what a Wayland/XDG app requires, rather than the whole parent env, so
/// the child is hermetic and testable. `DBUS_SESSION_BUS_ADDRESS` must ride
/// along: without it D-Bus clients (portals, `flatpak-spawn --watch-bus`)
/// fall back to a disabled bus and fail to launch.
fn inherit_display_env(cmd: &mut Command, wayland_display: Option<&str>) {
    cmd.env_clear();
    for var in [
        "XDG_RUNTIME_DIR",
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "XDG_DATA_DIRS",
        "DBUS_SESSION_BUS_ADDRESS",
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
    if let Some(display) = wayland_display {
        cmd.env("WAYLAND_DISPLAY", display);
    } else if let Ok(display) = std::env::var("WAYLAND_DISPLAY") {
        cmd.env("WAYLAND_DISPLAY", display);
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

        let supervisor = std::path::PathBuf::from(format!("/proc/{}/stat", report.pid));
        for _ in 0..300 {
            if !supervisor.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            !supervisor.exists(),
            "setsid supervisor {} was not reaped",
            report.pid
        );
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
    fn session_bus_and_desktop_vars_are_forwarded() {
        let mut cmd = Command::new("true");
        inherit_display_env(&mut cmd, Some("wayland-9"));
        let forwarded: std::collections::BTreeMap<String, Option<String>> = cmd
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            forwarded.get("WAYLAND_DISPLAY").and_then(|v| v.as_deref()),
            Some("wayland-9")
        );
        // Only read the shared test process environment; never mutate it.
        // Each whitelisted session variable that is set must reach the child.
        for var in [
            "XDG_RUNTIME_DIR",
            "XDG_CURRENT_DESKTOP",
            "DBUS_SESSION_BUS_ADDRESS",
        ] {
            if let Ok(value) = std::env::var(var) {
                assert_eq!(
                    forwarded.get(var).and_then(|v| v.as_deref()),
                    Some(value.as_str()),
                    "{var} not forwarded"
                );
            }
        }
    }

    #[test]
    fn foreground_waits_for_completion() {
        let dir = tempfile_dir();
        let marker = dir.path().join("foreground.txt");
        let s = Src {
            exec: Some("sh -c %f"),
            terminal: false,
            icon: None,
            wd: Some(dir.path().to_path_buf()),
        };
        let opts = LaunchOpts {
            files: vec!["printf done > foreground.txt".into()],
            foreground: true,
            ..Default::default()
        };
        let report = launch(&s as &dyn LaunchSource, &opts).unwrap();
        assert!(report.pid > 0);
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "done");
    }

    #[test]
    fn foreground_reports_nonzero_exit() {
        let s = Src {
            exec: Some("exit 7"),
            terminal: false,
            icon: None,
            wd: None,
        };
        let opts = LaunchOpts {
            foreground: true,
            ..Default::default()
        };
        let err = launch(&s as &dyn LaunchSource, &opts).unwrap_err();
        assert!(matches!(err, LaunchError::Exit { .. }), "{err:?}");
    }

    #[test]
    fn terminal_wrapping_runs_headlessly() {
        let s = Src {
            exec: Some("true"),
            terminal: true,
            icon: None,
            wd: None,
        };
        let opts = LaunchOpts {
            foreground: true,
            // `true` accepts the generated `-e <command>` arguments without
            // opening a real terminal window or depending on the host's
            // graphical session.
            terminal: Some("true".into()),
            ..Default::default()
        };
        launch(&s as &dyn LaunchSource, &opts).expect("headless terminal wrapper");
    }

    #[test]
    fn terminal_emulator_precedence() {
        assert_eq!(
            terminal_emulator_with_env(&LaunchOpts::default(), None),
            "xterm"
        );
        assert_eq!(
            terminal_emulator_with_env(&LaunchOpts::default(), Some("foot")),
            "foot"
        );
        assert_eq!(
            terminal_emulator_with_env(
                &LaunchOpts {
                    terminal: Some("foot --".into()),
                    ..Default::default()
                },
                Some("ignored"),
            ),
            "foot --"
        );
    }

    #[test]
    fn interaction_domain_sandbox_is_fail_closed_and_parent_bound() {
        if !Path::new(BWRAP).is_file() {
            return;
        }
        let (_portal_dir, portal, portal_path) = wayland_portal();
        let sandbox = InteractionDomainSandbox {
            interaction_domain_id: 9,
            wayland_listener: portal,
            wayland_socket_path: portal_path,
            app_id: "test.desktop".into(),
            limits: InteractionDomainResourceLimits::default(),
        };
        let mut command = Command::new(BWRAP);
        append_bubblewrap_args(&mut command, "true", &sandbox, true).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.iter().any(|arg| arg == "--unshare-all"));
        assert!(args.iter().any(|arg| arg == "--unshare-user"));
        assert!(args.iter().any(|arg| arg == "--disable-userns"));
        assert!(args.iter().any(|arg| arg == "--assert-userns-disabled"));
        assert!(args.iter().any(|arg| arg == "--die-with-parent"));
        assert!(args.iter().any(|arg| arg == "--cap-drop"));
        assert!(!args.iter().any(|arg| arg == "--share-net"));
        assert!(args.windows(2).any(|pair| pair == ["--tmpfs", "/dev/shm"]));
    }

    #[test]
    fn sandbox_portal_is_host_unlinked_and_accepts_multiple_connections() {
        if !Path::new(BWRAP).is_file() || !Path::new("/usr/bin/wayland-info").is_file() {
            return;
        }
        if let Err(error) = prepare_interaction_domain_host() {
            eprintln!("skipping real Interaction Domain portal test: {error}");
            return;
        }
        let (_portal_dir, portal, portal_path) = wayland_portal();
        let compositor_listener = portal.try_clone().unwrap();
        let removed_path = portal_path.clone();
        let mut pre_gate_connection = std::os::unix::net::UnixStream::connect(&portal_path)
            .expect("model a same-UID pre-gate connection");
        let source = Src {
            exec: Some("sh -c %f"),
            terminal: false,
            icon: None,
            wd: None,
        };
        let opts = LaunchOpts {
            files: vec!["wayland-info & wayland-info & wait".into()],
            sandbox: Some(InteractionDomainSandbox {
                interaction_domain_id: 10,
                wayland_listener: portal,
                wayland_socket_path: portal_path,
                app_id: "test.desktop".into(),
                limits: InteractionDomainResourceLimits::default(),
            }),
            ..Default::default()
        };
        let mut launch = launch_managed(&source, &opts).unwrap();
        assert!(
            !removed_path.exists(),
            "portal must have no host pathname after the sandbox gate opens"
        );
        pre_gate_connection
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let mut discarded = [0u8; 1];
        match pre_gate_connection.read(&mut discarded) {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                ) => {}
            result => panic!("pre-gate connection remained usable: {result:?}"),
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut accepted = 0;
        while accepted < 2 && std::time::Instant::now() < deadline {
            match compositor_listener.accept() {
                Ok((connection, _)) => {
                    drop(connection);
                    accepted += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => panic!("portal accept failed: {error}"),
            }
        }
        assert_eq!(
            accepted, 2,
            "one sandboxed application instance must be able to open multiple Wayland connections"
        );
        launch.terminate().unwrap();
    }

    #[test]
    fn managed_interaction_domain_process_can_be_paused_resumed_and_revoked() {
        if !Path::new(BWRAP).is_file() {
            return;
        }
        if let Err(error) = prepare_interaction_domain_host() {
            eprintln!("skipping real Interaction Domain cgroup test: {error}");
            return;
        }
        let (_portal_dir, portal, portal_path) = wayland_portal();
        let source = Src {
            exec: Some("sh -c %f"),
            terminal: false,
            icon: None,
            wd: None,
        };
        let opts = LaunchOpts {
            // The worker deliberately escapes the bubblewrap supervisor's
            // process group/session. Only the cgroup freezer/kill boundary can
            // still control it as one unit.
            files: vec!["setsid sh -c 'while :; do sleep 1; done' & wait".into()],
            sandbox: Some(InteractionDomainSandbox {
                interaction_domain_id: 11,
                wayland_listener: portal,
                wayland_socket_path: portal_path,
                app_id: "test.desktop".into(),
                limits: InteractionDomainResourceLimits::default(),
            }),
            ..Default::default()
        };
        let mut launch = launch_managed(&source, &opts).unwrap();
        assert!(
            launch.report().resource_limits_enforced,
            "managed launches must fail instead of dropping resource controls"
        );
        assert_eq!(
            std::fs::read_to_string(launch.cgroup.path.join("memory.max"))
                .unwrap()
                .trim(),
            InteractionDomainResourceLimits::default()
                .memory_max_bytes
                .to_string()
        );
        assert_eq!(
            std::fs::read_to_string(launch.cgroup.path.join("pids.max"))
                .unwrap()
                .trim(),
            InteractionDomainResourceLimits::default()
                .pids_max
                .to_string()
        );
        assert_eq!(
            std::fs::read_to_string(launch.cgroup.path.join("cpu.weight"))
                .unwrap()
                .trim(),
            InteractionDomainResourceLimits::default()
                .cpu_weight
                .to_string()
        );
        assert_eq!(
            std::fs::read_to_string(launch.cgroup.path.join("memory.oom.group"))
                .unwrap()
                .trim(),
            "1"
        );
        assert_eq!(
            std::fs::read_to_string(launch.cgroup.path.join("memory.swap.max"))
                .unwrap()
                .trim(),
            "0"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            launch.is_running().unwrap(),
            "bubblewrap exited during sandbox setup"
        );
        launch.pause().unwrap();
        assert!(
            std::fs::read_to_string(launch.cgroup.path.join("cgroup.events"))
                .unwrap()
                .lines()
                .any(|line| line == "frozen 1"),
            "complete Interaction Domain cgroup was not frozen"
        );
        launch.resume().unwrap();
        assert!(
            std::fs::read_to_string(launch.cgroup.path.join("cgroup.events"))
                .unwrap()
                .lines()
                .any(|line| line == "frozen 0"),
            "complete Interaction Domain cgroup did not resume"
        );
        launch.terminate().unwrap();
        assert!(!launch.is_running().unwrap());
    }

    fn tempfile_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn wayland_portal() -> (tempfile::TempDir, UnixListener, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wayland");
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        (directory, listener, path)
    }
}
