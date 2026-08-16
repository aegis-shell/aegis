//! Connection-bound observation lease retention (ADR-0125).
//!
//! Observation tokens are bound by the compositor to the exact connection
//! that received them, so the client that observed must stay open until the
//! follow-up action consumes the token. `ObservationLeases` retains those
//! clients, evicting expired leases and bounding the total so a busy agent
//! cannot leak connections.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use aegis_ipc::{Client, ObservationToken};

struct PendingObservation {
    expires_at: Instant,
    client: Client,
}

/// A bounded map from observation token to the connection that owns it.
pub struct ObservationLeases {
    pending: BTreeMap<String, PendingObservation>,
    max: usize,
}

impl ObservationLeases {
    pub fn new(max: usize) -> Self {
        Self {
            pending: BTreeMap::new(),
            max,
        }
    }

    /// Retain `client` under `token` until the token's expiry, evicting
    /// expired leases first and the earliest-expiring leases when full.
    pub fn retain(&mut self, token: &ObservationToken, ttl_ms: u64, client: Client) {
        let now = Instant::now();
        self.pending.retain(|_, pending| pending.expires_at > now);
        while self.pending.len() >= self.max {
            let Some(oldest) = self
                .pending
                .iter()
                .min_by_key(|(_, pending)| pending.expires_at)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            self.pending.remove(&oldest);
        }
        let expires_at = now.checked_add(Duration::from_millis(ttl_ms)).unwrap_or(now);
        self.pending
            .insert(token.0.clone(), PendingObservation { expires_at, client });
    }

    /// Take the connection owning `token`, consuming the lease. Expired
    /// leases are evicted and never returned.
    pub fn take(&mut self, token: &str) -> Option<Client> {
        let now = Instant::now();
        self.pending.retain(|_, pending| pending.expires_at > now);
        self.pending.remove(token).map(|pending| pending.client)
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}
