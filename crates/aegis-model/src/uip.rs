//! Universal Interaction Protocol (UIP) domain models.
//!
//! Provides a transport- and HID-agnostic interaction foundation for Aegis,
//! unifying physical hardware, remote networked devices (phones, tablets, IoT dials),
//! and autonomous agents under consistent mathematical manifolds, discrete transitions,
//! and transactional semantic mutations.

use crate::interaction_domain::{InteractionDomainId, InteractionPrincipalId, SeatId};
use std::collections::{HashMap, HashSet};

/// Monotonic timestamp in microseconds, used for causal ordering and jitter buffer management.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MonotonicTimestampUs(pub u64);

/// Observed state revision of an interaction domain or authority context.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AuthorityRevision(pub u64);

// ============================================================================
// 1. Action Ingest Primitives (Universal Input Plane)
// ============================================================================

/// Unified action ingest payload.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub enum ActionPayload {
    /// Continuous physical manifolds (high-frequency streaming).
    Continuous(ContinuousManifold),
    /// Discrete state transitions and symbolic tokens.
    Discrete(DiscreteTransition),
    /// Structured semantic mutations with precondition guards.
    Semantic(SemanticTransaction),
}

/// Continuous manifold: multi-dimensional physical and geometric measurements.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub enum ContinuousManifold {
    /// 1D continuous scalar (e.g. rotary dial, linear fader, scroll axis, throttle pedal).
    Scalar1D {
        channel: u16,
        /// Normalized position in [0.0, 1.0] or relative physical delta.
        value: f64,
        /// Velocity of movement (used for physical damping/inertia simulation).
        velocity: f32,
    },
    /// 2D continuous planar input (touchpad, graphic tablet, relative mouse, trackball).
    Planar2D {
        norm_x: f64,
        norm_y: f64,
        delta_x: f64,
        delta_y: f64,
        pressure: f32,
        tilt_rad: Option<(f32, f32)>,
    },
    /// 6-DoF spatial pose (spatial computing controllers, air mouse, head tracking).
    SpatialPose {
        position: [f32; 3],
        orientation: [f32; 4],
        linear_velocity: [f32; 3],
        angular_velocity: [f32; 3],
        confidence: f32,
    },
    /// Topological multi-touch mesh.
    TopologicalMesh {
        contacts: Vec<TouchContactNode>,
    },
}

/// A single contact point in a topological multi-touch surface.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct TouchContactNode {
    pub touch_id: u32,
    pub x: f64,
    pub y: f64,
    pub major_axis: f32,
    pub minor_axis: f32,
    pub phase: TouchPhase,
}

/// Phase of a touch contact node.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Down,
    Move,
    Hold,
    Up,
    Cancelled,
}

/// Discrete state transitions: switches, symbolic text, and selectors.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub enum DiscreteTransition {
    /// Physical trigger state (buttons, switches, foot pedals).
    Trigger {
        trigger_id: u16,
        state: bool,
        actuation_force: f32,
    },
    /// Symbolic text submission (direct UTF-8 stream, avoiding scancode emulation).
    Symbolic {
        text: String,
        is_commit: bool,
    },
    /// Discrete multi-position selector (mode switches, stepped knobs).
    StateSelector {
        selector_id: u16,
        state_index: u32,
    },
}

/// Structured semantic transaction with precondition barriers.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticTransaction {
    /// Targeted interaction domain.
    pub target_domain: InteractionDomainId,
    /// Targeted semantic element or target descriptor.
    pub target_element: u64,
    /// Optimistic concurrency barrier: expected revision of the domain.
    pub precondition_revision: AuthorityRevision,
    /// One-time authorization nonce / token (anti-replay).
    pub one_time_token: [u8; 32],
    /// Semantic intent payload.
    pub intent: IntentPayload,
}

/// Semantic intent payload.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub enum IntentPayload {
    Activate,
    Dismiss,
    SelectRange { start: u32, end: u32 },
    InvokeAction { action_name: String, parameters: Vec<u8> },
}

