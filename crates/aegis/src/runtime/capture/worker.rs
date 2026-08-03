use super::encoding::{CapturedPixels, PendingReadback, encode_capture};
use crate::runtime::commands::journal_effect_and_broadcast;
use crate::runtime::interaction_domain::InteractionDomainCaptureContext;

/// Pollable completion wakeup shared by the capture worker and the compositor
/// event loop. The backend currently accepts one auxiliary wakeup fd, so an
/// epoll instance multiplexes the Wayland server fd with this worker's
/// eventfd. The worker sends the completion through the channel *before*
/// signalling, making the fd a pure readiness notification rather than a
/// second data queue.
struct CaptureWakeMux {
    epoll: std::os::fd::OwnedFd,
    completion: std::sync::Arc<std::os::fd::OwnedFd>,
}

impl CaptureWakeMux {
    fn new() -> std::io::Result<Self> {
        use std::os::fd::FromRawFd;

        let epoll = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        if epoll < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let epoll = unsafe { std::os::fd::OwnedFd::from_raw_fd(epoll) };
        let completion = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if completion < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let completion =
            std::sync::Arc::new(unsafe { std::os::fd::OwnedFd::from_raw_fd(completion) });
        add_epoll_source(
            std::os::fd::AsRawFd::as_raw_fd(&epoll),
            std::os::fd::AsRawFd::as_raw_fd(completion.as_ref()),
            1,
        )?;
        Ok(Self { epoll, completion })
    }

    fn register_source(&self, fd: std::os::fd::RawFd) -> std::io::Result<()> {
        add_epoll_source(std::os::fd::AsRawFd::as_raw_fd(&self.epoll), fd, 2)
    }

    fn fd(&self) -> std::os::fd::RawFd {
        std::os::fd::AsRawFd::as_raw_fd(&self.epoll)
    }

    /// Drain underlying completion readiness first and queued epoll events
    /// second. Call this after dispatching the Wayland server but before
    /// draining the completion channel. That ordering cannot lose a worker
    /// completion: a completion racing after this drain either appears in the
    /// channel immediately or leaves the eventfd readable for the next wait.
    fn drain(&self) {
        let completion = std::os::fd::AsRawFd::as_raw_fd(self.completion.as_ref());
        loop {
            let mut value = 0u64;
            let read = unsafe {
                libc::read(
                    completion,
                    (&mut value as *mut u64).cast(),
                    std::mem::size_of::<u64>(),
                )
            };
            if read == std::mem::size_of::<u64>() as isize {
                continue;
            }
            if read < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
                continue;
            }
            break;
        }

        let mut events = [libc::epoll_event { events: 0, u64: 0 }; 8];
        loop {
            let ready =
                unsafe { libc::epoll_wait(self.fd(), events.as_mut_ptr(), events.len() as i32, 0) };
            if ready <= 0 || ready < events.len() as i32 {
                break;
            }
        }
    }
}

