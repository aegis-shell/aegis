//! Remote actor session management and dead-man fail-safe pipeline.

use crate::crypto::AuthToken;
use aegis_model::interaction_domain::{InteractionDomainId, InteractionPrincipalId, SeatId};
use aegis_model::uip::{InteractionFrame, MonotonicTimestampUs, RemoteSeatTracker};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("seat {0:?} mismatch in session")]
    SeatMismatch(SeatId),
    #[error("principal {0:?} unauthorized")]
    PrincipalMismatch(InteractionPrincipalId),
    #[error("session expired")]
    SessionExpired,
}

/// Lifecycle state of a remote UIP actor session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLifecycle {
    Active,
    Interrupted,
    Draining,
    Terminated,
}

/// One live remote actor session managed by the edge gateway.
pub struct RemoteSession {
    pub token: AuthToken,
    pub principal_id: InteractionPrincipalId,
    pub assigned_seat: SeatId,
    pub allowed_domains: Vec<InteractionDomainId>,
    pub tracker: RemoteSeatTracker,
    pub state: SessionLifecycle,
    pub timeout_duration: Duration,
    pub last_heartbeat: MonotonicTimestampUs,
}

impl RemoteSession {
    pub fn new(
        token: AuthToken,
        principal_id: InteractionPrincipalId,
        assigned_seat: SeatId,
        allowed_domains: Vec<InteractionDomainId>,
        timeout_duration: Duration,
    ) -> Self {
        Self {
            token,
            principal_id,
            assigned_seat,
            allowed_domains,
            tracker: RemoteSeatTracker::new(assigned_seat, principal_id),
            state: SessionLifecycle::Active,
            timeout_duration,
            last_heartbeat: MonotonicTimestampUs(0),
        }
    }

    /// Ingest an interaction frame received from the remote network endpoint.
    pub fn ingest_frame(&mut self, frame: &InteractionFrame) -> Result<(), SessionError> {
        if self.state != SessionLifecycle::Active {
            return Err(SessionError::SessionExpired);
        }
        if frame.seat_id != self.assigned_seat {
            return Err(SessionError::SeatMismatch(frame.seat_id));
        }
        if frame.principal_id != self.principal_id {
            return Err(SessionError::PrincipalMismatch(frame.principal_id));
        }

        self.last_heartbeat = frame.timestamp;
        self.tracker.observe_ingest(frame);
        Ok(())
    }

    /// Check if the session has exceeded its dead-man heartbeat threshold.
    pub fn check_heartbeat(&mut self, now: MonotonicTimestampUs) -> Option<Vec<InteractionFrame>> {
        if self.state != SessionLifecycle::Active {
            return None;
        }

        let elapsed_us = now.0.saturating_sub(self.last_heartbeat.0);
        let timeout_us = self.timeout_duration.as_micros() as u64;

        if elapsed_us > timeout_us {
            self.state = SessionLifecycle::Draining;
            let drain_frames = self.tracker.generate_drain_sequence(now);
            self.state = SessionLifecycle::Terminated;
            Some(drain_frames)
        } else {
            None
        }
    }
}