// ============================================================================
// 2. Interaction Ingest Frame & Flags
// ============================================================================

/// Ingest flags modifying how an interaction frame is validated and routed.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IngestFlags {
    /// Enforce `precondition_revision` check.
    pub revision_guarded: bool,
    /// Actor has hardware biometric or secure element confirmation.
    pub actor_confirmed: bool,
    /// Synthetic frame generated by the dead-man drain sequence.
    pub synthetic_drain: bool,
}

impl IngestFlags {
    pub const EMPTY: Self = Self {
        revision_guarded: false,
        actor_confirmed: false,
        synthetic_drain: false,
    };

    pub const DRAIN: Self = Self {
        revision_guarded: false,
        actor_confirmed: false,
        synthetic_drain: true,
    };
}

/// Standard interaction frame ingested into Aegis.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct InteractionFrame {
    pub seat_id: SeatId,
    pub principal_id: InteractionPrincipalId,
    pub timestamp: MonotonicTimestampUs,
    pub revision: AuthorityRevision,
    pub flags: IngestFlags,
    pub payload: ActionPayload,
}

// ============================================================================
// 3. Asymmetric State Perception Projections
// ============================================================================

/// Perception subscription requested by a remote or local actor.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub enum PerceptionSubscription {
    /// Blind endpoint (e.g. macro keypad, foot pedal): receives no state stream.
    Blind,
    /// Variable state stream (e.g. rotary dial OLED, status indicators).
    VariableStream {
        variables: Vec<String>,
    },
    /// Semantic tree slice (e.g. smartphone companion UI, accessibility deck).
    SemanticTree {
        domain: InteractionDomainId,
    },
    /// Rendered visual buffer (e.g. companion display, tablet stylus canvas).
    SurfaceBuffer {
        domain: InteractionDomainId,
        max_fps: u32,
    },
}

// ============================================================================
// 4. Dead-Man Switch & Ingest Tracker
// ============================================================================

/// Tracks active state for one remote seat to ensure deterministic auto-drain on disconnect.
#[derive(Debug, Clone)]
pub struct RemoteSeatTracker {
    pub seat_id: SeatId,
    pub principal_id: InteractionPrincipalId,
    pub active_triggers: HashSet<u16>,
    pub active_touch_points: HashMap<u32, (f64, f64)>,
    pub last_seen: MonotonicTimestampUs,
}

impl RemoteSeatTracker {
    pub fn new(seat_id: SeatId, principal_id: InteractionPrincipalId) -> Self {
        Self {
            seat_id,
            principal_id,
            active_triggers: HashSet::new(),
            active_touch_points: HashMap::new(),
            last_seen: MonotonicTimestampUs(0),
        }
    }

