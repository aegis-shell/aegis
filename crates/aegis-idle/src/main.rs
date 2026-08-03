mod dimmer;
mod logind;

use std::ffi::OsString;
use std::io;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use aegis_idle::{IdlePolicy, IdleStage};
use dimmer::Dimmer;
use logind::SleepEvent;
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    protocol::{wl_registry, wl_seat},
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};

const CONTROL_MESSAGE_LOCK: &[u8] = b"LOCK";
const CONTROL_MESSAGE_PING: &[u8] = b"PING";
const CONTROL_MESSAGE_STOP: &[u8] = b"STOP";
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POWER_RETRY_INITIAL: Duration = Duration::from_millis(250);
const POWER_RETRY_MAX: Duration = Duration::from_secs(5);
const INHIBITOR_RETRY_INITIAL: Duration = Duration::from_secs(1);
const INHIBITOR_RETRY_MAX: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputPower {
    On,
    Off,
}

impl OutputPower {
    fn powered(self) -> bool {
        self == Self::On
    }
}

#[derive(Debug, Clone, Copy)]
struct PowerRetry {
    target: OutputPower,
    not_before: Instant,
    delay: Duration,
}

enum SleepInhibitor {
    Disabled,
    Held {
        _fd: zbus::zvariant::OwnedFd,
    },
    AwaitingResume,
    Retry {
        not_before: Instant,
        delay: Duration,
    },
}

impl SleepInhibitor {
    fn initial(enabled: bool, now: Instant) -> Self {
        if !enabled {
            return Self::Disabled;
        }
        match logind::acquire_delay_inhibitor() {
            Ok(fd) => {
                log::debug!("idle: acquired logind sleep delay inhibitor");
                Self::Held { _fd: fd }
            }
            Err(error) => {
                log::warn!(
                    "idle: sleep delay inhibitor unavailable; retrying in {:.0}s: {error}",
                    INHIBITOR_RETRY_INITIAL.as_secs_f32()
                );
                Self::Retry {
                    not_before: now + INHIBITOR_RETRY_INITIAL,
                    delay: INHIBITOR_RETRY_INITIAL,
                }
            }
        }
    }

    fn enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }

    /// Drop the delay file descriptor only after a secure lock frame is
    /// confirmed. A failed acquisition also enters the sleep boundary so it
    /// is not retried while logind is already preparing the transition.
    fn release_for_sleep(&mut self) -> bool {
        let held = matches!(self, Self::Held { .. });
        if self.enabled() {
            *self = Self::AwaitingResume;
        }
        held
    }

    fn resumed(&mut self, now: Instant) {
        if self.enabled() {
            *self = Self::Retry {
                not_before: now,
                delay: INHIBITOR_RETRY_INITIAL,
            };
        }
    }

    fn maintain(&mut self, now: Instant) {
        let Self::Retry { not_before, delay } = self else {
            return;
        };
        if now < *not_before {
            return;
        }
        let previous_delay = *delay;
        match logind::acquire_delay_inhibitor() {
            Ok(fd) => {
                log::debug!("idle: acquired logind sleep delay inhibitor");
                *self = Self::Held { _fd: fd };
            }
            Err(error) => {
                let next_delay = previous_delay.saturating_mul(2).min(INHIBITOR_RETRY_MAX);
                log::warn!(
                    "idle: sleep delay inhibitor unavailable; retrying in {:.0}s: {error}",
                    next_delay.as_secs_f32()
                );
                *self = Self::Retry {
                    not_before: now + next_delay,
                    delay: next_delay,
                };
            }
        }
    }
}

struct LockProcess {
    child: Child,
    ready: Option<OwnedFd>,
    confirmed: bool,
}

struct Daemon {
    policy: IdlePolicy,
    notifier: Option<ext_idle_notifier_v1::ExtIdleNotifierV1>,
    seat: Option<wl_seat::WlSeat>,
    notifications: Vec<ext_idle_notification_v1::ExtIdleNotificationV1>,
    armed: bool,
    control: UnixDatagram,
    ipc_socket: PathBuf,
    dimmer: Dimmer,
    lock_process: Option<LockProcess>,
    lock_desired: bool,
    retry_lock_at: Instant,
    display_off_pending: bool,
    output_power: OutputPower,
    power_retry: Option<PowerRetry>,
    suspend_pending: bool,
    suspend_sent: bool,
    preparing_sleep: bool,
    sleep_inhibitor: SleepInhibitor,
    sleep_events: Receiver<SleepEvent>,
    exit: bool,
}

