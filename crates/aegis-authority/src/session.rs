use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ActorPrincipal;

const MAX_RETAINED_ACTOR_SESSIONS: usize = 4_096;

/// Compositor-issued identity of one live Actor session.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct ActorSessionId(pub u64);

impl ActorSessionId {
    pub fn is_valid(self) -> bool {
        self.0 != 0
    }
}

/// Explicit lifecycle of a live Actor execution context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorSessionState {
    Active,
    Suspended,
    Revoked,
    Expired,
}

/// Bounded runtime policy for one Actor session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActorSessionPolicy {
    pub ttl: Duration,
    pub idle_timeout: Duration,
    pub max_pending_actions: usize,
    pub max_observations: usize,
}

impl Default for ActorSessionPolicy {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(15 * 60),
            idle_timeout: Duration::from_secs(5 * 60),
            max_pending_actions: 64,
            max_observations: 64,
        }
    }
}

impl ActorSessionPolicy {
    pub fn validate(self) -> Result<Self, &'static str> {
        if !(Duration::from_secs(1)..=Duration::from_secs(24 * 60 * 60)).contains(&self.ttl) {
            return Err("Actor session ttl must be between one second and 24 hours");
        }
        if self.idle_timeout < Duration::from_secs(1) || self.idle_timeout > self.ttl {
            return Err("Actor session idle timeout must be positive and no greater than ttl");
        }
        if self.max_pending_actions == 0 || self.max_pending_actions > 4_096 {
            return Err("Actor session pending-action quota is out of range");
        }
        if self.max_observations == 0 || self.max_observations > 4_096 {
            return Err("Actor session observation quota is out of range");
        }
        Ok(self)
    }
}

/// Public, clock-independent Actor session snapshot.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActorSessionSnapshot {
    pub id: ActorSessionId,
    pub principal: Option<ActorPrincipal>,
    pub connection_id: u64,
    pub state: ActorSessionState,
    pub ttl_ms: u64,
    pub idle_timeout_ms: u64,
    pub max_pending_actions: u32,
    pub max_observations: u32,
}

struct ActorSessionRecord {
    snapshot: ActorSessionSnapshot,
    created_at: Instant,
    last_activity: Instant,
    expires_at: Instant,
}

/// Main-loop-owned Actor session lifecycle registry.
///
/// Durable identity is deliberately separate. Disconnect, TTL, or idle
/// expiry kills this live context and all resources bound to its id without
/// deleting the paired principal profile.
#[derive(Default)]
pub struct ActorSessionRegistry {
    sessions: BTreeMap<ActorSessionId, ActorSessionRecord>,
    next_id: u64,
}