    /// Observe an ingested frame to keep track of unreleased physical states.
    pub fn observe_ingest(&mut self, frame: &InteractionFrame) {
        self.last_seen = frame.timestamp;
        match &frame.payload {
            ActionPayload::Discrete(DiscreteTransition::Trigger { trigger_id, state, .. }) => {
                if *state {
                    self.active_triggers.insert(*trigger_id);
                } else {
                    self.active_triggers.remove(trigger_id);
                }
            }
            ActionPayload::Continuous(ContinuousManifold::TopologicalMesh { contacts }) => {
                for contact in contacts {
                    match contact.phase {
                        TouchPhase::Down | TouchPhase::Move | TouchPhase::Hold => {
                            self.active_touch_points.insert(contact.touch_id, (contact.x, contact.y));
                        }
                        TouchPhase::Up | TouchPhase::Cancelled => {
                            self.active_touch_points.remove(&contact.touch_id);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Generate an atomic sequence of cancellation/release frames for dead-man recovery.
    pub fn generate_drain_sequence(&mut self, now: MonotonicTimestampUs) -> Vec<InteractionFrame> {
        let mut drain_frames = Vec::new();

        // 1. Release all active triggers/keys
        for trigger_id in self.active_triggers.drain() {
            drain_frames.push(InteractionFrame {
                seat_id: self.seat_id,
                principal_id: self.principal_id,
                timestamp: now,
                revision: AuthorityRevision(0),
                flags: IngestFlags::DRAIN,
                payload: ActionPayload::Discrete(DiscreteTransition::Trigger {
                    trigger_id,
                    state: false,
                    actuation_force: 0.0,
                }),
            });
        }

        // 2. Cancel all active touch points
        if !self.active_touch_points.is_empty() {
            let cancel_contacts: Vec<TouchContactNode> = self
                .active_touch_points
                .drain()
                .map(|(touch_id, (x, y))| TouchContactNode {
                    touch_id,
                    x,
                    y,
                    major_axis: 0.0,
                    minor_axis: 0.0,
                    phase: TouchPhase::Cancelled,
                })
                .collect();

            drain_frames.push(InteractionFrame {
                seat_id: self.seat_id,
                principal_id: self.principal_id,
                timestamp: now,
                revision: AuthorityRevision(0),
                flags: IngestFlags::DRAIN,
                payload: ActionPayload::Continuous(ContinuousManifold::TopologicalMesh {
                    contacts: cancel_contacts,
                }),
            });
        }

        drain_frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dead_man_drain_generation() {
        let seat = SeatId(42);
        let principal = InteractionPrincipalId(7);
        let mut tracker = RemoteSeatTracker::new(seat, principal);

        // 1. Simulate key press and touch down
        tracker.observe_ingest(&InteractionFrame {
            seat_id: seat,
            principal_id: principal,
            timestamp: MonotonicTimestampUs(100),
            revision: AuthorityRevision(1),
            flags: IngestFlags::EMPTY,
            payload: ActionPayload::Discrete(DiscreteTransition::Trigger {
                trigger_id: 12,
                state: true,
                actuation_force: 1.0,
            }),
        });

        tracker.observe_ingest(&InteractionFrame {
            seat_id: seat,
            principal_id: principal,
            timestamp: MonotonicTimestampUs(105),
            revision: AuthorityRevision(1),
            flags: IngestFlags::EMPTY,
            payload: ActionPayload::Continuous(ContinuousManifold::TopologicalMesh {
                contacts: vec![TouchContactNode {
                    touch_id: 1,
                    x: 0.5,
                    y: 0.5,
                    major_axis: 1.0,
                    minor_axis: 1.0,
                    phase: TouchPhase::Down,
                }],
            }),
        });

        assert_eq!(tracker.active_triggers.len(), 1);
        assert_eq!(tracker.active_touch_points.len(), 1);

        // 2. Trigger dead-man switch drain
        let drain = tracker.generate_drain_sequence(MonotonicTimestampUs(200));
        assert_eq!(drain.len(), 2);
        assert!(tracker.active_triggers.is_empty());
        assert!(tracker.active_touch_points.is_empty());

        // 3. Verify all drain frames have synthetic drain flag
        for frame in &drain {
            assert!(frame.flags.synthetic_drain);
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_uip_serde_roundtrip() {
        let frame = InteractionFrame {
            seat_id: SeatId(3),
            principal_id: InteractionPrincipalId(9),
            timestamp: MonotonicTimestampUs(12345678),
            revision: AuthorityRevision(42),
            flags: IngestFlags {
                revision_guarded: true,
                actor_confirmed: true,
                synthetic_drain: false,
            },
            payload: ActionPayload::Continuous(ContinuousManifold::Planar2D {
                norm_x: 0.25,
                norm_y: 0.75,
                delta_x: 1.5,
                delta_y: -2.0,
                pressure: 0.8,
                tilt_rad: Some((0.1, 0.2)),
            }),
        };

        let json = serde_json::to_string(&frame).expect("serialize should succeed");
        let decoded: InteractionFrame =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(frame, decoded);
    }
}