fn main() {
    aegis_logging::init("info");
    match run() {
        Ok(()) => {}
        Err(error) => {
            log::error!("idle: {error}");
            eprintln!("aegis-idle: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse(std::env::args_os().skip(1))?;
    if options.lock_now {
        request_lock(&options.control_socket)?;
        return Ok(());
    }
    let policy = options.policy.validate()?;
    let control = bind_control_socket(&options.control_socket)?;
    let _socket_guard = SocketGuard::new(options.control_socket.clone())?;
    let (sleep_tx, sleep_rx) = mpsc::channel();
    if options.logind {
        logind::spawn_signal_monitor(sleep_tx);
    }
    let sleep_inhibitor = SleepInhibitor::initial(options.logind, Instant::now());

    let connection = Connection::connect_to_env()?;
    let mut event_queue = connection.new_event_queue();
    let qh = event_queue.handle();
    connection.display().get_registry(&qh, ());
    let now = Instant::now();
    let mut daemon = Daemon {
        policy,
        notifier: None,
        seat: None,
        notifications: Vec::new(),
        armed: false,
        control,
        ipc_socket: options.ipc_socket,
        dimmer: Dimmer::new(policy.dim_percent),
        lock_process: None,
        lock_desired: false,
        retry_lock_at: now,
        display_off_pending: false,
        output_power: OutputPower::On,
        power_retry: None,
        suspend_pending: false,
        suspend_sent: false,
        preparing_sleep: false,
        sleep_inhibitor,
        sleep_events: sleep_rx,
        exit: false,
    };
    event_queue.roundtrip(&mut daemon)?;
    daemon.arm(&qh);
    if !daemon.armed {
        return Err("compositor does not advertise ext-idle-notify-v1 and a seat".into());
    }
    log::info!("idle: staged policy armed");

    while !daemon.exit {
        event_queue.dispatch_pending(&mut daemon)?;
        daemon.maintenance();
        connection.flush()?;

        let Some(guard) = connection.prepare_read() else {
            continue;
        };
        let mut poll_fds = [
            libc::pollfd {
                fd: connection.as_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: daemon.control.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: daemon
                    .lock_process
                    .as_ref()
                    .and_then(|process| process.ready.as_ref())
                    .map(AsRawFd::as_raw_fd)
                    .unwrap_or(-1),
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            },
        ];
        let result = unsafe {
            libc::poll(
                poll_fds.as_mut_ptr(),
                poll_fds.len() as libc::nfds_t,
                POLL_INTERVAL.as_millis() as i32,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        }
        if poll_fds[0].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP) != 0 {
            guard.read()?;
        } else {
            drop(guard);
        }
        if poll_fds[1].revents & libc::POLLIN != 0 {
            daemon.read_control();
        }
        if poll_fds[2].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            daemon.read_lock_ready();
        }
    }
    daemon.power_retry = None;
    daemon.power_outputs(OutputPower::On);
    Ok(())
}

impl Daemon {
    fn arm(&mut self, qh: &QueueHandle<Self>) {
        if self.armed {
            return;
        }
        let (Some(notifier), Some(seat)) = (self.notifier.as_ref(), self.seat.as_ref()) else {
            return;
        };
        for (stage, timeout) in self.policy.stages() {
            let millis = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
            self.notifications
                .push(notifier.get_idle_notification(millis, seat, qh, stage));
        }
        self.armed = true;
    }

    fn stage_idled(&mut self, stage: IdleStage) {
        match stage {
            IdleStage::Dim => self.dimmer.dim(),
            IdleStage::Lock => {
                self.dimmer.restore();
                self.require_lock();
            }
            IdleStage::DisplayOff => {
                self.display_off_pending = true;
                self.require_lock();
            }
            IdleStage::Suspend => {
                self.suspend_pending = true;
                self.require_lock();
            }
        }
        self.apply_secure_actions();
    }

    fn resumed(&mut self) {
        self.dimmer.restore();
        self.display_off_pending = false;
        self.suspend_pending = false;
        self.suspend_sent = false;
        self.reconcile_output_power();
    }

    fn require_lock(&mut self) {
        self.lock_desired = true;
        self.try_start_lock();
    }

    fn try_start_lock(&mut self) {
        if self.lock_process.is_some() || !self.lock_desired || Instant::now() < self.retry_lock_at
        {
            return;
        }
        match start_locker() {
            Ok(process) => {
                log::info!("idle: session lock requested");
                self.lock_process = Some(process);
            }
            Err(error) => {
                log::error!("idle: could not start aegis-lock: {error}");
                self.retry_lock_at = Instant::now() + Duration::from_secs(5);
            }
        }
    }

    fn read_lock_ready(&mut self) {
        let Some(process) = self.lock_process.as_mut() else {
            return;
        };
        let ready = match consume_lock_ready(&mut process.ready) {
            Ok(ready) => ready,
            Err(error) => {
                log::warn!("idle: lock confirmation pipe failed: {error}");
                false
            }
        };
        if ready && !process.confirmed {
            process.confirmed = true;
            log::info!("idle: compositor confirmed the secure lock frame");
            self.apply_secure_actions();
        }
    }

    fn lock_confirmed(&self) -> bool {
        self.lock_process
            .as_ref()
            .is_some_and(|process| process.confirmed)
    }

    fn apply_secure_actions(&mut self) {
        if !self.lock_confirmed() {
            return;
        }
        if self.preparing_sleep && self.sleep_inhibitor.release_for_sleep() {
            log::debug!("idle: secure frame visible; releasing sleep delay inhibitor");
        }
        self.reconcile_output_power();
        if self.sleep_inhibitor.enabled() && self.suspend_pending && !self.suspend_sent {
            self.suspend_sent = true;
            logind::suspend_async();
        }
    }

    fn maintenance(&mut self) {
        self.read_sleep_events();
        self.check_locker();
        self.try_start_lock();
        self.reconcile_output_power();
        if !self.preparing_sleep {
            self.sleep_inhibitor.maintain(Instant::now());
        }
    }

    fn check_locker(&mut self) {
        let status = self
            .lock_process
            .as_mut()
            .and_then(|process| process.child.try_wait().ok())
            .flatten();
        let Some(status) = status else {
            return;
        };
        let process = self.lock_process.take().expect("status came from process");
        if process.confirmed && !status.success() {
            self.reconcile_output_power();
            self.retry_lock_at = Instant::now() + Duration::from_secs(5);
            log::error!(
                "idle: locker exited unexpectedly after securing the session; compositor remains fail-closed and a replacement is scheduled"
            );
        } else if process.confirmed {
            log::info!("idle: session unlocked");
            self.lock_desired = false;
        } else {
            log::warn!("idle: locker exited before secure presentation ({status})");
            self.retry_lock_at = Instant::now() + Duration::from_secs(5);
        }
    }

    fn read_sleep_events(&mut self) {
        while let Ok(event) = self.sleep_events.try_recv() {
            match event {
                SleepEvent::Preparing => {
                    log::info!("idle: system is preparing to sleep; requiring secure lock");
                    self.preparing_sleep = true;
                    self.require_lock();
                    self.apply_secure_actions();
                }
                SleepEvent::Resumed => {
                    log::info!("idle: system resumed; waking outputs behind lock");
                    self.preparing_sleep = false;
                    self.resumed();
                    self.sleep_inhibitor.resumed(Instant::now());
                }
            }
        }
    }

    fn power_outputs(&mut self, target: OutputPower) {
        if self.output_power == target {
            return;
        }
        let now = Instant::now();
        if power_retry_blocked(self.power_retry, target, now) {
            return;
        }
        let requested = aegis_ipc::ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        };
        let result = aegis_ipc::Client::connect_with_timeout(
            &self.ipc_socket,
            requested,
            // The compositor bounds the authoritative main-loop receipt at
            // two seconds. Leave transport headroom so that timeout can be
            // returned as a real refusal rather than racing the socket timer.
            Duration::from_secs(3),
        )
        .and_then(|mut client| {
            client.apply_system_action(aegis_core::system::SystemAction::SetOutputPower {
                powered: target.powered(),
            })
        });
        match result {
            Ok(()) => {
                self.output_power = target;
                self.power_retry = None;
                log::info!(
                    "idle: outputs {}",
                    match target {
                        OutputPower::On => "woken",
                        OutputPower::Off => "powered off",
                    }
                );
            }
            Err(error) => {
                let retry = next_power_retry(self.power_retry, target, Instant::now());
                self.power_retry = Some(retry);
                log::warn!(
                    "idle: output power request failed; retrying in {:.2}s: {error}",
                    retry.delay.as_secs_f32()
                );
            }
        }
    }

    fn reconcile_output_power(&mut self) {
        let powered = desired_output_power(self.lock_confirmed(), self.display_off_pending);
        self.power_outputs(powered);
    }

    fn read_control(&mut self) {
        let mut message = [0u8; 32];
        while let Ok(length) = self.control.recv(&mut message) {
            match &message[..length] {
                CONTROL_MESSAGE_LOCK => self.require_lock(),
                CONTROL_MESSAGE_PING => {}
                CONTROL_MESSAGE_STOP => self.exit = true,
                _ => log::debug!("idle: ignored unknown control datagram"),
            }
        }
    }
}

fn consume_lock_ready(ready: &mut Option<OwnedFd>) -> io::Result<bool> {
    let Some(fd) = ready.as_ref() else {
        return Ok(false);
    };
    let mut byte = 0u8;
    let read = unsafe { libc::read(fd.as_raw_fd(), std::ptr::from_mut(&mut byte).cast(), 1) };
    if read > 0 {
        // The pipe carries exactly one one-byte confirmation. Closing our
        // endpoint after consuming it prevents a permanent POLLHUP from
        // turning the daemon into a busy loop.
        ready.take();
        return Ok(true);
    }
    if read == 0 {
        ready.take();
        return Ok(false);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        ready.take();
        Err(error)
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Daemon {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _connection: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "ext_idle_notifier_v1" if state.notifier.is_none() => {
                    state.notifier = Some(registry.bind(name, version.min(2), qh, ()));
                    state.arm(qh);
                }
                "wl_seat" if state.seat.is_none() => {
                    state.seat = Some(registry.bind(name, version.min(9), qh, ()));
                    state.arm(qh);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, IdleStage> for Daemon {
    fn event(
        state: &mut Self,
        _notification: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        stage: &IdleStage,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.stage_idled(*stage),
            ext_idle_notification_v1::Event::Resumed => state.resumed(),
            _ => unreachable!(),
        }
    }
}

delegate_noop!(Daemon: ignore wl_seat::WlSeat);
delegate_noop!(Daemon: ignore ext_idle_notifier_v1::ExtIdleNotifierV1);

struct SocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl SocketGuard {
    fn new(path: PathBuf) -> io::Result<Self> {
        match path.symlink_metadata() {
            Ok(metadata) => Ok(Self {
                path,
                device: metadata.dev(),
                inode: metadata.ino(),
            }),
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                Err(error)
            }
        }
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if self.path.symlink_metadata().is_ok_and(|metadata| {
            metadata.file_type().is_socket()
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
        }) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn bind_control_socket(path: &Path) -> io::Result<UnixDatagram> {
    if let Ok(metadata) = path.symlink_metadata() {
        if !metadata.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("refusing to replace non-socket {}", path.display()),
            ));
        }
        let probe = UnixDatagram::unbound()?;
        match probe
            .connect(path)
            .and_then(|()| probe.send(CONTROL_MESSAGE_PING).map(|_| ()))
        {
            Ok(()) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another aegis-idle instance is running",
                ));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                ) => {}
            Err(error) => return Err(error),
        }
        if let Err(error) = std::fs::remove_file(path)
            && error.kind() != io::ErrorKind::NotFound
        {
            return Err(error);
        }
    }
    let socket = UnixDatagram::bind(path)?;
    socket.set_nonblocking(true)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(socket)
}