impl ActorSessionRegistry {
    pub fn start(
        &mut self,
        connection_id: u64,
        principal: Option<ActorPrincipal>,
        policy: ActorSessionPolicy,
    ) -> Result<ActorSessionSnapshot, String> {
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "Actor session id space exhausted".to_owned())?;
        self.start_with_id(
            ActorSessionId(self.next_id),
            connection_id,
            principal,
            policy,
        )
    }

    pub fn start_with_id(
        &mut self,
        id: ActorSessionId,
        connection_id: u64,
        principal: Option<ActorPrincipal>,
        policy: ActorSessionPolicy,
    ) -> Result<ActorSessionSnapshot, String> {
        self.prune_terminal();
        if self.sessions.len() >= MAX_RETAINED_ACTOR_SESSIONS {
            return Err("Actor session registry is at capacity".into());
        }
        if connection_id == 0 {
            return Err("Actor session connection id is invalid".into());
        }
        let policy = policy.validate().map_err(str::to_owned)?;
        if !id.is_valid() {
            return Err("Actor session id is invalid".into());
        }
        if self.sessions.contains_key(&id) {
            return Err("Actor session id is already live or retained".into());
        }
        if self.sessions.values().any(|record| {
            record.snapshot.connection_id == connection_id
                && matches!(
                    record.snapshot.state,
                    ActorSessionState::Active | ActorSessionState::Suspended
                )
        }) {
            return Err("connection already owns a live Actor session".into());
        }
        self.next_id = self.next_id.max(id.0);
        let now = Instant::now();
        let snapshot = ActorSessionSnapshot {
            id,
            principal,
            connection_id,
            state: ActorSessionState::Active,
            ttl_ms: millis(policy.ttl),
            idle_timeout_ms: millis(policy.idle_timeout),
            max_pending_actions: policy.max_pending_actions as u32,
            max_observations: policy.max_observations as u32,
        };
        self.sessions.insert(
            id,
            ActorSessionRecord {
                snapshot: snapshot.clone(),
                created_at: now,
                last_activity: now,
                expires_at: now + policy.ttl,
            },
        );
        Ok(snapshot)
    }

    pub fn authorize(&mut self, id: ActorSessionId) -> Result<ActorSessionSnapshot, String> {
        let now = Instant::now();
        let record = self
            .sessions
            .get_mut(&id)
            .ok_or_else(|| "unknown Actor session".to_owned())?;
        expire_record(record, now);
        match record.snapshot.state {
            ActorSessionState::Active => {
                record.last_activity = now;
                Ok(record.snapshot.clone())
            }
            ActorSessionState::Suspended => Err("Actor session is suspended".into()),
            ActorSessionState::Revoked => Err("Actor session is revoked".into()),
            ActorSessionState::Expired => Err("Actor session is expired".into()),
        }
    }

    /// Resolve and authorize the one live Actor session owned by an IPC
    /// connection. Session identity is intentionally independent from the
    /// transport's connection id.
    pub fn authorize_connection(
        &mut self,
        connection_id: u64,
    ) -> Result<ActorSessionSnapshot, String> {
        let id = self
            .sessions
            .values()
            .find(|record| record.snapshot.connection_id == connection_id)
            .map(|record| record.snapshot.id)
            .ok_or_else(|| "connection has no Actor session".to_owned())?;
        self.authorize(id)
    }

    pub fn suspend(&mut self, id: ActorSessionId) -> Result<(), String> {
        self.transition(id, ActorSessionState::Suspended)
    }

    pub fn resume(&mut self, id: ActorSessionId) -> Result<(), String> {
        let now = Instant::now();
        let record = self
            .sessions
            .get_mut(&id)
            .ok_or_else(|| "unknown Actor session".to_owned())?;
        expire_record(record, now);
        if record.snapshot.state != ActorSessionState::Suspended {
            return Err("only a suspended Actor session can resume".into());
        }
        record.snapshot.state = ActorSessionState::Active;
        record.last_activity = now;
        Ok(())
    }

    pub fn revoke(&mut self, id: ActorSessionId) -> Result<(), String> {
        self.transition(id, ActorSessionState::Revoked)
    }

    pub fn revoke_connection(&mut self, connection_id: u64) -> Vec<ActorSessionSnapshot> {
        let revoked: Vec<_> = self
            .sessions
            .values()
            .filter_map(|record| {
                (record.snapshot.connection_id == connection_id).then_some(record.snapshot.clone())
            })
            .collect();
        for snapshot in &revoked {
            self.sessions.remove(&snapshot.id);
        }
        revoked
    }

    /// Revoke and forget every live context owned by a durable principal.
    /// The returned ids let the caller cascade revocation into resource and
    /// observation registries without coupling those registries together.
    pub fn revoke_principal(&mut self, principal: &ActorPrincipal) -> Vec<ActorSessionSnapshot> {
        let revoked: Vec<_> = self
            .sessions
            .values()
            .filter_map(|record| {
                (record.snapshot.principal.as_ref() == Some(principal))
                    .then_some(record.snapshot.clone())
            })
            .collect();
        for snapshot in &revoked {
            self.sessions.remove(&snapshot.id);
        }
        revoked
    }

    /// Expire and forget all due sessions. Returning complete snapshots lets
    /// the runtime atomically cascade revocation into every session-bound
    /// registry without retaining terminal execution contexts indefinitely.
    pub fn expire_due(&mut self) -> Vec<ActorSessionSnapshot> {
        let now = Instant::now();
        let mut expired = Vec::new();
        for record in self.sessions.values_mut() {
            expire_record(record, now);
            if record.snapshot.state == ActorSessionState::Expired {
                expired.push(record.snapshot.clone());
            }
        }
        for snapshot in &expired {
            self.sessions.remove(&snapshot.id);
        }
        expired
    }

    pub fn snapshot(&self, id: ActorSessionId) -> Option<ActorSessionSnapshot> {
        self.sessions.get(&id).map(|record| record.snapshot.clone())
    }

    fn prune_terminal(&mut self) {
        self.sessions.retain(|_, record| {
            matches!(
                record.snapshot.state,
                ActorSessionState::Active | ActorSessionState::Suspended
            )
        });
    }

    fn transition(&mut self, id: ActorSessionId, target: ActorSessionState) -> Result<(), String> {
        let now = Instant::now();
        let record = self
            .sessions
            .get_mut(&id)
            .ok_or_else(|| "unknown Actor session".to_owned())?;
        expire_record(record, now);
        if !matches!(
            record.snapshot.state,
            ActorSessionState::Active | ActorSessionState::Suspended
        ) {
            return Err("terminal Actor session cannot transition".into());
        }
        record.snapshot.state = target;
        Ok(())
    }
}

