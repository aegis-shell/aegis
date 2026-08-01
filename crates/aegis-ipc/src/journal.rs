//! The mutation journal (ADR-0033).
//!
//! An in-memory, append-only ring buffer of [`JournalEntry`] records, one per
//! command or Realm authority action the compositor decides, regardless of
//! origin (chrome, keybinding, IPC, or internal cleanup). The journal records
//! the compositor's *decisions* — what it did, by whom, and with what outcome
//! — so the agent can reconstruct recent history without polling.
//!
//! The ring is bounded; oldest entries are evicted when full. `seq` is
//! monotonic across evictions, so a subscriber that falls behind detects the
//! gap and re-queries rather than reasoning over a partial history.
//!
//! See [ADR-0033](../../docs/adr/0033-mutation-journal.md).

use crate::schema::{Command, OpClass, RealmAction, SettingsAction};

/// Who caused a mutation. The agent filters its own echoes and models user
/// intent from the origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Origin {
    /// A chrome component (dock, decorations, launcher, workspace bar).
    Chrome,
    /// A keybinding match.
    Keybinding,
    /// A compositor-owned touchpad gesture (e.g., the three-finger
    /// navigation swipe).
    Gesture,
    /// An IPC `Do` request from connection `conn_id`.
    Ipc { conn_id: u64 },
    /// Internal compositor cleanup (e.g., closing a window whose client
    /// vanished).
    Internal,
}

/// What happened when the compositor applied a command.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Effect {
    /// The command was applied to the model.
    Applied,
    /// The command was refused (scope violation, target gone, etc.).
    Refused { reason: String },
    /// The command was a no-op (nothing changed).
    NoOp,
}

/// The exact mutation the compositor decided.
///
/// Realm actions carry both authority revisions so an observer can correlate
/// a transfer, observer change, lifecycle transition, or output reconfigure
/// with snapshots and captured pixels without racing a later action.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum JournalMutation {
    Command {
        cmd: Command,
    },
    Realm {
        action: RealmAction,
        before_revision: u64,
        after_revision: u64,
    },
    Settings {
        action: SettingsAction,
        before_revision: u64,
        after_revision: u64,
    },
    /// Agent authorization lifecycle (ADR-0088): pairing, runtime grants,
    /// and principal management.
    AgentAuth {
        principal: String,
        action: AgentAuthAction,
    },
}

/// One agent-authorization lifecycle event (ADR-0088), carried by
/// [`JournalMutation::AgentAuth`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum AgentAuthAction {
    /// A principal was issued (interactive pairing or pre-provisioning).
    Paired,
    /// A runtime grant was answered interactively; `persistence` records
    /// how long the decision lives.
    Granted {
        op: OpClass,
        persistence: GrantPersistence,
    },
    /// A recorded runtime grant was revoked by the user.
    GrantRevoked { op: OpClass },
    /// A principal was forgotten; its credential and grants died with it.
    Forgotten,
    /// A principal's display label was changed.
    Renamed,
    /// A principal's approved capability ceiling was replaced.
    CeilingChanged,
}

/// How long an interactively answered runtime grant lives (ADR-0088).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum GrantPersistence {
    /// Allowed for this one operation only; nothing was recorded.
    Once,
    /// Allowed until the compositor exits; recorded in memory only.
    Session,
    /// Allowed durably; recorded on disk.
    Always,
    /// Refused until the compositor exits; recorded in memory only.
    DeniedSession,
}

/// One record in the mutation journal. Entries are append-only and ordered
/// by [`seq`](Self::seq), which is monotonic and never reused.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JournalEntry {
    /// Monotonic sequence number. Never reused; gaps may appear if entries
    /// have been evicted from the ring.
    pub seq: u64,
    /// Monotonic clock milliseconds at apply time. For ordering only; not
    /// wall-clock time.
    pub ts_mono_ms: u64,
    /// Who caused the command.
    pub origin: Origin,
    /// The exact command or Realm authority action decided.
    pub mutation: JournalMutation,
    /// What happened.
    pub effect: Effect,
}

