use super::*;

pub(super) struct FrameCapture {
    pub(super) crop: Option<aegis_model::Rect>,
    pub(super) target: CaptureTarget,
    /// Cursor state sampled when a saved screenshot was requested. Output
    /// capture, streams, and picker readbacks deliberately leave this empty.
    pub(super) cursor: Option<CaptureCursorState>,
}

impl CompositorRuntime {
    /// Drain one-shot and stream capture requests and bind at most one
    /// readback to the upcoming presentation frame. Runs before the
    /// render/skip decision so a bound capture always forces presentation.
    /// The readback copy is recorded after every scene and cursor draw, so it
    /// captures exactly the pixels submitted rather than a later re-render of
    /// mutable state.
    pub(super) fn prepare_frame_capture(
        &mut self,
        session_locked: bool,
        pending_screenshots: &mut Vec<PendingScreenshot>,
    ) -> Option<FrameCapture> {
        let mut frame_capture = None;
        for req in self.capture_rx.try_iter() {
            if session_locked || !self.host.is_active() {
                let _ = req
                    .reply
                    .send(Err("session is locked or inactive".to_owned()));
            } else if !self.capture_worker.reserve() {
                let _ = req
                    .reply
                    .send(Err("another capture is still being processed".to_owned()));
            } else {
                frame_capture = Some(FrameCapture {
                    crop: req.region,
                    target: CaptureTarget::Reply { reply: req.reply },
                    cursor: None,
                });
            }
        }
        for request in pending_screenshots.drain(..) {
            let PendingScreenshot {
                command: cmd,
                ts_mono_ms: ts,
                origin,
                cursor,
            } = request;
            let aegis_ipc::Command::Screenshot { path, region } = &cmd else {
                continue;
            };
            if session_locked || !self.host.is_active() {
                journal_effect_and_broadcast(
                    &self.journal,
                    &self.ipc,
                    ts,
                    origin,
                    cmd,
                    aegis_ipc::Effect::Refused {
                        reason: "session is locked or inactive".into(),
                    },
                );
            } else if !self.capture_worker.reserve() {
                journal_effect_and_broadcast(
                    &self.journal,
                    &self.ipc,
                    ts,
                    origin,
                    cmd,
                    aegis_ipc::Effect::Refused {
                        reason: "another capture is still being processed".into(),
                    },
                );
            } else {
                frame_capture = Some(FrameCapture {
                    crop: *region,
                    target: CaptureTarget::Screenshot {
                        path: path.clone(),
                        command: cmd,
                        ts_mono_ms: ts,
                        origin,
                    },
                    cursor: Some(cursor),
                });
            }
        }
        // Stream fan-out (ADR-0052): when no one-shot capture claimed this
        // frame's readback and the staging slot and worker lane are free,
        // bind one readback shared by every due stream. One-shots keep
        // priority; a locked or inactive session simply produces no stream
        // frames (the stream survives).
        if frame_capture.is_none()
            && self.pending_capture.is_none()
            && !self.stream_job_in_flight
            && !session_locked
            && self.host.is_active()
            && !self.capture_worker.is_busy()
            && !self.streams.due_ids(std::time::Instant::now()).is_empty()
        {
            frame_capture = Some(FrameCapture {
                crop: None,
                target: CaptureTarget::Stream,
                cursor: None,
            });
        }
        frame_capture
    }
}