fn expire_record(record: &mut ActorSessionRecord, now: Instant) {
    if matches!(
        record.snapshot.state,
        ActorSessionState::Revoked | ActorSessionState::Expired
    ) {
        return;
    }
    let idle_timeout = Duration::from_millis(record.snapshot.idle_timeout_ms);
    if now >= record.expires_at
        || now.saturating_duration_since(record.last_activity) >= idle_timeout
    {
        record.snapshot.state = ActorSessionState::Expired;
    }
    debug_assert!(record.created_at <= record.last_activity);
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_explicit_bounded_and_connection_revocable() {
        let mut sessions = ActorSessionRegistry::default();
        let snapshot = sessions
            .start(
                7,
                Some(ActorPrincipal::new("prin_a").unwrap()),
                ActorSessionPolicy::default(),
            )
            .unwrap();
        assert!(snapshot.id.is_valid());
        assert_ne!(snapshot.id.0, snapshot.connection_id);
        assert_eq!(sessions.authorize(snapshot.id).unwrap().connection_id, 7);
        assert_eq!(sessions.authorize_connection(7).unwrap().id, snapshot.id);
        sessions.suspend(snapshot.id).unwrap();
        assert!(sessions.authorize(snapshot.id).is_err());
        sessions.resume(snapshot.id).unwrap();
        assert_eq!(sessions.revoke_connection(7), vec![snapshot.clone()]);
        assert!(sessions.authorize(snapshot.id).is_err());
    }

    #[test]
    fn invalid_or_duplicate_sessions_fail_closed() {
        let mut sessions = ActorSessionRegistry::default();
        let bad = ActorSessionPolicy {
            max_pending_actions: 0,
            ..ActorSessionPolicy::default()
        };
        assert!(sessions.start(1, None, bad).is_err());
        sessions
            .start(1, None, ActorSessionPolicy::default())
            .unwrap();
        assert!(
            sessions
                .start(1, None, ActorSessionPolicy::default())
                .is_err()
        );
    }

    #[test]
    fn forgetting_a_principal_revokes_only_its_live_sessions() {
        let mut sessions = ActorSessionRegistry::default();
        let first = ActorPrincipal::new("prin_a").unwrap();
        let second = ActorPrincipal::new("prin_b").unwrap();
        let a = sessions
            .start(1, Some(first.clone()), ActorSessionPolicy::default())
            .unwrap();
        let b = sessions
            .start(2, Some(second), ActorSessionPolicy::default())
            .unwrap();

        assert_eq!(sessions.revoke_principal(&first), vec![a.clone()]);
        assert!(sessions.authorize(a.id).is_err());
        assert!(sessions.authorize(b.id).is_ok());
    }

    #[test]
    fn due_sessions_are_explicitly_collected_and_forgotten() {
        let mut sessions = ActorSessionRegistry::default();
        let session = sessions
            .start(7, None, ActorSessionPolicy::default())
            .unwrap();
        sessions.sessions.get_mut(&session.id).unwrap().expires_at = Instant::now();

        let expired = sessions.expire_due();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, session.id);
        assert_eq!(expired[0].state, ActorSessionState::Expired);
        assert!(sessions.snapshot(session.id).is_none());
    }
}
