//! Lightweight Client SDK for Mobile, Embedded (ESP32), and Remote AI Actors.

use crate::crypto::AuthToken;
use tessera_model::interaction_domain::{InteractionDomainId, InteractionPrincipalId, SeatId};
use tessera_model::uip::*;

/// A lightweight UIP client helper for constructing standard interaction frames.
#[derive(Debug, Clone)]
pub struct UipClient {
    pub seat_id: SeatId,
    pub principal_id: InteractionPrincipalId,
    pub token: AuthToken,
    pub clock_offset_us: u64,
}

impl UipClient {
    pub fn new(seat_id: SeatId, principal_id: InteractionPrincipalId, token: AuthToken) -> Self {
        Self {
            seat_id,
            principal_id,
            token,
            clock_offset_us: 0,
        }
    }

    /// Build a 2D planar motion frame (e.g. mobile touchpad or absolute stylus).
    pub fn motion_2d(
        &self,
        timestamp: MonotonicTimestampUs,
        norm_x: f64,
        norm_y: f64,
        delta_x: f64,
        delta_y: f64,
        pressure: f32,
    ) -> InteractionFrame {
        InteractionFrame {
            seat_id: self.seat_id,
            principal_id: self.principal_id,
            timestamp,
            revision: AuthorityRevision(0),
            flags: IngestFlags::EMPTY,
            payload: ActionPayload::Continuous(ContinuousManifold::Planar2D {
                norm_x,
                norm_y,
                delta_x,
                delta_y,
                pressure,
                tilt_rad: None,
            }),
        }
    }

    /// Build a 1D scalar frame (e.g. rotary dial, slider, volume knob).
    pub fn scalar_1d(
        &self,
        timestamp: MonotonicTimestampUs,
        channel: u16,
        value: f64,
        velocity: f32,
    ) -> InteractionFrame {
        InteractionFrame {
            seat_id: self.seat_id,
            principal_id: self.principal_id,
            timestamp,
            revision: AuthorityRevision(0),
            flags: IngestFlags::EMPTY,
            payload: ActionPayload::Continuous(ContinuousManifold::Scalar1D {
                channel,
                value,
                velocity,
            }),
        }
    }

    /// Build a discrete trigger frame (e.g. button click, pedal switch).
    pub fn trigger(
        &self,
        timestamp: MonotonicTimestampUs,
        trigger_id: u16,
        state: bool,
    ) -> InteractionFrame {
        InteractionFrame {
            seat_id: self.seat_id,
            principal_id: self.principal_id,
            timestamp,
            revision: AuthorityRevision(0),
            flags: IngestFlags::EMPTY,
            payload: ActionPayload::Discrete(DiscreteTransition::Trigger {
                trigger_id,
                state,
                actuation_force: if state { 1.0 } else { 0.0 },
            }),
        }
    }

    /// Build a semantic transaction frame with optimistic concurrency guard.
    pub fn semantic_intent(
        &self,
        timestamp: MonotonicTimestampUs,
        target_domain: InteractionDomainId,
        target_element: u64,
        precondition_revision: AuthorityRevision,
        intent: IntentPayload,
    ) -> InteractionFrame {
        InteractionFrame {
            seat_id: self.seat_id,
            principal_id: self.principal_id,
            timestamp,
            revision: precondition_revision,
            flags: IngestFlags {
                revision_guarded: true,
                actor_confirmed: false,
                synthetic_drain: false,
            },
            payload: ActionPayload::Semantic(SemanticTransaction {
                target_domain,
                target_element,
                precondition_revision,
                one_time_token: *self.token.as_bytes(),
                intent,
            }),
        }
    }
}
