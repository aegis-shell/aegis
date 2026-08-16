use std::os::fd::AsRawFd;

use super::*;

pub(super) fn write_output_capture<H: Handler>(
    stream: &mut UnixStream,
    payload: CaptureOutputPayload,
    lease_deadline: std::time::Instant,
    handler: &H,
    scope: &LiveScopeBinding,
) -> io::Result<()> {
    match crate::blob::SealedBlob::new(&payload.png) {
        Ok(_blob) if std::time::Instant::now() >= lease_deadline => {
            audit_capability_effect(
                handler,
                scope,
                ActorCapability::CaptureOutput,
                crate::journal::CapabilityUseAction::Capture,
                crate::journal::Effect::Refused {
                    reason: "capture delivery refused".into(),
                },
            );
            write_msg(
                stream,
                &Response::Error {
                    message: "privileged capability lease expired before capture delivery".into(),
                },
            )
        }
        Ok(_blob)
            if !handler.capture_security_active()
                || !scope.resolve(handler).is_some_and(|scope| {
                    scope.ops.as_ref().is_some_and(|ops| {
                        ops.contains(&crate::schema::ActorCapability::CaptureOutput)
                    })
                }) =>
        {
            audit_capability_effect(
                handler,
                scope,
                ActorCapability::CaptureOutput,
                crate::journal::CapabilityUseAction::Capture,
                crate::journal::Effect::Refused {
                    reason: "capture delivery refused".into(),
                },
            );
            write_msg(
                stream,
                &Response::Error {
                    message: "capture authorization changed before final delivery".into(),
                },
            )
        }
        Ok(blob) => {
            audit_capability_effect(
                handler,
                scope,
                ActorCapability::CaptureOutput,
                crate::journal::CapabilityUseAction::Capture,
                crate::journal::Effect::Applied,
            );
            write_msg(
                stream,
                &Response::CaptureOutput {
                    width: payload.width,
                    height: payload.height,
                    png_bytes: blob.len(),
                },
            )?;
            blob.send(stream)
        }
        Err(error) => {
            audit_capability_effect(
                handler,
                scope,
                ActorCapability::CaptureOutput,
                crate::journal::CapabilityUseAction::Capture,
                crate::journal::Effect::Refused {
                    reason: "capture delivery refused".into(),
                },
            );
            write_msg(
                stream,
                &Response::Error {
                    message: format!("prepare output capture transfer: {error}"),
                },
            )
        }
    }
}

pub(super) fn write_interaction_domain_capture<H: Handler>(
    stream: &mut UnixStream,
    mut payload: CaptureInteractionDomainPayload,
    lease_deadline: std::time::Instant,
    handler: &H,
    scope: &LiveScopeBinding,
    via_grant: bool,
) -> io::Result<()> {
    match crate::blob::SealedBlob::new(&payload.png) {
        Ok(_blob) if std::time::Instant::now() >= lease_deadline => {
            audit_capability_effect(
                handler,
                scope,
                ActorCapability::CaptureInteractionDomain,
                crate::journal::CapabilityUseAction::Capture,
                crate::journal::Effect::Refused {
                    reason: "Interaction Domain capture delivery refused".into(),
                },
            );
            write_msg(
                stream,
                &Response::Error {
                    message:
                        "privileged capability lease expired before Interaction Domain capture delivery"
                            .into(),
                },
            )
        }
        Ok(blob) => {
            let snapshot = handler.interaction_domains();
            let capture_allowed = scope.resolve(handler).is_some_and(|scope| {
                if via_grant {
                    scope.asks(ActorCapability::CaptureInteractionDomain)
                        && scope.permits_interaction_domain_capture_target(
                            payload.capture.interaction_domain,
                        )
                } else {
                    scope.permits_interaction_domain_capture(payload.capture.interaction_domain)
                }
            });
            let authorized = handler.capture_security_active()
                && capture_allowed
                && snapshot.revision == payload.capture.revision
                && snapshot
                    .interaction_domains
                    .iter()
                    .any(|interaction_domain| {
                        interaction_domain.id == payload.capture.interaction_domain
                            && interaction_domain.state
                                == aegis_model::interaction_domain::InteractionDomainState::Active
                    });
            if !authorized {
                audit_capability_effect(
                    handler,
                    scope,
                    ActorCapability::CaptureInteractionDomain,
                    crate::journal::CapabilityUseAction::Capture,
                    crate::journal::Effect::Refused {
                        reason: "Interaction Domain capture delivery refused".into(),
                    },
                );
                return write_msg(
                    stream,
                    &Response::Error {
                        message:
                            "Interaction Domain capture authorization changed before final delivery"
                                .into(),
                    },
                );
            }
            payload.capture.png_bytes = blob.len();
            audit_capability_effect(
                handler,
                scope,
                ActorCapability::CaptureInteractionDomain,
                crate::journal::CapabilityUseAction::Capture,
                crate::journal::Effect::Applied,
            );
            write_msg(
                stream,
                &Response::CaptureInteractionDomain {
                    capture: payload.capture,
                },
            )?;
            blob.send(stream)
        }
        Err(error) => {
            audit_capability_effect(
                handler,
                scope,
                ActorCapability::CaptureInteractionDomain,
                crate::journal::CapabilityUseAction::Capture,
                crate::journal::Effect::Refused {
                    reason: "Interaction Domain capture delivery refused".into(),
                },
            );
            write_msg(
                stream,
                &Response::Error {
                    message: format!("prepare Interaction Domain capture transfer: {error}"),
                },
            )
        }
    }
}