/// A snapshot of journal entries returned by [`Journal::since`] or the IPC
/// `GetJournal` request. Carries the ring's bounds so the client detects
/// gaps.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JournalSnapshot {
    /// Entries with `seq > since`, oldest first.
    pub entries: Vec<JournalEntry>,
    /// The `seq` of the oldest entry currently in the ring. If the client's
    /// `since` is older than this, entries were evicted; re-query the full
    /// state instead of reasoning over a partial journal.
    pub oldest_seq: u64,
    /// The `seq` of the newest entry in the ring (matching the last entry in
    /// `entries`, or `oldest_seq - 1` if the ring is empty).
    pub latest_seq: u64,
}

/// An in-memory append-only ring buffer of journal entries (ADR-0033).
///
/// Bounded capacity; oldest entries are evicted when full. `seq` is
/// monotonic across evictions, so a subscriber that falls behind detects the
/// gap from [`Journal::oldest_seq`] and re-queries.
#[derive(Debug)]
pub struct Journal {
    entries: std::collections::VecDeque<JournalEntry>,
    next_seq: u64,
    capacity: usize,
}

/// The default ring capacity (ADR-0033).
pub const DEFAULT_CAPACITY: usize = 4096;

impl Journal {
    /// An empty journal with the given capacity.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Journal {
            entries: std::collections::VecDeque::with_capacity(cap),
            next_seq: 1,
            capacity: cap,
        }
    }

    /// An empty journal with [`DEFAULT_CAPACITY`].
    pub fn default_capacity() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }

    /// Append a new entry. Returns a reference to the stored entry. If the
    /// ring is full, the oldest entry is evicted.
    pub fn append(
        &mut self,
        ts_mono_ms: u64,
        origin: Origin,
        mutation: JournalMutation,
        effect: Effect,
    ) -> JournalEntry {
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        let entry = JournalEntry {
            seq,
            ts_mono_ms,
            origin,
            mutation,
            effect,
        };
        self.entries.push_back(entry.clone());
        entry
    }

    /// All entries with `seq > since`, oldest first, plus the ring's bounds.
    /// If `since` is 0, returns everything in the ring.
    pub fn since(&self, after: u64) -> JournalSnapshot {
        let entries: Vec<JournalEntry> = self
            .entries
            .iter()
            .filter(|e| e.seq > after)
            .cloned()
            .collect();
        JournalSnapshot {
            entries,
            oldest_seq: self.oldest_seq(),
            latest_seq: self.latest_seq(),
        }
    }

    /// The `seq` of the oldest entry in the ring, or `latest_seq + 1` if
    /// empty (so `oldest_seq > latest_seq` signals an empty ring).
    pub fn oldest_seq(&self) -> u64 {
        self.entries
            .front()
            .map(|e| e.seq)
            .unwrap_or_else(|| self.next_seq)
    }

    /// The `seq` of the newest entry, or `0` if the ring has never been
    /// written to.
    pub fn latest_seq(&self) -> u64 {
        self.entries.back().map(|e| e.seq).unwrap_or(0)
    }

    /// Number of entries currently in the ring.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ring is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Command;

    fn cmd(n: u64) -> Command {
        Command::Focus {
            id: aegis_core::window::WindowId(n),
        }
    }

    fn command(n: u64) -> JournalMutation {
        JournalMutation::Command { cmd: cmd(n) }
    }

    #[test]
    fn append_assigns_monotonic_seq() {
        let mut j = Journal::new(8);
        let e1 = j.append(0, Origin::Chrome, command(1), Effect::Applied);
        let s1 = e1.seq;
        let e2 = j.append(1, Origin::Keybinding, command(2), Effect::Applied);
        assert!(e2.seq > s1);
        assert_eq!(j.latest_seq(), e2.seq);
    }

    #[test]
    fn ring_evicts_oldest_when_full() {
        let mut j = Journal::new(3);
        j.append(0, Origin::Chrome, command(1), Effect::Applied);
        j.append(1, Origin::Chrome, command(2), Effect::Applied);
        j.append(2, Origin::Chrome, command(3), Effect::Applied);
        assert_eq!(j.len(), 3);
        // Fourth append evicts the first.
        j.append(3, Origin::Chrome, command(4), Effect::Applied);
        assert_eq!(j.len(), 3);
        let snap = j.since(0);
        assert_eq!(snap.entries.len(), 3);
        // The first entry (seq=1) was evicted; oldest is now seq=2.
        assert_eq!(snap.oldest_seq, 2);
    }

    #[test]
    fn since_filters_by_seq() {
        let mut j = Journal::new(8);
        j.append(0, Origin::Chrome, command(1), Effect::Applied);
        let mid = j.latest_seq();
        j.append(1, Origin::Ipc { conn_id: 7 }, command(2), Effect::Applied);
        j.append(2, Origin::Keybinding, command(3), Effect::Applied);
        let snap = j.since(mid);
        assert_eq!(snap.entries.len(), 2);
        assert!(snap.entries.iter().all(|e| e.seq > mid));
    }

    #[test]
    fn gap_detection_via_oldest_seq() {
        let mut j = Journal::new(2);
        j.append(0, Origin::Chrome, command(1), Effect::Applied);
        j.append(1, Origin::Chrome, command(2), Effect::Applied);
        j.append(2, Origin::Chrome, command(3), Effect::Applied);
        // Client asks for everything since seq=0, but seq=1 was evicted.
        let snap = j.since(0);
        assert_eq!(snap.oldest_seq, 2, "oldest in ring is seq=2");
        assert!(snap.entries.iter().all(|e| e.seq >= 2));
    }

    #[test]
    fn empty_journal_since_returns_empty_with_bounds() {
        let j = Journal::new(8);
        let snap = j.since(0);
        assert!(snap.entries.is_empty());
        assert_eq!(snap.latest_seq, 0);
    }

    #[test]
    fn entry_round_trips_through_serde() {
        let entry = JournalEntry {
            seq: 42,
            ts_mono_ms: 12345,
            origin: Origin::Ipc { conn_id: 7 },
            mutation: command(99),
            effect: Effect::Refused {
                reason: "out of scope".into(),
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: JournalEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn realm_action_round_trips_with_authority_revisions() {
        let mutation = JournalMutation::Realm {
            action: RealmAction::Create {
                label: "research".into(),
                capabilities: aegis_core::realm::SeatCapabilities::POINTER_KEYBOARD,
                output: Some(aegis_core::realm::VirtualOutput::DEFAULT_AGENT),
            },
            before_revision: 7,
            after_revision: 8,
        };
        let mut journal = Journal::new(4);
        let entry = journal.append(
            5,
            Origin::Ipc { conn_id: 9 },
            mutation.clone(),
            Effect::Applied,
        );
        assert_eq!(entry.mutation, mutation);
        let encoded = serde_json::to_string(&entry).unwrap();
        let decoded: JournalEntry = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, entry);
    }

    #[test]
    fn origin_tags_distinguish_sources() {
        let mut j = Journal::new(8);
        j.append(0, Origin::Chrome, command(1), Effect::Applied);
        j.append(1, Origin::Keybinding, command(2), Effect::Applied);
        j.append(2, Origin::Ipc { conn_id: 3 }, command(3), Effect::Applied);
        j.append(3, Origin::Internal, command(4), Effect::Applied);
        let snap = j.since(0);
        assert_eq!(snap.entries.len(), 4);
        assert!(matches!(snap.entries[0].origin, Origin::Chrome));
        assert!(matches!(snap.entries[1].origin, Origin::Keybinding));
        assert!(matches!(snap.entries[2].origin, Origin::Ipc { conn_id: 3 }));
        assert!(matches!(snap.entries[3].origin, Origin::Internal));
    }

    #[test]
    fn effect_variants_round_trip() {
        let effects = [
            Effect::Applied,
            Effect::Refused {
                reason: "test".into(),
            },
            Effect::NoOp,
        ];
        for e in &effects {
            let json = serde_json::to_string(e).unwrap();
            let back: Effect = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, e);
        }
    }

    #[test]
    fn agent_auth_mutation_round_trips() {
        let mutation = JournalMutation::AgentAuth {
            principal: "prin_ab12".into(),
            action: AgentAuthAction::Granted {
                op: OpClass::Close,
                persistence: GrantPersistence::Always,
            },
        };
        let mut journal = Journal::new(4);
        let entry = journal.append(
            3,
            Origin::Ipc { conn_id: 5 },
            mutation.clone(),
            Effect::Applied,
        );
        assert_eq!(entry.mutation, mutation);
        let encoded = serde_json::to_string(&entry).unwrap();
        let decoded: JournalEntry = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, entry);
    }
}