fn add_epoll_source(
    epoll: std::os::fd::RawFd,
    source: std::os::fd::RawFd,
    tag: u64,
) -> std::io::Result<()> {
    let mut event = libc::epoll_event {
        // Level-triggered readiness intentionally matches the backend's old
        // direct poll contract. If server dispatch leaves protocol work, the
        // mux remains readable and the loop immediately returns again.
        events: libc::EPOLLIN as u32,
        u64: tag,
    };
    let result = unsafe { libc::epoll_ctl(epoll, libc::EPOLL_CTL_ADD, source, &mut event) };
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn signal_completion(fd: &std::os::fd::OwnedFd) {
    let value = 1u64;
    let written = unsafe {
        libc::write(
            std::os::fd::AsRawFd::as_raw_fd(fd),
            (&value as *const u64).cast(),
            std::mem::size_of::<u64>(),
        )
    };
    if written != std::mem::size_of::<u64>() as isize {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock {
            log::warn!("capture completion wakeup failed: {error}");
        }
    }
}

fn send_completion(
    completions: &std::sync::mpsc::Sender<CaptureCompletion>,
    wake: &std::os::fd::OwnedFd,
    completion: CaptureCompletion,
) -> bool {
    if completions.send(completion).is_err() {
        return false;
    }
    signal_completion(wake);
    true
}

pub(in crate::runtime) enum CaptureTarget {
    Screenshot {
        path: String,
        command: aegis_ipc::Command,
        ts_mono_ms: u64,
        origin: aegis_ipc::Origin,
    },
    Reply {
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureOutputPayload, String>>,
    },
    InteractionDomainReply {
        context: InteractionDomainCaptureContext,
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureInteractionDomainPayload, String>>,
    },
    /// One user-picked pixel readback (ADR-0054). The main loop answers the
    /// waiting `PickTarget` IPC request with the colour.
    Pixel {
        point: aegis_core::Point,
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::PickResult, String>>,
    },
    /// One shared readback for every stream that was due this presentation
    /// (ADR-0052). The worker converts to opaque BGRA and the main loop fans
    /// the result out over the IPC.
    Stream,
}

pub(in crate::runtime) struct PendingCapture {
    pub(in crate::runtime) readback: PendingReadback,
    pub(in crate::runtime) target: CaptureTarget,
}

enum CaptureJob {
    Screenshot {
        capture: CapturedPixels,
        path: String,
        command: aegis_ipc::Command,
        ts_mono_ms: u64,
        origin: aegis_ipc::Origin,
    },
    Reply {
        capture: CapturedPixels,
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureOutputPayload, String>>,
    },
    InteractionDomainReply {
        capture: CapturedPixels,
        context: InteractionDomainCaptureContext,
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureInteractionDomainPayload, String>>,
    },
    Pixel {
        capture: CapturedPixels,
        point: aegis_core::Point,
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::PickResult, String>>,
    },
    Stream {
        capture: CapturedPixels,
    },
}

pub(in crate::runtime) enum CaptureCompletion {
    /// PNG encoding finished. This is deliberately delivered before the
    /// potentially slow atomic file write so the main loop can publish the
    /// image clipboard immediately.
    ScreenshotEncoded {
        path: String,
        origin: aegis_ipc::Origin,
        security_generation: u64,
        encoded: Result<std::sync::Arc<[u8]>, String>,
    },
    /// Atomic file write + fsync + rename finished. The screenshot command's
    /// journal effect is decided here, preserving the existing contract that
    /// `Applied` means the destination file was durably committed.
    ScreenshotSaved {
        path: String,
        command: aegis_ipc::Command,
        ts_mono_ms: u64,
        origin: aegis_ipc::Origin,
        security_generation: u64,
        /// Shared with the already-published clipboard payload without a
        /// second multi-megabyte copy. Absent when encoding failed.
        png: Option<std::sync::Arc<[u8]>>,
        written: Result<(), String>,
    },
    Reply {
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureOutputPayload, String>>,
        security_generation: u64,
        encoded: Result<aegis_ipc::CaptureOutputPayload, String>,
    },
    InteractionDomainReply {
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureInteractionDomainPayload, String>>,
        security_generation: u64,
        observation_token: aegis_ipc::ObservationToken,
        encoded: Result<aegis_ipc::CaptureInteractionDomainPayload, String>,
    },
    Pixel {
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::PickResult, String>>,
        security_generation: u64,
        picked: Result<aegis_ipc::PickResult, String>,
    },
    /// Raw opaque-BGRA pixels for stream fan-out. No reply channel: the main
    /// loop owns delivery and drop accounting.
    Stream {
        security_generation: u64,
        pixels: Result<StreamPixels, String>,
    },
}

impl CaptureCompletion {
    /// Whether this completion ends the worker lane reservation. The encoded
    /// screenshot phase intentionally keeps the lane reserved until its file
    /// commit finishes, bounding retained PNG/readback memory to one capture.
    pub(in crate::runtime) fn finishes_reserved_job(&self) -> bool {
        !matches!(self, Self::ScreenshotEncoded { .. } | Self::Stream { .. })
    }
}

/// One converted stream frame: tightly packed opaque BGRA (alpha 255),
/// `width * 4` bytes per row (ADR-0052).
pub(in crate::runtime) struct StreamPixels {
    pub(in crate::runtime) width: u32,
    pub(in crate::runtime) height: u32,
    pub(in crate::runtime) bgra: std::sync::Arc<[u8]>,
    pub(in crate::runtime) damage: Vec<aegis_core::Rect>,
}

/// Single bounded post-processing lane for screenshots and IPC pixel
/// captures. Only one full-frame payload may be in flight, which keeps
/// repeated requests from consuming unbounded memory or compounding stalls.
pub(in crate::runtime) struct CaptureWorker {
    jobs: std::sync::mpsc::Sender<CaptureJob>,
    pub(in crate::runtime) completions: std::sync::mpsc::Receiver<CaptureCompletion>,
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    allowed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    security_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    wake: CaptureWakeMux,
}

impl CaptureWorker {
    pub(in crate::runtime) fn spawn() -> std::io::Result<Self> {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<CaptureJob>();
        let (completion_tx, completion_rx) = std::sync::mpsc::channel::<CaptureCompletion>();
        let wake = CaptureWakeMux::new()?;
        let worker_wake = std::sync::Arc::clone(&wake.completion);
        let busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_busy = std::sync::Arc::clone(&busy);
        let allowed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let worker_allowed = std::sync::Arc::clone(&allowed);
        let security_generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        let worker_security_generation = std::sync::Arc::clone(&security_generation);
        std::thread::Builder::new()
            .name("aegis-capture".into())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    match job {
                        CaptureJob::Screenshot {
                            capture,
                            path,
                            command,
                            ts_mono_ms,
                            origin,
                        } => {
                            let generation = capture.security_generation;
                            let permitted = || {
                                worker_allowed.load(std::sync::atomic::Ordering::Acquire)
                                    && generation
                                        == worker_security_generation
                                            .load(std::sync::atomic::Ordering::Acquire)
                            };
                            let encoded: Result<std::sync::Arc<[u8]>, String> = if permitted() {
                                encode_capture(capture).map(|(_, _, png)| std::sync::Arc::from(png))
                            } else {
                                Err("session locked before capture completed".into())
                            };
                            // Publish the encoded phase before touching disk:
                            // clipboard availability is now bounded by GPU
                            // readback + PNG encoding, never by fsync latency.
                            if !send_completion(
                                &completion_tx,
                                worker_wake.as_ref(),
                                CaptureCompletion::ScreenshotEncoded {
                                    path: path.clone(),
                                    origin: origin.clone(),
                                    security_generation: generation,
                                    encoded: encoded.clone(),
                                },
                            ) {
                                worker_busy.store(false, std::sync::atomic::Ordering::Release);
                                break;
                            }
                            // Commit the same Arc on the worker after the
                            // early delivery. Authority is re-checked before
                            // creating the file, so a lock during encoding
                            // still suppresses persistent output.
                            let written = encoded.as_ref().map_err(Clone::clone).and_then(|png| {
                                if permitted() {
                                    super::output::atomic_write_capture(&path, png)
                                } else {
                                    Err("session locked before capture completed".into())
                                }
                            });
                            let png = encoded.ok();
                            if !send_completion(
                                &completion_tx,
                                worker_wake.as_ref(),
                                CaptureCompletion::ScreenshotSaved {
                                    path,
                                    command,
                                    ts_mono_ms,
                                    origin,
                                    security_generation: generation,
                                    png,
                                    written,
                                },
                            ) {
                                worker_busy.store(false, std::sync::atomic::Ordering::Release);
                                break;
                            }
                            // The main loop clears `busy` after it records the
                            // saved phase. Readiness is carried by eventfd,
                            // not by rendering artificial animation frames.
                        }
                        CaptureJob::Reply { capture, reply } => {
                            let generation = capture.security_generation;
                            let encoded = if worker_allowed
                                .load(std::sync::atomic::Ordering::Acquire)
                                && generation
                                    == worker_security_generation
                                        .load(std::sync::atomic::Ordering::Acquire)
                            {
                                encode_capture(capture).map(|(width, height, png)| {
                                    aegis_ipc::CaptureOutputPayload { width, height, png }
                                })
                            } else {
                                Err("session locked before capture completed".into())
                            };
                            if !send_completion(
                                &completion_tx,
                                worker_wake.as_ref(),
                                CaptureCompletion::Reply {
                                    reply,
                                    security_generation: generation,
                                    encoded,
                                },
                            ) {
                                worker_busy.store(false, std::sync::atomic::Ordering::Release);
                                break;
                            }
                        }
                        CaptureJob::InteractionDomainReply {
                            capture,
                            context,
                            reply,
                        } => {
                            let generation = capture.security_generation;
                            let observation_token = context
                                .observation
                                .as_ref()
                                .expect(
                                    "Interaction Domain capture must issue its observation lease before encoding",
                                )
                                .token
                                .clone();
                            let encoded = if worker_allowed
                                .load(std::sync::atomic::Ordering::Acquire)
                                && generation
                                    == worker_security_generation
                                        .load(std::sync::atomic::Ordering::Acquire)
                            {
                                encode_capture(capture).map(|(width, height, png)| {
                                    aegis_ipc::CaptureInteractionDomainPayload {
                                        capture: aegis_ipc::InteractionDomainCapture {
                                            interaction_domain: context.interaction_domain,
                                            width,
                                            height,
                                            scale_milli: context.scale_milli,
                                            region: context.region,
                                            placements: context.placements,
                                            observation: context.observation.expect(
                                                "Interaction Domain capture must issue its observation lease before encoding",
                                            ),
                                            png_bytes: png.len() as u64,
                                            revision: context.revision,
                                        },
                                        png,
                                    }
                                })
                            } else {
                                Err("session locked before Interaction Domain capture completed".into())
                            };
                            if !send_completion(
                                &completion_tx,
                                worker_wake.as_ref(),
                                CaptureCompletion::InteractionDomainReply {
                                    reply,
                                    security_generation: generation,
                                    observation_token,
                                    encoded,
                                },
                            ) {
                                worker_busy.store(false, std::sync::atomic::Ordering::Release);
                                break;
                            }
                        }
                        CaptureJob::Pixel {
                            capture,
                            point,
                            reply,
                        } => {
                            let generation = capture.security_generation;
                            let picked = if worker_allowed
                                .load(std::sync::atomic::Ordering::Acquire)
                                && generation
                                    == worker_security_generation
                                        .load(std::sync::atomic::Ordering::Acquire)
                            {
                                super::encoding::read_picked_pixel(capture)
                                    .map(|rgb| aegis_ipc::PickResult::Pixel { point, rgb })
                            } else {
                                Err("session locked before pixel pick completed".into())
                            };
                            if !send_completion(
                                &completion_tx,
                                worker_wake.as_ref(),
                                CaptureCompletion::Pixel {
                                    reply,
                                    security_generation: generation,
                                    picked,
                                },
                            ) {
                                worker_busy.store(false, std::sync::atomic::Ordering::Release);
                                break;
                            }
                        }
                        CaptureJob::Stream { capture } => {
                            let generation = capture.security_generation;
                            let pixels = if worker_allowed
                                .load(std::sync::atomic::Ordering::Acquire)
                                && generation
                                    == worker_security_generation
                                        .load(std::sync::atomic::Ordering::Acquire)
                            {
                                super::encoding::stream_pixels(capture)
                            } else {
                                Err("session locked before stream frame completed".into())
                            };
                            if !send_completion(
                                &completion_tx,
                                worker_wake.as_ref(),
                                CaptureCompletion::Stream {
                                    security_generation: generation,
                                    pixels,
                                },
                            ) {
                                worker_busy.store(false, std::sync::atomic::Ordering::Release);
                                break;
                            }
                        }
                    }
                }
                worker_busy.store(false, std::sync::atomic::Ordering::Release);
            })?;
        Ok(Self {
            jobs: job_tx,
            completions: completion_rx,
            busy,
            allowed,
            security_generation,
            wake,
        })
    }

    /// Add the Wayland server event-loop fd to the completion wake mux. The
    /// host backend polls [`wakeup_fd`](Self::wakeup_fd), preserving client
    /// commit wakeups while also reacting immediately to worker completions.
    pub(in crate::runtime) fn register_server_wakeup_fd(
        &self,
        fd: std::os::fd::RawFd,
    ) -> std::io::Result<()> {
        self.wake.register_source(fd)
    }

    pub(in crate::runtime) fn wakeup_fd(&self) -> std::os::fd::RawFd {
        self.wake.fd()
    }

    pub(in crate::runtime) fn drain_wakeup(&self) {
        self.wake.drain();
    }

    pub(in crate::runtime) fn reserve(&self) -> bool {
        self.busy
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
    }

    pub(in crate::runtime) fn release(&self) {
        self.busy.store(false, std::sync::atomic::Ordering::Release);
    }

    pub(in crate::runtime) fn is_busy(&self) -> bool {
        self.busy.load(std::sync::atomic::Ordering::Acquire)
    }

    pub(in crate::runtime) fn delivery_gate(
        &self,
    ) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        std::sync::Arc::clone(&self.allowed)
    }

    pub(in crate::runtime) fn set_allowed(&self, allowed: bool) {
        let was_allowed = self
            .allowed
            .swap(allowed, std::sync::atomic::Ordering::AcqRel);
        if was_allowed && !allowed {
            self.security_generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }

    pub(in crate::runtime) fn security_generation(&self) -> u64 {
        self.security_generation
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub(in crate::runtime) fn permits(&self, security_generation: u64) -> bool {
        self.allowed.load(std::sync::atomic::Ordering::Acquire)
            && self.security_generation() == security_generation
    }

    pub(in crate::runtime) fn invalidate_security_context(&self) {
        self.security_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }

    fn submit(&self, job: CaptureJob) -> Result<(), Box<CaptureJob>> {
        self.jobs.send(job).map_err(|error| Box::new(error.0))
    }
}

pub(in crate::runtime) fn refuse_capture_target(
    worker: &CaptureWorker,
    target: CaptureTarget,
    reason: String,
    journal: &std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
    ipc: &Option<aegis_ipc::Server>,
) {
    // Stream frames never reserve the worker lane (they skip busy frames
    // instead), so releasing it here must not clear another target's
    // reservation.
    if !matches!(target, CaptureTarget::Stream) {
        worker.release();
    }
    match target {
        CaptureTarget::Screenshot {
            command,
            ts_mono_ms,
            origin,
            ..
        } => journal_effect_and_broadcast(
            journal,
            ipc,
            ts_mono_ms,
            origin,
            command,
            aegis_ipc::Effect::Refused { reason },
        ),
        CaptureTarget::Reply { reply } => {
            let _ = reply.send(Err(reason));
        }
        CaptureTarget::InteractionDomainReply { reply, .. } => {
            let _ = reply.send(Err(reason));
        }
        CaptureTarget::Pixel { reply, .. } => {
            let _ = reply.send(Err(reason));
        }
        // A stream frame carries no reply channel: the main loop simply
        // skips one frame and the stream resumes at the next presentation.
        CaptureTarget::Stream => {}
    }
}

pub(in crate::runtime) fn queue_captured_pixels(
    worker: &CaptureWorker,
    capture: CapturedPixels,
    target: CaptureTarget,
    journal: &std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
    ipc: &Option<aegis_ipc::Server>,
) {
    let job = match target {
        CaptureTarget::Screenshot {
            path,
            command,
            ts_mono_ms,
            origin,
        } => CaptureJob::Screenshot {
            capture,
            path,
            command,
            ts_mono_ms,
            origin,
        },
        CaptureTarget::Reply { reply } => CaptureJob::Reply { capture, reply },
        CaptureTarget::InteractionDomainReply { context, reply } => {
            CaptureJob::InteractionDomainReply {
                capture,
                context,
                reply,
            }
        }
        CaptureTarget::Pixel { point, reply } => CaptureJob::Pixel {
            capture,
            point,
            reply,
        },
        CaptureTarget::Stream => CaptureJob::Stream { capture },
    };
    if let Err(job) = worker.submit(job) {
        let target = match *job {
            CaptureJob::Screenshot {
                path,
                command,
                ts_mono_ms,
                origin,
                ..
            } => CaptureTarget::Screenshot {
                path,
                command,
                ts_mono_ms,
                origin,
            },
            CaptureJob::Reply { reply, .. } => CaptureTarget::Reply { reply },
            CaptureJob::InteractionDomainReply { context, reply, .. } => {
                CaptureTarget::InteractionDomainReply { context, reply }
            }
            CaptureJob::Pixel { point, reply, .. } => CaptureTarget::Pixel { point, reply },
            CaptureJob::Stream { .. } => CaptureTarget::Stream,
        };
        refuse_capture_target(
            worker,
            target,
            "capture worker stopped".to_owned(),
            journal,
            ipc,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poll_readable(fd: std::os::fd::RawFd) -> bool {
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut pollfd, 1, 0) };
        ready == 1 && pollfd.revents & libc::POLLIN != 0
    }

    #[test]
    fn completion_eventfd_wakes_and_drains_the_mux() {
        let wake = CaptureWakeMux::new().unwrap();
        assert!(!poll_readable(wake.fd()));

        signal_completion(wake.completion.as_ref());
        assert!(poll_readable(wake.fd()));

        wake.drain();
        assert!(!poll_readable(wake.fd()));
    }

    #[test]
    fn mux_preserves_wayland_style_external_wakeups() {
        use std::os::fd::{AsRawFd, FromRawFd};

        let wake = CaptureWakeMux::new().unwrap();
        let nested = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        assert!(nested >= 0);
        let nested = unsafe { std::os::fd::OwnedFd::from_raw_fd(nested) };
        let external = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        assert!(external >= 0);
        let external = unsafe { std::os::fd::OwnedFd::from_raw_fd(external) };
        add_epoll_source(nested.as_raw_fd(), external.as_raw_fd(), 9).unwrap();
        // libwayland's event-loop fd is itself epoll-backed. Verify that the
        // completion mux supports the same nested-epoll shape.
        wake.register_source(nested.as_raw_fd()).unwrap();

        signal_completion(&external);
        assert!(poll_readable(wake.fd()));

        // Draining only the outer mux cannot consume readiness owned by the
        // nested source. Level triggering must leave the mux observable until
        // the Wayland-style source itself is dispatched.
        wake.drain();
        assert!(poll_readable(wake.fd()));

        // The real iteration dispatches/drains the Wayland source first.
        let mut nested_event = libc::epoll_event { events: 0, u64: 0 };
        assert_eq!(
            unsafe { libc::epoll_wait(nested.as_raw_fd(), &mut nested_event, 1, 0) },
            1
        );
        let mut value = 0u64;
        assert_eq!(
            unsafe {
                libc::read(
                    external.as_raw_fd(),
                    (&mut value as *mut u64).cast(),
                    std::mem::size_of::<u64>(),
                )
            },
            std::mem::size_of::<u64>() as isize
        );
        wake.drain();
        assert!(!poll_readable(wake.fd()));
    }

    #[test]
    fn encoded_phase_keeps_the_bounded_lane_reserved() {
        let encoded = CaptureCompletion::ScreenshotEncoded {
            path: "/tmp/shot.png".into(),
            origin: aegis_ipc::Origin::Keybinding,
            security_generation: 1,
            encoded: Err("test".into()),
        };
        let (reply, _receiver) = std::sync::mpsc::channel();
        let replied = CaptureCompletion::Reply {
            reply,
            security_generation: 1,
            encoded: Err("test".into()),
        };
        let streamed = CaptureCompletion::Stream {
            security_generation: 1,
            pixels: Err("test".into()),
        };

        assert!(!encoded.finishes_reserved_job());
        assert!(replied.finishes_reserved_job());
        assert!(!streamed.finishes_reserved_job());
    }
}