pub(super) fn write_window_capture<H: Handler>(
    stream: &mut UnixStream,
    mut payload: CaptureWindowPayload,
    lease_deadline: std::time::Instant,
    handler: &H,
    scope: &LiveScopeBinding,
    via_grant: bool,
) -> io::Result<()> {
    match crate::blob::SealedBlob::new(&payload.png) {
        Ok(_blob) if std::time::Instant::now() >= lease_deadline => {
            audit_capability_effect(
                handler,
                scope,
                ActorCapability::CaptureWindow,
                crate::journal::CapabilityUseAction::Capture,
                crate::journal::Effect::Refused {
                    reason: "window capture delivery refused".into(),
                },
            );
            write_msg(
                stream,
                &Response::Error {
                    message: "privileged capability lease expired before window capture delivery"
                        .into(),
                },
            )
        }
        Ok(blob) => {
            let capture_allowed = scope.resolve(handler).is_some_and(|scope| {
                if via_grant {
                    scope.asks(ActorCapability::CaptureWindow)
                        && scope.permits_window(payload.capture.window)
                } else {
                    scope.decide_window_capture(payload.capture.window)
                        == crate::schema::AuthorizationDecision::Permit
                }
            });
            let authorized = handler.capture_security_active()
                && capture_allowed
                && handler.window_capture_target_exists(payload.capture.window);
            if !authorized {
                audit_capability_effect(
                    handler,
                    scope,
                    ActorCapability::CaptureWindow,
                    crate::journal::CapabilityUseAction::Capture,
                    crate::journal::Effect::Refused {
                        reason: "window capture delivery refused".into(),
                    },
                );
                return write_msg(
                    stream,
                    &Response::Error {
                        message: "window capture authorization changed before final delivery"
                            .into(),
                    },
                );
            }
            payload.capture.png_bytes = blob.len();
            audit_capability_effect(
                handler,
                scope,
                ActorCapability::CaptureWindow,
                crate::journal::CapabilityUseAction::Capture,
                crate::journal::Effect::Applied,
            );
            write_msg(
                stream,
                &Response::CaptureWindow {
                    capture: payload.capture,
                },
            )?;
            blob.send(stream)
        }
        Err(error) => {
            audit_capability_effect(
                handler,
                scope,
                ActorCapability::CaptureWindow,
                crate::journal::CapabilityUseAction::Capture,
                crate::journal::Effect::Refused {
                    reason: "window capture delivery refused".into(),
                },
            );
            write_msg(
                stream,
                &Response::Error {
                    message: format!("prepare window capture transfer: {error}"),
                },
            )
        }
    }
}

