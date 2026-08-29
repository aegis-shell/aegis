use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use aegis_model::input::SyntheticInputAction;
use aegis_model::interaction_domain::InteractionDomainId;
use aegis_model::semantic::{
    SemanticAction, SemanticActionIntent, SemanticObjectId, SemanticSnapshot, SemanticSource,
};
use aegis_model::window::WindowId;

use super::{ActorPrincipal, ActorSessionId};

const OBSERVATION_TTL: Duration = Duration::from_secs(15);
const MAX_LIVE_OBSERVATIONS: usize = 1024;
const MAX_RETAINED_SEMANTIC_OBJECTS: usize = 16_384;
const DEFAULT_MAX_OBSERVATIONS_PER_ACTOR: usize = 64;

/// One authenticated Actor bound to a live broker connection.
///
/// The principal is compositor-issued. `None` is reserved for trusted local
/// components; it never represents a self-asserted Agent identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorBinding {
    pub session: ActorSessionId,
    pub connection_id: u64,
    pub principal: Option<ActorPrincipal>,
}

/// Opaque bearer reference to one compositor-owned observation lease.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ObservationToken(pub String);

/// A semantic interaction-domain observation usable as one action
/// transaction's precondition without granting framebuffer access.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SemanticObservation {
    pub token: ObservationToken,
    /// Remaining lease time when issued. Informational only; the compositor's
    /// monotonic deadline remains authoritative.
    pub ttl_ms: u64,
    pub snapshot: SemanticSnapshot,
}

/// Observation-bound action intent submitted by an Actor.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ActorActionIntent {
    pub interaction_domain: InteractionDomainId,
    pub target: SemanticObjectId,
    pub observation: ObservationToken,
    pub actions: Vec<SemanticActionIntent>,
}