fn request_lock(socket: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let control = UnixDatagram::unbound()?;
    if control.connect(socket).is_ok() && control.send(CONTROL_MESSAGE_LOCK).is_ok() {
        return Ok(());
    }
    log::warn!("idle: daemon unavailable; starting a standalone locker");
    Command::new(locker_program()?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()?;
    Ok(())
}

fn start_locker() -> io::Result<LockProcess> {
    let mut descriptors = [-1; 2];
    if unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let read = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
    let flags = unsafe { libc::fcntl(write.as_raw_fd(), libc::F_GETFD) };
    if flags < 0
        || unsafe { libc::fcntl(write.as_raw_fd(), libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let child = Command::new(locker_program()?)
        .args(["--ready-fd", &write.as_raw_fd().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()?;
    drop(write);
    Ok(LockProcess {
        child,
        ready: Some(read),
        confirmed: false,
    })
}

fn locker_program() -> io::Result<OsString> {
    let path = std::env::current_exe()?.with_file_name("aegis-lock");
    if path.is_file() {
        Ok(path.into_os_string())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "trusted sibling aegis-lock binary is missing",
        ))
    }
}

struct Options {
    policy: IdlePolicy,
    ipc_socket: PathBuf,
    control_socket: PathBuf,
    lock_now: bool,
    logind: bool,
}

impl Options {
    fn parse(args: impl Iterator<Item = OsString>) -> Result<Self, Box<dyn std::error::Error>> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or("$XDG_RUNTIME_DIR is unset")?;
        let mut options = Self {
            policy: IdlePolicy::default(),
            ipc_socket: runtime.join("aegis.sock"),
            control_socket: runtime.join("aegis-idle.sock"),
            lock_now: false,
            logind: true,
        };
        let mut args = args.map(|arg| arg.to_string_lossy().into_owned());
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--lock-now" => options.lock_now = true,
                "--no-logind" => options.logind = false,
                "--socket" => {
                    options.ipc_socket = PathBuf::from(args.next().ok_or("--socket needs a path")?);
                }
                "--control-socket" => {
                    options.control_socket =
                        PathBuf::from(args.next().ok_or("--control-socket needs a path")?);
                }
                "--dim-after" => {
                    options.policy.dim_after =
                        parse_timeout(&args.next().ok_or("--dim-after needs a value")?)?;
                }
                "--lock-after" => {
                    options.policy.lock_after =
                        parse_timeout(&args.next().ok_or("--lock-after needs a value")?)?;
                }
                "--display-off-after" => {
                    options.policy.display_off_after =
                        parse_timeout(&args.next().ok_or("--display-off-after needs a value")?)?;
                }
                "--suspend-after" => {
                    options.policy.suspend_after =
                        parse_timeout(&args.next().ok_or("--suspend-after needs a value")?)?;
                }
                "--dim-percent" => {
                    options.policy.dim_percent =
                        args.next().ok_or("--dim-percent needs a value")?.parse()?;
                }
                "--help" | "-h" => {
                    println!(
                        "Usage: aegis-idle [OPTIONS]\n\
                         \n  --lock-now                 Lock through the running daemon\
                         \n  --socket PATH              Aegis IPC socket for output power\
                         \n  --control-socket PATH      Idle coordinator control socket\
                         \n  --dim-after SECONDS|off\
                         \n  --lock-after SECONDS|off\
                         \n  --display-off-after SECONDS|off\
                         \n  --suspend-after SECONDS|off\
                         \n  --dim-percent 1..100\
                         \n  --no-logind                Disable host sleep integration"
                    );
                    std::process::exit(0);
                }
                "--version" | "-V" => {
                    println!("aegis-idle {}", env!("CARGO_PKG_VERSION"));
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown argument: {argument}").into()),
            }
        }
        Ok(options)
    }
}

fn parse_timeout(value: &str) -> Result<Option<Duration>, Box<dyn std::error::Error>> {
    if value.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    let seconds: u64 = value.parse()?;
    if seconds == 0 {
        return Err("idle timeout must be positive or 'off'".into());
    }
    Ok(Some(Duration::from_secs(seconds)))
}

fn desired_output_power(lock_confirmed: bool, display_off_pending: bool) -> OutputPower {
    if lock_confirmed && display_off_pending {
        OutputPower::Off
    } else {
        OutputPower::On
    }
}

fn power_retry_blocked(retry: Option<PowerRetry>, target: OutputPower, now: Instant) -> bool {
    retry.is_some_and(|retry| retry.target == target && now < retry.not_before)
}

fn next_power_retry(previous: Option<PowerRetry>, target: OutputPower, now: Instant) -> PowerRetry {
    let delay = previous
        .filter(|retry| retry.target == target)
        .map(|retry| retry.delay.saturating_mul(2).min(POWER_RETRY_MAX))
        .unwrap_or(POWER_RETRY_INITIAL);
    PowerRetry {
        target,
        not_before: now + delay,
        delay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_socket(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aegis-idle-{name}-{}-{}.sock",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ))
    }

    #[test]
    fn active_control_socket_is_never_replaced() {
        let path = test_socket("active");
        let _ = std::fs::remove_file(&path);
        let first = bind_control_socket(&path).unwrap();
        let error = bind_control_socket(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        drop(first);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stale_control_socket_is_recovered() {
        let path = test_socket("stale");
        let _ = std::fs::remove_file(&path);
        drop(bind_control_socket(&path).unwrap());
        let replacement = bind_control_socket(&path).unwrap();
        drop(replacement);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn socket_guard_does_not_unlink_a_replacement_inode() {
        let path = test_socket("guard");
        let _ = std::fs::remove_file(&path);
        let original = bind_control_socket(&path).unwrap();
        let guard = SocketGuard::new(path.clone()).unwrap();
        std::fs::remove_file(&path).unwrap();
        let replacement = bind_control_socket(&path).unwrap();
        drop(guard);
        assert!(path.exists());
        drop(original);
        drop(replacement);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn output_power_waits_for_the_confirmed_lock_boundary() {
        assert_eq!(desired_output_power(false, false), OutputPower::On);
        assert_eq!(desired_output_power(false, true), OutputPower::On);
        assert_eq!(desired_output_power(true, false), OutputPower::On);
        assert_eq!(desired_output_power(true, true), OutputPower::Off);
    }

    #[test]
    fn opposite_power_target_bypasses_the_previous_retry_backoff() {
        let now = Instant::now();
        let retry = Some(PowerRetry {
            target: OutputPower::Off,
            not_before: now + Duration::from_secs(5),
            delay: Duration::from_secs(5),
        });
        assert!(power_retry_blocked(retry, OutputPower::Off, now));
        assert!(!power_retry_blocked(retry, OutputPower::On, now));
    }

    #[test]
    fn repeated_power_failures_back_off_without_delaying_the_first_retry() {
        let now = Instant::now();
        let first = next_power_retry(None, OutputPower::Off, now);
        assert_eq!(first.delay, POWER_RETRY_INITIAL);
        let second = next_power_retry(Some(first), OutputPower::Off, first.not_before);
        assert_eq!(second.delay, POWER_RETRY_INITIAL * 2);

        let mut retry = second;
        for _ in 0..10 {
            retry = next_power_retry(Some(retry), OutputPower::Off, retry.not_before);
        }
        assert_eq!(retry.delay, POWER_RETRY_MAX);
    }

    #[test]
    fn lock_confirmation_pipe_is_consumed_exactly_once() {
        let mut descriptors = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_NONBLOCK) },
            0
        );
        let read = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
        let write = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
        assert_eq!(
            unsafe { libc::write(write.as_raw_fd(), std::ptr::from_ref(&b'\n').cast(), 1,) },
            1
        );
        drop(write);

        let mut ready = Some(read);
        assert!(consume_lock_ready(&mut ready).unwrap());
        assert!(ready.is_none());
        assert!(!consume_lock_ready(&mut ready).unwrap());
    }

    #[test]
    fn closed_unconfirmed_pipe_cannot_leave_a_poll_hup_source() {
        let mut descriptors = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_NONBLOCK) },
            0
        );
        let read = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
        drop(unsafe { OwnedFd::from_raw_fd(descriptors[1]) });

        let mut ready = Some(read);
        assert!(!consume_lock_ready(&mut ready).unwrap());
        assert!(ready.is_none());
    }
}
