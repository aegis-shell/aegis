use super::encoding::{CapturedPixels, PendingReadback, encode_capture};
use crate::runtime::commands::journal_effect_and_broadcast;
use crate::runtime::realm::RealmCaptureContext;

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
    RealmReply {
        context: RealmCaptureContext,
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureRealmPayload, String>>,
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
    RealmReply {
        capture: CapturedPixels,
        context: RealmCaptureContext,
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureRealmPayload, String>>,
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
    Screenshot {
        path: String,
        command: aegis_ipc::Command,
        ts_mono_ms: u64,
        origin: aegis_ipc::Origin,
        security_generation: u64,
        encoded: Result<Vec<u8>, String>,
        /// Outcome of the atomic write + rename, already committed by the
        /// worker so the frame loop never blocks on the multi-MB write/fsync.
        /// Mirrors `encoded`'s error when encoding itself failed.
        written: Result<(), String>,
    },
    Reply {
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureOutputPayload, String>>,
        security_generation: u64,
        encoded: Result<aegis_ipc::CaptureOutputPayload, String>,
    },
    RealmReply {
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureRealmPayload, String>>,
        security_generation: u64,
        encoded: Result<aegis_ipc::CaptureRealmPayload, String>,
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
}

impl CaptureWorker {
    pub(in crate::runtime) fn spawn() -> std::io::Result<Self> {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<CaptureJob>();
        let (completion_tx, completion_rx) = std::sync::mpsc::channel::<CaptureCompletion>();
        let busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_busy = std::sync::Arc::clone(&busy);
        let allowed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let worker_allowed = std::sync::Arc::clone(&allowed);
        let security_generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        let worker_security_generation = std::sync::Arc::clone(&security_generation);
        std::thread::Builder::new()
            .name("ass-capture".into())
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
                            let encoded = if permitted() {
                                encode_capture(capture).map(|(_, _, png)| png)
                            } else {
                                Err("session locked before capture completed".into())
                            };
                            // Commit the PNG on this worker as well: the
                            // write + fsync + rename of a multi-MB file must
                            // not stall the frame loop. Delivery authority is
                            // re-checked right before the commit so a session
                            // lock during encoding still suppresses the
                            // on-disk capture.
                            let written = encoded.as_ref().map_err(Clone::clone).and_then(|png| {
                                if permitted() {
                                    super::output::atomic_write_capture(&path, png)
                                } else {
                                    Err("session locked before capture completed".into())
                                }
                            });
                            if completion_tx
                                .send(CaptureCompletion::Screenshot {
                                    path,
                                    command,
                                    ts_mono_ms,
                                    origin,
                                    security_generation: generation,
                                    encoded,
                                    written,
                                })
                                .is_err()
                            {
                                worker_busy.store(false, std::sync::atomic::Ordering::Release);
                                break;
                            }
                            // The main loop clears `busy` after it records the
                            // completion, keeping the loop awake until then.
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
                            if completion_tx
                                .send(CaptureCompletion::Reply {
                                    reply,
                                    security_generation: generation,
                                    encoded,
                                })
                                .is_err()
                            {
                                worker_busy.store(false, std::sync::atomic::Ordering::Release);
                                break;
                            }
                        }
                        CaptureJob::RealmReply {
                            capture,
                            context,
                            reply,
                        } => {
                            let generation = capture.security_generation;
                            let encoded = if worker_allowed
                                .load(std::sync::atomic::Ordering::Acquire)
                                && generation
                                    == worker_security_generation
                                        .load(std::sync::atomic::Ordering::Acquire)
                            {
                                encode_capture(capture).map(|(width, height, png)| {
                                    aegis_ipc::CaptureRealmPayload {
                                        capture: aegis_ipc::RealmCapture {
                                            realm: context.realm,
                                            width,
                                            height,
                                            scale_milli: context.scale_milli,
                                            region: context.region,
                                            placements: context.placements,
                                            png_bytes: png.len() as u64,
                                            revision: context.revision,
                                        },
                                        png,
                                    }
                                })
                            } else {
                                Err("session locked before Realm capture completed".into())
                            };
                            if completion_tx
                                .send(CaptureCompletion::RealmReply {
                                    reply,
                                    security_generation: generation,
                                    encoded,
                                })
                                .is_err()
                            {
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
                            if completion_tx
                                .send(CaptureCompletion::Pixel {
                                    reply,
                                    security_generation: generation,
                                    picked,
                                })
                                .is_err()
                            {
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
                            if completion_tx
                                .send(CaptureCompletion::Stream {
                                    security_generation: generation,
                                    pixels,
                                })
                                .is_err()
                            {
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
        })
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
        CaptureTarget::RealmReply { reply, .. } => {
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
        CaptureTarget::RealmReply { context, reply } => CaptureJob::RealmReply {
            capture,
            context,
            reply,
        },
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
            CaptureJob::RealmReply { context, reply, .. } => {
                CaptureTarget::RealmReply { context, reply }
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