impl ActorActionIntent {
    pub fn validate(&self) -> Result<(), &'static str> {
        const MAX_INPUT_ACTIONS: usize = 64;
        const MAX_SCROLL_DELTA: f32 = 1_000.0;
        if !self.interaction_domain.is_valid() || !self.target.is_valid() {
            return Err("interaction domain or semantic target is invalid");
        }
        if self.observation.0.len() < 32 || self.observation.0.len() > 128 {
            return Err("observation token length is out of range");
        }
        if self.actions.is_empty() || self.actions.len() > MAX_INPUT_ACTIONS {
            return Err("input action count is out of range");
        }
        for action in &self.actions {
            let SemanticActionIntent::SyntheticInput { actions } = action else {
                match action {
                    SemanticActionIntent::SetValue { value }
                    | SemanticActionIntent::TypeText { text: value }
                        if value.len() > 16_384 || value.contains('\0') =>
                    {
                        return Err("semantic action text is out of range");
                    }
                    _ => {}
                }
                continue;
            };
            if actions.is_empty() || actions.len() > MAX_INPUT_ACTIONS {
                return Err("synthetic fallback action count is out of range");
            }
            for action in actions {
                match *action {
                    SyntheticInputAction::Click { button, .. }
                        if !(0x110..=0x117).contains(&button) =>
                    {
                        return Err("input button code is out of range");
                    }
                    SyntheticInputAction::Scroll { dx, dy, .. }
                        if !dx.is_finite()
                            || !dy.is_finite()
                            || dx.abs() > MAX_SCROLL_DELTA
                            || dy.abs() > MAX_SCROLL_DELTA =>
                    {
                        return Err("input scroll delta is out of range");
                    }
                    SyntheticInputAction::KeyPress { code } if code > 0x2ff => {
                        return Err("input key code is out of range");
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

/// Authoritative receipt created only after the compositor main loop has
/// revalidated and delivered the complete action batch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActorActionReceipt {
    pub action_id: u64,
    pub interaction_domain: InteractionDomainId,
    pub target: SemanticObjectId,
    /// Owning durable toplevel resolved from compositor-held semantics.
    pub window: WindowId,
    pub authority_revision: u64,
    pub actions_applied: u32,
    pub committed_mono_ms: u64,
}

struct ObservationLease {
    actor: ActorBinding,
    snapshot: SemanticSnapshot,
    expires_at: Instant,
    max_observations: usize,
}

/// Bounded, main-loop-owned observation leases.
///
/// Tokens are unguessable, Actor-bound, short-lived, and removed before
/// semantic validation. This makes an owning Actor's action attempt
/// exactly-once even when a precondition fails. A different Actor cannot
/// revoke a leaked token.
#[derive(Default)]
pub struct ObservationLeaseRegistry {
    leases: BTreeMap<ObservationToken, ObservationLease>,
    next_action_id: u64,
}

#[derive(Debug)]
pub struct ValidatedActorAction {
    pub action_id: u64,
    pub window: WindowId,
    pub authority_revision: u64,
    pub source: SemanticSource,
}

impl ObservationLeaseRegistry {
    pub fn discard_all(&mut self) {
        self.leases.clear();
    }

    pub fn discard(&mut self, token: &ObservationToken) {
        self.leases.remove(token);
    }

    pub fn discard_connection(&mut self, connection_id: u64) {
        self.leases
            .retain(|_, lease| lease.actor.connection_id != connection_id);
    }

    pub fn discard_for_actor(&mut self, actor: &ActorBinding, token: &ObservationToken) {
        if self
            .leases
            .get(token)
            .is_some_and(|lease| lease.actor == *actor)
        {
            self.leases.remove(token);
        }
    }

    pub fn issue(
        &mut self,
        actor: ActorBinding,
        snapshot: SemanticSnapshot,
    ) -> Result<SemanticObservation, String> {
        self.issue_bounded(actor, snapshot, DEFAULT_MAX_OBSERVATIONS_PER_ACTOR)
    }

    /// Issue a lease under the owning session's negotiated quota.
    pub fn issue_bounded(
        &mut self,
        actor: ActorBinding,
        snapshot: SemanticSnapshot,
        max_observations: usize,
    ) -> Result<SemanticObservation, String> {
        if max_observations == 0 || max_observations > 4_096 {
            return Err("Actor observation quota is out of range".into());
        }
        let now = Instant::now();
        self.leases.retain(|_, lease| lease.expires_at > now);
        if self
            .leases
            .values()
            .filter(|lease| lease.actor.session == actor.session)
            .count()
            >= max_observations
        {
            return Err("Actor observation quota exhausted".into());
        }
        if snapshot.objects.len() > MAX_RETAINED_SEMANTIC_OBJECTS {
            return Err("semantic observation exceeds the retained-object safety bound".into());
        }
        while self.leases.len() >= MAX_LIVE_OBSERVATIONS
            || self
                .leases
                .values()
                .map(|lease| lease.snapshot.objects.len())
                .sum::<usize>()
                .saturating_add(snapshot.objects.len())
                > MAX_RETAINED_SEMANTIC_OBJECTS
        {
            let Some(oldest) = self
                .leases
                .iter()
                .min_by_key(|(_, lease)| lease.expires_at)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            self.leases.remove(&oldest);
        }

        let token = loop {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes)
                .map_err(|error| format!("generate observation token: {error}"))?;
            let mut encoded = String::with_capacity(bytes.len() * 2);
            use std::fmt::Write as _;
            for byte in bytes {
                write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
            }
            let token = ObservationToken(encoded);
            if !self.leases.contains_key(&token) {
                break token;
            }
        };
        self.leases.insert(
            token.clone(),
            ObservationLease {
                actor,
                snapshot: snapshot.clone(),
                expires_at: now + OBSERVATION_TTL,
                max_observations,
            },
        );
        Ok(SemanticObservation {
            token,
            ttl_ms: OBSERVATION_TTL.as_millis() as u64,
            snapshot,
        })
    }

    /// Replace a capture-time lease with a fresh client-visible lease at
    /// delivery so GPU readback and encoding do not consume the advertised
    /// action window.
    pub fn refresh_for_delivery(
        &mut self,
        token: &ObservationToken,
    ) -> Result<SemanticObservation, String> {
        let lease = self
            .leases
            .remove(token)
            .ok_or_else(|| "capture observation was revoked before delivery".to_owned())?;
        self.issue_bounded(lease.actor, lease.snapshot, lease.max_observations)
    }

    /// Consume and validate one action transaction. `permits_window` is the
    /// caller's live resource-scope decision; keeping that policy callback at
    /// the seam prevents this transport-neutral crate from depending on IPC.
    pub fn consume(
        &mut self,
        actor: &ActorBinding,
        intent: &ActorActionIntent,
        current: &SemanticSnapshot,
        permits_window: impl FnOnce(WindowId) -> bool,
    ) -> Result<ValidatedActorAction, String> {
        let bound_actor = &self
            .leases
            .get(&intent.observation)
            .ok_or_else(|| "unknown, expired, or already consumed observation".to_owned())?
            .actor;
        if bound_actor != actor {
            return Err("observation belongs to a different Actor connection".into());
        }
        let lease = self
            .leases
            .remove(&intent.observation)
            .expect("the Actor-bound observation was present immediately before removal");
        if lease.expires_at <= Instant::now() {
            return Err("observation expired before action commit".into());
        }
        if lease.snapshot.interaction_domain != intent.interaction_domain
            || current.interaction_domain != intent.interaction_domain
        {
            return Err("observation domain does not match action domain".into());
        }
        if lease.snapshot.authority_revision != current.authority_revision {
            return Err(format!(
                "interaction-domain authority changed after observation (observed r{}, current r{})",
                lease.snapshot.authority_revision, current.authority_revision
            ));
        }
        let observed = lease
            .snapshot
            .object(intent.target)
            .ok_or_else(|| "semantic target was not present in the observation".to_owned())?;
        let current_object = current
            .object(intent.target)
            .ok_or_else(|| "semantic target no longer exists".to_owned())?;
        if !permits_window(current_object.window) {
            return Err("semantic target's owning window is out of scope".into());
        }
        if observed != current_object {
            return Err("semantic target state changed after observation".into());
        }
        if !current_object.state.visible
            || !current_object.state.enabled
            || current_object.state.read_only
        {
            return Err("semantic target is not actionable by this interaction domain".into());
        }
        if current_object.source == SemanticSource::Accessibility && intent.actions.len() != 1 {
            return Err(
                "accessibility targets accept exactly one transactional semantic action".into(),
            );
        }
        for action in &intent.actions {
            let required = match action {
                SemanticActionIntent::Invoke => Some(SemanticAction::Invoke),
                SemanticActionIntent::Focus => Some(SemanticAction::Focus),
                SemanticActionIntent::SetValue { .. } => Some(SemanticAction::SetValue),
                SemanticActionIntent::TypeText { .. } => Some(SemanticAction::TypeText),
                SemanticActionIntent::Select { .. } => Some(SemanticAction::Select),
                SemanticActionIntent::Expand => Some(SemanticAction::Expand),
                SemanticActionIntent::Collapse => Some(SemanticAction::Collapse),
                SemanticActionIntent::SyntheticInput { .. } => None,
            };
            if required.is_some_and(|required| !current_object.actions.contains(&required)) {
                return Err(format!(
                    "semantic target does not declare {:?}",
                    required.expect("checked as some")
                ));
            }
            if let SemanticActionIntent::SyntheticInput { actions } = action {
                if current_object.source != SemanticSource::Compositor {
                    return Err(
                        "synthetic fallback is restricted to compositor-owned window roots".into(),
                    );
                }
                for action in actions {
                    let required = match action {
                        SyntheticInputAction::PointerMove { .. }
                        | SyntheticInputAction::Click { .. } => SemanticAction::Pointer,
                        SyntheticInputAction::Scroll { .. } => SemanticAction::Scroll,
                        SyntheticInputAction::KeyPress { .. } => SemanticAction::TypeText,
                    };
                    if !current_object.actions.contains(&required) {
                        return Err(format!("semantic target does not declare {required:?}"));
                    }
                    let position = match action {
                        SyntheticInputAction::PointerMove { position }
                        | SyntheticInputAction::Click { position, .. }
                        | SyntheticInputAction::Scroll { position, .. } => Some(*position),
                        SyntheticInputAction::KeyPress { .. } => None,
                    };
                    if position.is_some_and(|position| {
                        position.x < 0
                            || position.y < 0
                            || position.x >= current_object.local_size.w
                            || position.y >= current_object.local_size.h
                    }) {
                        return Err(
                            "semantic action position is outside the target-local extent".into(),
                        );
                    }
                }
            }
        }
        let window = current_object.window;
        self.next_action_id = self
            .next_action_id
            .checked_add(1)
            .ok_or_else(|| "Actor action id space exhausted".to_owned())?;
        Ok(ValidatedActorAction {
            action_id: self.next_action_id,
            window,
            authority_revision: current.authority_revision,
            source: current_object.source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_model::semantic::{
        SemanticActionIntent, SemanticObject, SemanticRole, SemanticSource, SemanticState,
    };

    fn actor(connection_id: u64, principal: &str) -> ActorBinding {
        ActorBinding {
            session: ActorSessionId(connection_id),
            connection_id,
            principal: Some(ActorPrincipal::new(principal).unwrap()),
        }
    }

    fn snapshot(domain: u64, authority_revision: u64, object_revision: u64) -> SemanticSnapshot {
        SemanticSnapshot {
            interaction_domain: InteractionDomainId(domain),
            authority_revision,
            objects: vec![SemanticObject {
                id: SemanticObjectId::for_window(WindowId(9)),
                parent: None,
                window: WindowId(9),
                source: SemanticSource::Compositor,
                role: SemanticRole::Window,
                name: Some("Checkout".into()),
                description: None,
                value: None,
                app_id: Some("shop.example".into()),
                bounds: aegis_model::Rect::new(0, 0, 800, 600),
                local_size: aegis_model::Size { w: 800, h: 600 },
                state: SemanticState {
                    visible: true,
                    enabled: true,
                    ..Default::default()
                },
                actions: vec![SemanticAction::Pointer],
                revision: object_revision,
            }],
        }
    }

    fn intent(observation: ObservationToken) -> ActorActionIntent {
        ActorActionIntent {
            interaction_domain: InteractionDomainId(7),
            target: SemanticObjectId::for_window(WindowId(9)),
            observation,
            actions: vec![SemanticActionIntent::SyntheticInput {
                actions: vec![SyntheticInputAction::PointerMove {
                    position: aegis_model::Point { x: 20, y: 30 },
                }],
            }],
        }
    }

    #[test]
    fn observation_is_actor_bound_and_single_use() {
        let mut registry = ObservationLeaseRegistry::default();
        let owner = actor(4, "prin_a");
        let observed = snapshot(7, 11, 3);
        let lease = registry.issue(owner.clone(), observed.clone()).unwrap();
        let action = intent(lease.token);
        let validated = registry
            .consume(&owner, &action, &observed, |_| true)
            .unwrap();
        assert_eq!(validated.window, WindowId(9));
        assert!(
            registry
                .consume(&owner, &action, &observed, |_| true)
                .is_err()
        );

        let lease = registry.issue(owner.clone(), observed.clone()).unwrap();
        let action = intent(lease.token);
        assert!(
            registry
                .consume(&actor(5, "prin_a"), &action, &observed, |_| true)
                .unwrap_err()
                .contains("different Actor")
        );
        registry
            .consume(&owner, &action, &observed, |_| true)
            .expect("another Actor cannot revoke the owner's lease");
    }

    #[test]
    fn changed_authority_or_semantics_aborts_and_consumes() {
        let mut registry = ObservationLeaseRegistry::default();
        let owner = actor(4, "prin_a");
        let observed = snapshot(7, 11, 3);
        let lease = registry.issue(owner.clone(), observed.clone()).unwrap();
        let action = intent(lease.token);
        assert!(
            registry
                .consume(&owner, &action, &snapshot(7, 12, 3), |_| true)
                .unwrap_err()
                .contains("authority changed")
        );
        assert!(
            registry
                .consume(&owner, &action, &observed, |_| true)
                .is_err()
        );

        let lease = registry.issue(owner.clone(), observed.clone()).unwrap();
        assert!(
            registry
                .consume(&owner, &intent(lease.token), &snapshot(7, 11, 4), |_| true)
                .unwrap_err()
                .contains("semantic target state changed")
        );
    }

    #[test]
    fn resource_and_coordinate_preconditions_fail_closed() {
        let mut registry = ObservationLeaseRegistry::default();
        let owner = actor(4, "prin_a");
        let observed = snapshot(7, 11, 3);
        let lease = registry.issue(owner.clone(), observed.clone()).unwrap();
        assert!(
            registry
                .consume(&owner, &intent(lease.token), &observed, |_| false)
                .unwrap_err()
                .contains("out of scope")
        );

        let lease = registry.issue(owner.clone(), observed.clone()).unwrap();
        let mut action = intent(lease.token);
        action.actions = vec![SemanticActionIntent::SyntheticInput {
            actions: vec![SyntheticInputAction::Click {
                button: 0x110,
                position: aegis_model::Point { x: 800, y: 10 },
            }],
        }];
        assert!(
            registry
                .consume(&owner, &action, &observed, |_| true)
                .unwrap_err()
                .contains("target-local extent")
        );
    }

    #[test]
    fn disconnect_discard_and_delivery_refresh_are_actor_scoped() {
        let mut registry = ObservationLeaseRegistry::default();
        let owner = actor(4, "prin_a");
        let observed = snapshot(7, 11, 3);
        let first = registry.issue(owner.clone(), observed.clone()).unwrap();
        registry.discard_for_actor(&actor(5, "prin_a"), &first.token);
        registry
            .consume(&owner, &intent(first.token), &observed, |_| true)
            .unwrap();

        let internal = registry.issue(owner.clone(), observed.clone()).unwrap();
        let delivered = registry.refresh_for_delivery(&internal.token).unwrap();
        assert_ne!(delivered.token, internal.token);
        registry.discard_connection(owner.connection_id);
        assert!(
            registry
                .consume(&owner, &intent(delivered.token), &observed, |_| true)
                .is_err()
        );
    }
}
