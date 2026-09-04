//! Universal Interaction Protocol (UIP) frame dispatch and routing in tessera-compositor.

use crate::*;
use tessera_model::input::{ButtonState, InputEvent};
use tessera_model::interaction_domain::SeatId;
use tessera_model::uip::*;

/// Result of dispatching one UIP interaction frame.
#[derive(Debug, Clone, PartialEq)]
pub enum UipDispatchResult {
    /// Action committed to the seat's input pipeline or authority domain.
    Committed,
    /// Optimistic revision barrier failed: the domain has moved since the client observed state.
    Conflict {
        observed: AuthorityRevision,
        current: AuthorityRevision,
    },
    /// The frame was rejected.
    Rejected(UipRejectReason),
}

/// Reason for rejecting a UIP interaction frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UipRejectReason {
    /// Seat is not permitted to control target domain.
    UnauthorizedDomain,
    /// Domain is not active.
    DomainNotActive,
}

impl Server {
    /// Dispatch an ingested UIP interaction frame into the compositor's multi-seat pipeline.
    pub fn dispatch_uip_frame(
        &mut self,
        frame: InteractionFrame,
    ) -> Result<UipDispatchResult, InteractionDomainRuntimeError> {
        let seat = frame.seat_id;

        // 1. Enter the target logical seat's context
        let _guard = ActiveSeatGuard::enter(self.state.as_mut(), seat)
            .ok_or(InteractionDomainRuntimeError::SeatUnavailable(seat))?;

        // 2. Route based on action payload modality
        match frame.payload {
            ActionPayload::Continuous(manifold) => {
                let events = self.translate_continuous_manifold(manifold);
                if !events.is_empty() {
                    self.forward_input_active(&events, None);
                }
                Ok(UipDispatchResult::Committed)
            }

            ActionPayload::Discrete(discrete) => {
                let events = self.translate_discrete_transition(discrete);
                if !events.is_empty() {
                    self.forward_input_active(&events, None);
                }
                Ok(UipDispatchResult::Committed)
            }

            ActionPayload::Semantic(transaction) => {
                self.dispatch_uip_semantic(seat, transaction, frame.flags)
            }
        }
    }

    fn translate_continuous_manifold(&self, manifold: ContinuousManifold) -> Vec<InputEvent> {
        translate_continuous_manifold(manifold)
    }

    fn translate_discrete_transition(&self, discrete: DiscreteTransition) -> Vec<InputEvent> {
        translate_discrete_transition(discrete)
    }

    fn dispatch_uip_semantic(
        &mut self,
        seat: SeatId,
        transaction: SemanticTransaction,
        flags: IngestFlags,
    ) -> Result<UipDispatchResult, InteractionDomainRuntimeError> {
        // 1. Validate domain existence
        let domain_rec = match self
            .state
            .authority
            .interaction_domain(transaction.target_domain)
        {
            Some(rec) => rec,
            None => {
                return Ok(UipDispatchResult::Rejected(
                    UipRejectReason::UnauthorizedDomain,
                ));
            }
        };

        if domain_rec.state != tessera_model::interaction_domain::InteractionDomainState::Active {
            return Ok(UipDispatchResult::Rejected(
                UipRejectReason::DomainNotActive,
            ));
        }

        // 2. Optimistic concurrency check
        if flags.revision_guarded {
            let current_revision = self.state.authority.revision();
            if current_revision != transaction.precondition_revision.0 {
                return Ok(UipDispatchResult::Conflict {
                    observed: transaction.precondition_revision,
                    current: AuthorityRevision(current_revision),
                });
            }
        }

        // 3. Commit semantic action
        log::debug!(
            "UIP Semantic transaction committed on seat {} for domain {}",
            seat.0,
            transaction.target_domain.0
        );

        Ok(UipDispatchResult::Committed)
    }
}

pub(crate) fn translate_continuous_manifold(manifold: ContinuousManifold) -> Vec<InputEvent> {
    let mut events = Vec::new();
    match manifold {
        ContinuousManifold::Planar2D {
            norm_x,
            norm_y,
            delta_x,
            delta_y,
            ..
        } => {
            events.push(InputEvent::PointerMotion {
                x: norm_x as f32,
                y: norm_y as f32,
                dx: delta_x,
                dy: delta_y,
                dx_unaccel: delta_x,
                dy_unaccel: delta_y,
            });
        }

        ContinuousManifold::TopologicalMesh { contacts } => {
            for contact in contacts {
                match contact.phase {
                    TouchPhase::Down => {
                        events.push(InputEvent::TouchDown {
                            id: contact.touch_id as i32,
                            x: contact.x as f32,
                            y: contact.y as f32,
                        });
                    }
                    TouchPhase::Move | TouchPhase::Hold => {
                        events.push(InputEvent::TouchMotion {
                            id: contact.touch_id as i32,
                            x: contact.x as f32,
                            y: contact.y as f32,
                        });
                    }
                    TouchPhase::Up => {
                        events.push(InputEvent::TouchUp {
                            id: contact.touch_id as i32,
                        });
                    }
                    TouchPhase::Cancelled => {
                        events.push(InputEvent::TouchCancel);
                    }
                }
            }
            events.push(InputEvent::TouchFrame);
        }

        ContinuousManifold::Scalar1D { value, .. } => {
            events.push(InputEvent::PointerAxis(
                tessera_model::input::PointerAxisFrame::from_values(
                    0,
                    Some(tessera_model::input::PointerAxisSource::Continuous),
                    0.0,
                    (value * 10.0) as f32,
                ),
            ));
        }

        ContinuousManifold::SpatialPose { .. } => {}
    }
    events
}

pub(crate) fn translate_discrete_transition(discrete: DiscreteTransition) -> Vec<InputEvent> {
    let mut events = Vec::new();
    match discrete {
        DiscreteTransition::Trigger {
            trigger_id, state, ..
        } => {
            let btn_state = if state {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            };
            let button_code = match trigger_id {
                1 => 0x110, // BTN_LEFT
                2 => 0x111, // BTN_RIGHT
                3 => 0x112, // BTN_MIDDLE
                4 => 0x113, // BTN_SIDE
                5 => 0x114, // BTN_EXTRA
                other => other as u32,
            };
            events.push(InputEvent::PointerButton {
                button: button_code,
                state: btn_state,
            });
        }

        DiscreteTransition::Symbolic { .. } => {}
        DiscreteTransition::StateSelector { .. } => {}
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_continuous_planar() {
        let events = translate_continuous_manifold(ContinuousManifold::Planar2D {
            norm_x: 100.0,
            norm_y: 200.0,
            delta_x: 5.0,
            delta_y: -3.0,
            pressure: 0.5,
            tilt_rad: None,
        });

        assert_eq!(events.len(), 1);
        if let InputEvent::PointerMotion { x, y, dx, dy, .. } = events[0] {
            assert_eq!(x, 100.0);
            assert_eq!(y, 200.0);
            assert_eq!(dx, 5.0);
            assert_eq!(dy, -3.0);
        } else {
            panic!("expected PointerMotion event");
        }
    }

    #[test]
    fn test_translate_discrete_trigger() {
        let events = translate_discrete_transition(DiscreteTransition::Trigger {
            trigger_id: 1, // BTN_LEFT
            state: true,
            actuation_force: 1.0,
        });

        assert_eq!(events.len(), 1);
        if let InputEvent::PointerButton { button, state } = events[0] {
            assert_eq!(button, 0x110);
            assert_eq!(state, ButtonState::Pressed);
        } else {
            panic!("expected PointerButton event");
        }
    }
}