/// Write one stream frame: the JSON [`Event::StreamFrame`] metadata,
/// followed by its sealed pixel memfd for SHM frames. Live policy is
/// re-checked per frame (ADR-0052): an expired lease or a revoked/narrowed
/// scope ends the stream (`StreamEnded`, lane unregistered, handler
/// notified); an inactive lock/VT gate drops the frame silently and the
/// stream survives. A dmabuf slot frame (protocol 25) references a
/// descriptor the consumer received at start, so only the JSON header —
/// with `slot` set and the slot's byte length — crosses the wire.
#[allow(clippy::too_many_arguments)]
pub(super) fn write_stream_frame<H: Handler>(
    stream: &mut UnixStream,
    payload: StreamFramePayload,
    handler: &H,
    scope: &LiveScopeBinding,
    target: crate::schema::StreamTarget,
    lease_deadline: &Mutex<std::time::Instant>,
    streams: &Mutex<HashMap<u64, StreamLane>>,
) -> io::Result<()> {
    let lease_alive = std::time::Instant::now() < *lease_deadline.lock().unwrap();
    let scope_allows = scope
        .resolve(handler)
        .is_some_and(|scope| scope_permits_stream(&scope, &target));
    if !lease_alive || !scope_allows {
        let reason = if !lease_alive {
            "privileged capability lease expired"
        } else {
            "stream scope was revoked or narrowed"
        };
        let lane = streams.lock().unwrap().remove(&payload.stream_id());
        if lane.is_some() {
            handler.stream_output_stop(payload.stream_id());
        }
        audit_capability_effect(
            handler,
            scope,
            ActorCapability::StreamOutput,
            crate::journal::CapabilityUseAction::Stop,
            crate::journal::Effect::Refused {
                reason: "stream authorization ended".into(),
            },
        );
        return write_msg(
            stream,
            &Event::StreamEnded {
                stream_id: payload.stream_id(),
                reason: reason.to_owned(),
            },
        );
    }
    if !handler.capture_security_active() {
        // Session locked or seat inactive: pause delivery, keep the stream.
        return Ok(());
    }
    match payload {
        StreamFramePayload::Pixels(frame) => {
            let blob = match crate::blob::SealedBlob::new(&frame.pixels) {
                Ok(blob) => blob,
                // A malformed frame (size overflow) is dropped, not fatal.
                Err(_) => return Ok(()),
            };
            write_msg(
                stream,
                &Event::StreamFrame {
                    stream_id: frame.stream_id,
                    sequence: frame.sequence,
                    width: frame.width,
                    height: frame.height,
                    stride: frame.stride,
                    format: frame.format,
                    damage: frame.damage,
                    dropped: frame.dropped,
                    byte_len: blob.len(),
                    slot: None,
                },
            )?;
            blob.send(stream)
        }
        StreamFramePayload::Slot(frame) => write_msg(
            stream,
            &Event::StreamFrame {
                stream_id: frame.stream_id,
                sequence: frame.sequence,
                width: frame.width,
                height: frame.height,
                stride: frame.stride,
                format: frame.format,
                damage: frame.damage,
                dropped: frame.dropped,
                byte_len: frame.byte_len,
                slot: Some(frame.slot),
            },
        ),
    }
}

/// Write a `StreamOutputStarted` reply for a zero-copy dmabuf stream
/// (protocol 25): the JSON response first, then one `0xfd`-marked
/// `SCM_RIGHTS` message per slot in slot order. A failed descriptor send
/// tears down the connection like any other writer error; the client then
/// drops the stream instead of running with a partial slot table.
pub(super) fn write_stream_started(
    stream: &mut UnixStream,
    response: &Response,
    table: &StreamSlotTable,
) -> io::Result<()> {
    write_msg(stream, response)?;
    for fd in &table.fds {
        crate::blob::send_fd(stream, fd.as_raw_fd())?;
    }
    Ok(())
}
