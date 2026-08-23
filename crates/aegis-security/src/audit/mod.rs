//! Hash-chained event storage and bounded live audit projections.
//!
//! The event store is deliberately generic: domain crates own event schemas
//! and reducers, while this crate owns ordering, durability, file safety, and
//! hash-chain verification.

pub mod segments;

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use hmac::{Hmac, Mac};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Default live projection size. Durable storage, when configured, is not
/// truncated when this projection evicts its oldest entry.
pub const DEFAULT_CAPACITY: usize = 4_096;

/// Maximum serialized event envelope accepted from disk or for append.
pub const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;

/// Default hard ceiling for one durable audit stream. The store refuses an
/// append before crossing the ceiling; it never silently deletes history.
pub const DEFAULT_MAX_STORE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Default filesystem reserve protected from audit growth. The append path
/// checks available blocks before writing and fails closed while the reserve
/// still exists for the rest of the session and operating system.
pub const DEFAULT_MIN_FREE_BYTES: u64 = 512 * 1024 * 1024;

/// Maximum uncheckpointed byte tail under the default policy.
pub const DEFAULT_CHECKPOINT_INTERVAL_BYTES: u64 = 8 * 1024 * 1024;

/// Maximum uncheckpointed event tail under the default policy.
pub const DEFAULT_CHECKPOINT_INTERVAL_EVENTS: u64 = 4_096;

/// Default active-stream size that triggers sealing into a compressed
/// segment (ADR-0137).
pub const DEFAULT_SEGMENT_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Default number of sealed segments kept on disk; `0` keeps everything.
pub const DEFAULT_RETAIN_SEGMENTS: usize = 0;

const STORE_VERSION: u32 = 1;
const CHECKPOINT_VERSION: u32 = 1;
const CHECKPOINT_KEY_BYTES: usize = 32;
const MAX_CHECKPOINT_BYTES: u64 = 16 * 1024;
const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

type HmacSha256 = Hmac<Sha256>;

/// Operational bounds for one persistent audit stream.
///
/// These limits protect the host filesystem without weakening retention:
/// reaching either bound refuses the next event instead of deleting or
/// overwriting an older record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditStoreOptions {
    pub max_store_bytes: u64,
    pub min_free_bytes: u64,
    pub checkpoint_interval_bytes: u64,
    pub checkpoint_interval_events: u64,
    /// Active-stream size that triggers sealing into a compressed segment.
    pub segment_max_bytes: u64,
    /// Sealed segments kept on disk; `0` keeps everything. Pruning also
    /// requires every removed segment to carry an export acknowledgement.
    pub retain_segments: usize,
}

impl Default for AuditStoreOptions {
    fn default() -> Self {
        Self {
            max_store_bytes: DEFAULT_MAX_STORE_BYTES,
            min_free_bytes: DEFAULT_MIN_FREE_BYTES,
            checkpoint_interval_bytes: DEFAULT_CHECKPOINT_INTERVAL_BYTES,
            checkpoint_interval_events: DEFAULT_CHECKPOINT_INTERVAL_EVENTS,
            segment_max_bytes: DEFAULT_SEGMENT_MAX_BYTES,
            retain_segments: DEFAULT_RETAIN_SEGMENTS,
        }
    }
}

impl AuditStoreOptions {
    fn validate(self) -> Result<Self, AuditError> {
        if self.max_store_bytes < (MAX_EVENT_BYTES + 1) as u64 {
            return Err(AuditError::InvalidOptions(
                "max_store_bytes must fit one maximum-sized event",
            ));
        }
        if self.checkpoint_interval_bytes == 0 || self.checkpoint_interval_events == 0 {
            return Err(AuditError::InvalidOptions(
                "checkpoint intervals must be greater than zero",
            ));
        }
        if self.segment_max_bytes == 0 || self.segment_max_bytes > self.max_store_bytes {
            return Err(AuditError::InvalidOptions(
                "segment_max_bytes must be greater than zero and not exceed max_store_bytes",
            ));
        }
        Ok(self)
    }
}

/// Ordered event entry used by the live projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry<O, M, E> {
    pub seq: u64,
    pub ts_mono_ms: u64,
    pub origin: O,
    pub mutation: M,
    pub effect: E,
}

/// Bounded view returned to live audit consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditSnapshot<T> {
    pub entries: Vec<T>,
    pub oldest_seq: u64,
    pub latest_seq: u64,
}

/// In-memory append-only projection with detectable eviction gaps.
#[derive(Debug)]
pub struct AuditLog<T> {
    entries: VecDeque<T>,
    next_seq: u64,
    capacity: usize,
    store: Option<ChainedEventStore<T>>,
}

impl<T> AuditLog<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity.max(1)),
            next_seq: 1,
            capacity: capacity.max(1),
            store: None,
        }
    }

    pub fn default_capacity() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn oldest_seq(&self) -> u64
    where
        T: Sequence,
    {
        self.entries
            .front()
            .map(Sequence::sequence)
            .unwrap_or(self.next_seq)
    }

    pub fn latest_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn is_persistent(&self) -> bool {
        self.store.is_some()
    }

    /// Current durable stream length, when persistence is configured.
    pub fn persistent_bytes(&self) -> Option<u64> {
        self.store.as_ref().map(|store| store.file_len)
    }

    /// Whether a valid authenticated checkpoint bounded synchronous startup
    /// replay. A false value means this open performed the one-time complete
    /// scan needed to establish a checkpoint.
    pub fn checkpoint_accelerated(&self) -> bool {
        self.store
            .as_ref()
            .is_some_and(|store| store.checkpoint_accelerated)
    }

    /// Whether an authenticated checkpoint restored the live projection while
    /// a complete historical verification continues in the background.
    pub fn historical_verification_pending(&self) -> bool {
        self.store
            .as_ref()
            .is_some_and(|store| store.verification.is_pending())
    }

    /// Status summary across sealed segments and the active stream
    /// (ADR-0137). `None` when persistence is not configured.
    pub fn audit_status(&self) -> Option<super::audit::segments::AuditStatus> {
        self.store.as_ref().map(|store| store.audit_status())
    }

    /// Fast integrity check (presence, size, compressed digest) of every
    /// sealed segment against the authenticated manifest.
    pub fn verify_sealed_segments(&self) -> Result<usize, AuditError> {
        match self.store.as_ref() {
            Some(store) => store.verify_sealed_segments(),
            None => Ok(0),
        }
    }

    /// Sealed segment records, in seal order. Empty when persistence is not
    /// configured.
    pub fn sealed_segments(&self) -> &[super::audit::segments::SegmentRecord] {
        match self.store.as_ref() {
            Some(store) => store.sealed_segments(),
            None => &[],
        }
    }

    /// Directory holding sealed segments. `None` when persistence is not
    /// configured.
    pub fn segments_dir(&self) -> Option<std::path::PathBuf> {
        self.store.as_ref().map(|store| store.segments_dir())
    }

    /// Record an export acknowledgement for every sealed segment and persist
    /// it in the manifest (ADR-0137). Requires `&mut` because the manifest is
    /// rewritten atomically.
    pub fn mark_segments_exported(&mut self, destination: &str) -> Result<usize, AuditError>
    where
        T: Clone + Sequence + Serialize + DeserializeOwned + Send + 'static,
    {
        match self.store.as_mut() {
            Some(store) => store.mark_segments_exported(destination),
            None => Ok(0),
        }
    }

    /// Apply the retention policy explicitly (operator-initiated prune).
    /// Removed segments must carry export acknowledgements when
    /// `require_export` is true.
    pub fn prune_segments(
        &mut self,
        keep: usize,
        require_export: bool,
    ) -> Result<Vec<super::audit::segments::SegmentRecord>, AuditError>
    where
        T: Clone + Sequence + Serialize + DeserializeOwned + Send + 'static,
    {
        match self.store.as_mut() {
            Some(store) => store.prune_segments(keep, require_export),
            None => Ok(Vec::new()),
        }
    }

    pub fn since(&self, after: u64) -> AuditSnapshot<T>
    where
        T: Clone + Sequence,
    {
        AuditSnapshot {
            entries: self
                .entries
                .iter()
                .filter(|entry| entry.sequence() > after)
                .cloned()
                .collect(),
            oldest_seq: self.oldest_seq(),
            latest_seq: self.latest_seq(),
        }
    }

    /// Restore a verified durable prefix into the bounded live projection.
    pub fn restore(&mut self, entries: impl IntoIterator<Item = T>) -> Result<(), AuditError>
    where
        T: Sequence,
    {
        for entry in entries {
            let seq = entry.sequence();
            if seq != self.next_seq {
                return Err(AuditError::Sequence {
                    expected: self.next_seq,
                    actual: seq,
                });
            }
            self.push_memory(entry)?;
        }
        Ok(())
    }

    pub fn push_verified(&mut self, entry: T) -> Result<(), AuditError>
    where
        T: Clone + Sequence + Serialize + DeserializeOwned,
    {
        if let Some(store) = self.store.as_mut() {
            store.append(&entry)?;
        }
        self.push_memory(entry)
    }

    fn push_memory(&mut self, entry: T) -> Result<(), AuditError>
    where
        T: Sequence,
    {
        let seq = entry.sequence();
        if seq != self.next_seq {
            return Err(AuditError::Sequence {
                expected: self.next_seq,
                actual: seq,
            });
        }
        self.next_seq = self
            .next_seq
            .checked_add(1)
            .ok_or(AuditError::SequenceExhausted)?;
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
        Ok(())
    }
}

impl<T> AuditLog<T>
where
    T: Clone + Sequence + Serialize + DeserializeOwned + Send + 'static,
{
    /// Open a verified durable stream and rebuild its bounded live
    /// projection. Any unsafe file or invalid record aborts initialization.
    pub fn open_persistent(capacity: usize, path: impl Into<PathBuf>) -> Result<Self, AuditError> {
        Self::open_persistent_with_options(capacity, path, AuditStoreOptions::default())
    }

    /// Open a persistent stream with explicit disk and checkpoint bounds.
    pub fn open_persistent_with_options(
        capacity: usize,
        path: impl Into<PathBuf>,
        options: AuditStoreOptions,
    ) -> Result<Self, AuditError> {
        let (store, entries) =
            ChainedEventStore::open_with_options(path, capacity.max(1), options)?;
        let mut log = Self::new(capacity);
        log.entries = entries.into();
        log.next_seq = store.next_sequence();
        log.store = Some(store);
        Ok(log)
    }
}

impl<O, M, E> AuditLog<AuditEntry<O, M, E>> {
    pub fn prepare(
        &self,
        ts_mono_ms: u64,
        origin: O,
        mutation: M,
        effect: E,
    ) -> AuditEntry<O, M, E> {
        AuditEntry {
            seq: self.next_seq,
            ts_mono_ms,
            origin,
            mutation,
            effect,
        }
    }

    pub fn append(
        &mut self,
        ts_mono_ms: u64,
        origin: O,
        mutation: M,
        effect: E,
    ) -> AuditEntry<O, M, E>
    where
        O: Clone + Serialize + DeserializeOwned,
        M: Clone + Serialize + DeserializeOwned,
        E: Clone + Serialize + DeserializeOwned,
    {
        self.try_append(ts_mono_ms, origin, mutation, effect)
            .expect("durable audit append failed; refusing to continue without an event record")
    }

    pub fn try_append(
        &mut self,
        ts_mono_ms: u64,
        origin: O,
        mutation: M,
        effect: E,
    ) -> Result<AuditEntry<O, M, E>, AuditError>
    where
        O: Clone + Serialize + DeserializeOwned,
        M: Clone + Serialize + DeserializeOwned,
        E: Clone + Serialize + DeserializeOwned,
    {
        let entry = self.prepare(ts_mono_ms, origin, mutation, effect);
        self.push_verified(entry.clone())?;
        Ok(entry)
    }
}

/// Stable sequence access required by projections and durable stores.
pub trait Sequence {
    fn sequence(&self) -> u64;
}

impl<O, M, E> Sequence for AuditEntry<O, M, E> {
    fn sequence(&self) -> u64 {
        self.seq
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredEnvelope<T> {
    version: u32,
    sequence: u64,
    previous_hash: String,
    event: T,
    hash: String,
}

#[derive(Serialize)]
struct HashMaterial<'a, T> {
    version: u32,
    sequence: u64,
    previous_hash: &'a str,
    event: &'a T,
}

#[derive(Debug, Clone)]
struct RecordBoundary {
    offset: u64,
    sequence: u64,
    previous_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointBody {
    version: u32,
    log_len: u64,
    projection_offset: u64,
    projection_sequence: u64,
    projection_previous_hash: String,
    next_sequence: u64,
    tail_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedCheckpoint {
    checkpoint: CheckpointBody,
    mac: String,
}

#[derive(Debug)]
enum VerificationState {
    Pending,
    Verified,
    Failed(String),
}

#[derive(Clone)]
struct VerificationGate(Arc<(Mutex<VerificationState>, Condvar)>);

impl std::fmt::Debug for VerificationGate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerificationGate")
            .finish_non_exhaustive()
    }
}

impl VerificationGate {
    fn verified() -> Self {
        Self(Arc::new((
            Mutex::new(VerificationState::Verified),
            Condvar::new(),
        )))
    }

    fn pending() -> Self {
        Self(Arc::new((
            Mutex::new(VerificationState::Pending),
            Condvar::new(),
        )))
    }

    fn complete(&self, result: Result<(), AuditError>) {
        let (state, ready) = &*self.0;
        let mut state = state.lock().unwrap();
        *state = match result {
            Ok(()) => VerificationState::Verified,
            Err(error) => VerificationState::Failed(error.to_string()),
        };
        ready.notify_all();
    }

    fn wait(&self) -> Result<(), AuditError> {
        let (state, ready) = &*self.0;
        let mut state = state.lock().unwrap();
        while matches!(*state, VerificationState::Pending) {
            state = ready.wait(state).unwrap();
        }
        match &*state {
            VerificationState::Verified => Ok(()),
            VerificationState::Failed(message) => {
                Err(AuditError::BackgroundVerification(message.clone()))
            }
            VerificationState::Pending => unreachable!("verification wait exited while pending"),
        }
    }

    fn is_pending(&self) -> bool {
        let (state, _) = &*self.0;
        matches!(*state.lock().unwrap(), VerificationState::Pending)
    }
}

#[derive(Debug)]
struct Replay<T> {
    entries: VecDeque<T>,
    boundaries: VecDeque<RecordBoundary>,
    next_sequence: u64,
    tail_hash: String,
    file_len: u64,
}

struct ReplayStart {
    offset: u64,
    sequence: u64,
    previous_hash: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplayEnd {
    EndOfFile,
    Checkpoint,
}

/// Owner-only append-only event store with a SHA-256 hash chain and an
/// authenticated bounded-replay checkpoint.
pub struct ChainedEventStore<T> {
    path: PathBuf,
    parent: PathBuf,
    writer: BufWriter<File>,
    next_sequence: u64,
    tail_hash: String,
    file_len: u64,
    projection_capacity: usize,
    boundaries: VecDeque<RecordBoundary>,
    checkpoint_path: PathBuf,
    checkpoint_key: Zeroizing<[u8; CHECKPOINT_KEY_BYTES]>,
    checkpoint_accelerated: bool,
    events_since_checkpoint: u64,
    bytes_since_checkpoint: u64,
    verification: VerificationGate,
    verification_cancel: Arc<AtomicBool>,
    verification_worker: Option<std::thread::JoinHandle<()>>,
    manifest: segments::SegmentManifest,
    /// Global sequence of this epoch's first event (1 unless a seal happened).
    epoch_first_sequence: u64,
    /// Chain hash this epoch extends (genesis unless a seal happened).
    epoch_previous_hash: String,
    options: AuditStoreOptions,
    _event: std::marker::PhantomData<T>,
}

impl<T> std::fmt::Debug for ChainedEventStore<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChainedEventStore")
            .field("path", &self.path)
            .field("next_sequence", &self.next_sequence)
            .field("tail_hash", &self.tail_hash)
            .field("file_len", &self.file_len)
            .field("checkpoint_accelerated", &self.checkpoint_accelerated)
            .finish_non_exhaustive()
    }
}

impl<T> ChainedEventStore<T>
where
    T: Clone + Sequence + Serialize + DeserializeOwned,
{
    /// Open an existing store with the default operational bounds.
    ///
    /// The returned projection is bounded to [`DEFAULT_CAPACITY`].
    pub fn open(path: impl Into<PathBuf>) -> Result<(Self, Vec<T>), AuditError>
    where
        T: Send + 'static,
    {
        Self::open_with_options(path, DEFAULT_CAPACITY, AuditStoreOptions::default())
    }

    /// Open and verify an existing store, or create a new owner-only store.
    /// The returned events contain only the newest `projection_capacity`
    /// hash-verified records.
    ///
    /// A successful open holds the store's exclusive advisory lock for the
    /// store's lifetime. A second live opener fails fast with
    /// [`AuditError::Locked`] instead of interleaving appends from stale
    /// sequence state, which would corrupt the chain seen by the next open.
    pub fn open_with_options(
        path: impl Into<PathBuf>,
        projection_capacity: usize,
        options: AuditStoreOptions,
    ) -> Result<(Self, Vec<T>), AuditError>
    where
        T: Send + 'static,
    {
        let options = options.validate()?;
        let projection_capacity = projection_capacity.max(1);
        let path = path.into();
        let parent = path.parent().ok_or(AuditError::NoParent)?.to_path_buf();
        std::fs::create_dir_all(&parent).map_err(|source| AuditError::Io {
            path: parent.clone(),
            source,
        })?;
        restrict_directory(&parent)?;

        let (file, created) = open_store_file(&path)?;
        validate_private_file(&path, &file)?;
        lock_store_file(&path, &file)?;
        if created {
            // `sync_data` on later appends cannot make the directory entry
            // that first named this file durable. Persist that entry before
            // the store is accepted as production audit storage.
            sync_directory(&parent)?;
        }

        let replay_file = file.try_clone().map_err(|source| AuditError::Io {
            path: path.clone(),
            source,
        })?;
        let checkpoint_path = sidecar_path(&path, "checkpoint")?;
        let checkpoint_key_path = sidecar_path(&path, "key")?;
        let checkpoint_exists = path_exists(&checkpoint_path)?;
        let checkpoint_key =
            load_or_create_checkpoint_key(&parent, &checkpoint_key_path, checkpoint_exists)?;
        let checkpoint = if checkpoint_exists {
            Some(read_checkpoint(
                &checkpoint_path,
                checkpoint_key.as_ref(),
                projection_capacity,
                file.metadata()
                    .map_err(|source| AuditError::Io {
                        path: path.clone(),
                        source,
                    })?
                    .len(),
            )?)
        } else {
            None
        };

        let checkpoint_accelerated = checkpoint.is_some();
        let mut replay = match checkpoint.as_ref() {
            Some(checkpoint) => replay_from(
                &path,
                replay_file,
                ReplayStart {
                    offset: checkpoint.projection_offset,
                    sequence: checkpoint.projection_sequence,
                    previous_hash: checkpoint.projection_previous_hash.clone(),
                },
                projection_capacity,
                Some(checkpoint),
                None,
                ReplayEnd::EndOfFile,
            )?,
            None => replay_from(
                &path,
                replay_file,
                ReplayStart {
                    offset: 0,
                    sequence: 1,
                    previous_hash: GENESIS_HASH.to_owned(),
                },
                projection_capacity,
                None,
                None,
                ReplayEnd::EndOfFile,
            )?,
        };

        let manifest = segments::SegmentManifest::open(&path, checkpoint_key.as_ref())?;
        let manifest_has_chain = manifest.tail_hash() != GENESIS_HASH;
        if replay.file_len == 0 && manifest_has_chain {
            // A previous epoch sealed this stream. The fresh active file
            // continues the chain from the manifest anchor; the genesis-hash
            // replay above produced an empty, unanchored result that must be
            // reinterpreted rather than trusted.
            if manifest.next_sequence() == 0 {
                return Err(AuditError::SegmentState(
                    "manifest continues a chain from sequence zero",
                ));
            }
            replay.next_sequence = manifest.next_sequence();
            replay.tail_hash = manifest.tail_hash().to_owned();
        } else if manifest_has_chain
            && (replay.next_sequence != manifest.next_sequence()
                || replay.tail_hash != manifest.tail_hash())
            && replay.entries.is_empty()
        {
            return Err(AuditError::SegmentState(
                "active stream does not match the segment manifest",
            ));
        }
        let epoch_previous_hash = if manifest_has_chain && replay.file_len == 0 {
            manifest.tail_hash().to_owned()
        } else {
            GENESIS_HASH.to_owned()
        };
        let epoch_first_sequence = if manifest_has_chain && replay.file_len == 0 {
            manifest.next_sequence()
        } else {
            1
        };

        if replay.file_len > options.max_store_bytes {
            return Err(AuditError::QuotaExceeded {
                path,
                max_bytes: options.max_store_bytes,
                attempted_bytes: replay.file_len,
            });
        }

        let (verification, verification_cancel, verification_worker) = match checkpoint.as_ref() {
            Some(checkpoint) if checkpoint.projection_offset > 0 => {
                let verify_file = file.try_clone().map_err(|source| AuditError::Io {
                    path: path.clone(),
                    source,
                })?;
                let gate = VerificationGate::pending();
                let worker_gate = gate.clone();
                let cancel = Arc::new(AtomicBool::new(false));
                let worker_cancel = Arc::clone(&cancel);
                let worker_path = path.clone();
                let checkpoint = checkpoint.clone();
                let worker = std::thread::Builder::new()
                    .name("aegis-audit-verify".into())
                    .spawn(move || {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            verify_prefix::<T>(
                                &worker_path,
                                verify_file,
                                checkpoint.projection_offset,
                                checkpoint.projection_sequence,
                                &checkpoint.projection_previous_hash,
                                &worker_cancel,
                            )
                        }))
                        .unwrap_or(Err(AuditError::VerificationWorkerPanicked));
                        if !worker_cancel.load(Ordering::Acquire) {
                            worker_gate.complete(result);
                        }
                    })
                    .map_err(|source| AuditError::Io {
                        path: path.clone(),
                        source,
                    })?;
                (gate, cancel, Some(worker))
            }
            _ => (
                VerificationGate::verified(),
                Arc::new(AtomicBool::new(false)),
                None,
            ),
        };

        let checkpoint_next_sequence = checkpoint.as_ref().map_or(1, |value| value.next_sequence);
        let checkpoint_log_len = checkpoint.as_ref().map_or(0, |value| value.log_len);
        let events_since_checkpoint = replay
            .next_sequence
            .saturating_sub(checkpoint_next_sequence);
        let bytes_since_checkpoint = replay.file_len.saturating_sub(checkpoint_log_len);
        let entries = replay.entries.into_iter().collect();
        let mut store = Self {
            path,
            parent,
            writer: BufWriter::new(file),
            next_sequence: replay.next_sequence,
            tail_hash: replay.tail_hash,
            file_len: replay.file_len,
            projection_capacity,
            boundaries: replay.boundaries,
            checkpoint_path,
            checkpoint_key,
            checkpoint_accelerated,
            events_since_checkpoint,
            bytes_since_checkpoint,
            verification,
            verification_cancel,
            verification_worker,
            manifest,
            epoch_first_sequence,
            epoch_previous_hash,
            options,
            _event: std::marker::PhantomData,
        };
        if !checkpoint_accelerated {
            store.persist_checkpoint()?;
        }
        Ok((store, entries))
    }

    /// Append and synchronously persist one event. Sequence and chain state
    /// advance only after the bytes reach the kernel's durable-file boundary.
    pub fn append(&mut self, event: &T) -> Result<(), AuditError> {
        // A checkpoint authenticates the old prefix so startup can restore a
        // bounded tail immediately, but no new authority history may extend
        // that prefix until the background complete-chain verification has
        // succeeded.
        self.verification.wait()?;
        let actual = event.sequence();
        if actual != self.next_sequence {
            return Err(AuditError::Sequence {
                expected: self.next_sequence,
                actual,
            });
        }
        let hash = event_hash(self.next_sequence, &self.tail_hash, event)?;
        let envelope = StoredEnvelope {
            version: STORE_VERSION,
            sequence: self.next_sequence,
            previous_hash: self.tail_hash.clone(),
            event: event.clone(),
            hash: hash.clone(),
        };
        let mut bytes = serde_json::to_vec(&envelope).map_err(AuditError::Encode)?;
        if bytes.len() > MAX_EVENT_BYTES {
            return Err(AuditError::Oversized(bytes.len()));
        }
        bytes.push(b'\n');
        let attempted_bytes = self.file_len.saturating_add(bytes.len() as u64);
        if attempted_bytes > self.options.max_store_bytes {
            return Err(AuditError::QuotaExceeded {
                path: self.path.clone(),
                max_bytes: self.options.max_store_bytes,
                attempted_bytes,
            });
        }
        let available = available_bytes(self.writer.get_ref(), &self.path)?;
        let required = self
            .options
            .min_free_bytes
            .saturating_add(bytes.len() as u64);
        if available < required {
            return Err(AuditError::LowSpace {
                path: self.path.clone(),
                available_bytes: available,
                required_bytes: required,
            });
        }
        let boundary = RecordBoundary {
            offset: self.file_len,
            sequence: self.next_sequence,
            previous_hash: self.tail_hash.clone(),
        };
        self.writer
            .write_all(&bytes)
            .and_then(|_| self.writer.flush())
            .map_err(|source| AuditError::Io {
                path: self.path.clone(),
                source,
            })?;
        self.writer
            .get_ref()
            .sync_data()
            .map_err(|source| AuditError::Io {
                path: self.path.clone(),
                source,
            })?;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(AuditError::SequenceExhausted)?;
        self.tail_hash = hash;
        self.file_len = attempted_bytes;
        if self.boundaries.len() == self.projection_capacity {
            self.boundaries.pop_front();
        }
        self.boundaries.push_back(boundary);
        self.events_since_checkpoint = self.events_since_checkpoint.saturating_add(1);
        self.bytes_since_checkpoint = self
            .bytes_since_checkpoint
            .saturating_add(bytes.len() as u64);
        if self.events_since_checkpoint >= self.options.checkpoint_interval_events
            || self.bytes_since_checkpoint >= self.options.checkpoint_interval_bytes
        {
            self.persist_checkpoint()?;
        }
        if self.file_len >= self.options.segment_max_bytes {
            self.seal_active()?;
        }
        Ok(())
    }

    /// Seal the active stream into a compressed segment and continue the
    /// chain in a fresh active file (ADR-0137). The active stream is
    /// verified end to end during compression, so a corrupt stream is never
    /// sealed. Retention (`retain_segments`) is applied after sealing and
    /// requires export acknowledgements before any segment is deleted.
    pub fn seal_active(&mut self) -> Result<Option<segments::SegmentRecord>, AuditError> {
        if self.file_len == 0 {
            return Ok(None);
        }
        // Flush any buffered checkpoint state so the on-disk stream matches
        // `file_len` exactly before compressing it.
        self.writer
            .get_ref()
            .sync_data()
            .map_err(|source| AuditError::Io {
                path: self.path.clone(),
                source,
            })?;
        let first = self.epoch_first_sequence;
        let previous = self.epoch_previous_hash.clone();
        let sealed = self.manifest.seal_active(
            &self.path,
            first,
            &previous,
            self.next_sequence,
            &self.tail_hash,
            self.file_len,
        )?;
        // The compressed copy is durable and manifest-recorded; truncating the
        // active stream starts the next epoch anchored at the sealed tail.
        self.writer
            .get_ref()
            .set_len(0)
            .and_then(|_| self.writer.get_ref().sync_data())
            .map_err(|source| AuditError::Io {
                path: self.path.clone(),
                source,
            })?;
        self.writer
            .get_ref()
            .seek(SeekFrom::Start(0))
            .map_err(|source| AuditError::Io {
                path: self.path.clone(),
                source,
            })?;
        self.epoch_first_sequence = self.next_sequence;
        self.epoch_previous_hash = self.tail_hash.clone();
        self.file_len = 0;
        self.boundaries.clear();
        self.events_since_checkpoint = 0;
        self.bytes_since_checkpoint = 0;
        self.persist_checkpoint()?;
        if self.options.retain_segments > 0 {
            // Retention is a policy, not a durability requirement: a failed
            // prune (for example, a segment that was never exported) must not
            // fail the durable append that triggered the seal. The manifest
            // keeps every segment until an operator acknowledges an export
            // and prunes explicitly.
            let _ = self.manifest.prune(self.options.retain_segments, true);
        }
        Ok(Some(sealed))
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn tail_hash(&self) -> &str {
        &self.tail_hash
    }

    pub fn len_bytes(&self) -> u64 {
        self.file_len
    }

    pub fn checkpoint_accelerated(&self) -> bool {
        self.checkpoint_accelerated
    }

    fn persist_checkpoint(&mut self) -> Result<(), AuditError> {
        let (projection_offset, projection_sequence, projection_previous_hash) = self
            .boundaries
            .front()
            .map(|boundary| {
                (
                    boundary.offset,
                    boundary.sequence,
                    boundary.previous_hash.clone(),
                )
            })
            .unwrap_or((self.file_len, self.next_sequence, self.tail_hash.clone()));
        let checkpoint = CheckpointBody {
            version: CHECKPOINT_VERSION,
            log_len: self.file_len,
            projection_offset,
            projection_sequence,
            projection_previous_hash,
            next_sequence: self.next_sequence,
            tail_hash: self.tail_hash.clone(),
        };
        write_checkpoint(
            &self.parent,
            &self.checkpoint_path,
            self.checkpoint_key.as_ref(),
            &checkpoint,
        )?;
        self.events_since_checkpoint = 0;
        self.bytes_since_checkpoint = 0;
        Ok(())
    }
}

impl<T> Drop for ChainedEventStore<T> {
    fn drop(&mut self) {
        self.verification_cancel.store(true, Ordering::Release);
        if let Some(worker) = self.verification_worker.take() {
            let _ = worker.join();
        }
    }
}

/// Segment and retention operations that do not depend on the event type
/// (ADR-0137). Split out so `AuditLog<T>` can expose them without the
/// serialize/deserialize bounds the append path needs.
impl<T> ChainedEventStore<T> {
    /// Status summary across sealed segments and the active stream.
    pub fn audit_status(&self) -> segments::AuditStatus {
        self.manifest.status(self.file_len)
    }

    /// Fast integrity check of every sealed segment against the manifest.
    pub fn verify_sealed_segments(&self) -> Result<usize, AuditError> {
        self.manifest.verify_fast()
    }

    /// Record an export acknowledgement for every sealed segment and persist
    /// it in the manifest. Returns the acknowledged segment count.
    pub fn mark_segments_exported(&mut self, destination: &str) -> Result<usize, AuditError> {
        self.manifest.mark_exported(destination)
    }

    /// Apply the retention policy explicitly (operator-initiated prune).
    /// Removed segments must carry export acknowledgements when
    /// `require_export` is true.
    pub fn prune_segments(
        &mut self,
        keep: usize,
        require_export: bool,
    ) -> Result<Vec<segments::SegmentRecord>, AuditError> {
        self.manifest.prune(keep, require_export)
    }

    /// Sealed segment records, in seal order.
    pub fn sealed_segments(&self) -> &[segments::SegmentRecord] {
        self.manifest.segments()
    }

    /// Directory holding sealed segments (diagnostics and exports).
    pub fn segments_dir(&self) -> PathBuf {
        self.parent.join(segments::SEGMENTS_DIR)
    }
}

/// Take the store's exclusive advisory lock without blocking. The lock is
/// bound to the open file description, so the store holds it through `writer`
/// until drop or process exit releases it. Two live writers on one store
/// replay the same tail and then interleave appends whose sequences no longer
/// match either writer, corrupting the chain for the next open; the loser of
/// this lock must fail fast instead.
fn lock_store_file(path: &Path, file: &File) -> Result<(), AuditError> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(());
    }
    let source = std::io::Error::last_os_error();
    if source.kind() == std::io::ErrorKind::WouldBlock {
        return Err(AuditError::Locked(path.to_path_buf()));
    }
    Err(AuditError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn open_store_file(path: &Path) -> Result<(File, bool), AuditError> {
    let existing = || {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .append(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        options.open(path)
    };
    match existing() {
        Ok(file) => Ok((file, false)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .append(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
            match options.open(path) {
                Ok(file) => Ok((file, true)),
                // Another opener may have won the create race. Reopen it
                // without creation and subject it to the normal metadata
                // checks instead of replacing it.
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => existing()
                    .map(|file| (file, false))
                    .map_err(|source| AuditError::Io {
                        path: path.to_path_buf(),
                        source,
                    }),
                Err(source) => Err(AuditError::Io {
                    path: path.to_path_buf(),
                    source,
                }),
            }
        }
        Err(source) => Err(AuditError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn sync_directory(path: &Path) -> Result<(), AuditError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY);
    let directory = options.open(path).map_err(|source| AuditError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    directory.sync_all().map_err(|source| AuditError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn replay_from<T>(
    path: &Path,
    mut file: File,
    start: ReplayStart,
    capacity: usize,
    checkpoint: Option<&CheckpointBody>,
    cancel: Option<&AtomicBool>,
    end: ReplayEnd,
) -> Result<Replay<T>, AuditError>
where
    T: Sequence + Serialize + DeserializeOwned,
{
    file.seek(SeekFrom::Start(start.offset))
        .map_err(|source| AuditError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut reader = BufReader::new(file);
    let mut entries = VecDeque::with_capacity(capacity);
    let mut boundaries = VecDeque::with_capacity(capacity);
    let mut expected_sequence = start.sequence;
    let mut previous_hash = start.previous_hash;
    let mut offset = start.offset;
    let mut checkpoint_verified = checkpoint.is_none();
    let mut line = Vec::new();
    if let Some(checkpoint) = checkpoint
        && offset == checkpoint.log_len
    {
        validate_replay_checkpoint(checkpoint, expected_sequence, &previous_hash)?;
        checkpoint_verified = true;
    }
    while end != ReplayEnd::Checkpoint || !checkpoint_verified {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
            return Err(AuditError::VerificationCancelled);
        }
        let boundary = RecordBoundary {
            offset,
            sequence: expected_sequence,
            previous_hash: previous_hash.clone(),
        };
        line.clear();
        let read = reader
            .by_ref()
            .take((MAX_EVENT_BYTES + 2) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|source| AuditError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        offset = offset
            .checked_add(read as u64)
            .ok_or(AuditError::SequenceExhausted)?;
        if line.len() > MAX_EVENT_BYTES + 1 {
            return Err(AuditError::Oversized(line.len()));
        }
        if line.last() != Some(&b'\n') {
            return Err(AuditError::IncompleteRecord(expected_sequence));
        }
        line.pop();
        let envelope: StoredEnvelope<T> =
            serde_json::from_slice(&line).map_err(AuditError::Decode)?;
        if envelope.version != STORE_VERSION {
            return Err(AuditError::Version(envelope.version));
        }
        if envelope.sequence != expected_sequence || envelope.event.sequence() != expected_sequence
        {
            return Err(AuditError::Sequence {
                expected: expected_sequence,
                actual: envelope.sequence,
            });
        }
        if envelope.previous_hash != previous_hash {
            return Err(AuditError::PreviousHash(expected_sequence));
        }
        let expected_hash = event_hash(expected_sequence, &previous_hash, &envelope.event)?;
        if !constant_time_eq(envelope.hash.as_bytes(), expected_hash.as_bytes()) {
            return Err(AuditError::Hash(expected_sequence));
        }
        previous_hash = expected_hash;
        expected_sequence = expected_sequence
            .checked_add(1)
            .ok_or(AuditError::SequenceExhausted)?;
        if entries.len() == capacity {
            entries.pop_front();
            boundaries.pop_front();
        }
        entries.push_back(envelope.event);
        boundaries.push_back(boundary);
        if let Some(checkpoint) = checkpoint {
            if offset == checkpoint.log_len {
                validate_replay_checkpoint(checkpoint, expected_sequence, &previous_hash)?;
                checkpoint_verified = true;
            } else if !checkpoint_verified && offset > checkpoint.log_len {
                return Err(AuditError::CheckpointState(
                    "checkpoint offset is not an event boundary",
                ));
            }
        }
    }
    if !checkpoint_verified {
        return Err(AuditError::CheckpointState(
            "checkpoint offset was not reached",
        ));
    }
    Ok(Replay {
        entries,
        boundaries,
        next_sequence: expected_sequence,
        tail_hash: previous_hash,
        file_len: offset,
    })
}

fn validate_replay_checkpoint(
    checkpoint: &CheckpointBody,
    next_sequence: u64,
    tail_hash: &str,
) -> Result<(), AuditError> {
    if next_sequence != checkpoint.next_sequence || tail_hash != checkpoint.tail_hash {
        return Err(AuditError::CheckpointState(
            "checkpoint does not match the durable event chain",
        ));
    }
    Ok(())
}

fn verify_prefix<T>(
    path: &Path,
    file: File,
    prefix_len: u64,
    expected_next_sequence: u64,
    expected_tail_hash: &str,
    cancel: &AtomicBool,
) -> Result<(), AuditError>
where
    T: Sequence + Serialize + DeserializeOwned,
{
    let replay = replay_from::<T>(
        path,
        file,
        ReplayStart {
            offset: 0,
            sequence: 1,
            previous_hash: GENESIS_HASH.to_owned(),
        },
        1,
        Some(&CheckpointBody {
            version: CHECKPOINT_VERSION,
            log_len: prefix_len,
            projection_offset: 0,
            projection_sequence: 1,
            projection_previous_hash: GENESIS_HASH.to_owned(),
            next_sequence: expected_next_sequence,
            tail_hash: expected_tail_hash.to_owned(),
        }),
        Some(cancel),
        ReplayEnd::Checkpoint,
    )?;
    if replay.file_len < prefix_len {
        return Err(AuditError::CheckpointState(
            "durable stream ended before the authenticated prefix",
        ));
    }
    Ok(())
}

fn sidecar_path(path: &Path, suffix: &str) -> Result<PathBuf, AuditError> {
    let name = path
        .file_name()
        .ok_or(AuditError::NoParent)?
        .to_string_lossy();
    Ok(path.with_file_name(format!("{name}.{suffix}")))
}

fn path_exists(path: &Path) -> Result<bool, AuditError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(AuditError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn load_or_create_checkpoint_key(
    parent: &Path,
    path: &Path,
    checkpoint_exists: bool,
) -> Result<Zeroizing<[u8; CHECKPOINT_KEY_BYTES]>, AuditError> {
    match open_private_file(path) {
        Ok(mut file) => {
            let mut key = Zeroizing::new([0u8; CHECKPOINT_KEY_BYTES]);
            file.read_exact(key.as_mut())
                .map_err(|source| AuditError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
            let mut trailing = [0u8; 1];
            if file.read(&mut trailing).map_err(|source| AuditError::Io {
                path: path.to_path_buf(),
                source,
            })? != 0
            {
                return Err(AuditError::CheckpointState(
                    "checkpoint key has an invalid length",
                ));
            }
            Ok(key)
        }
        Err(AuditError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound && !checkpoint_exists =>
        {
            let mut key = Zeroizing::new([0u8; CHECKPOINT_KEY_BYTES]);
            getrandom::fill(key.as_mut())
                .map_err(|source| AuditError::Entropy(source.to_string()))?;
            let mut options = OpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
            let mut file = options.open(path).map_err(|source| AuditError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            file.write_all(key.as_ref())
                .and_then(|_| file.flush())
                .and_then(|_| file.sync_data())
                .map_err(|source| AuditError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
            sync_directory(parent)?;
            Ok(key)
        }
        Err(AuditError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound && checkpoint_exists =>
        {
            Err(AuditError::CheckpointState(
                "authenticated checkpoint exists but its key is missing",
            ))
        }
        Err(error) => Err(error),
    }
}

fn open_private_file(path: &Path) -> Result<File, AuditError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let file = options.open(path).map_err(|source| AuditError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    validate_private_file(path, &file)?;
    Ok(file)
}

fn read_checkpoint(
    path: &Path,
    key: &[u8],
    projection_capacity: usize,
    log_len: u64,
) -> Result<CheckpointBody, AuditError> {
    let file = open_private_file(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_CHECKPOINT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| AuditError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > MAX_CHECKPOINT_BYTES {
        return Err(AuditError::CheckpointState("checkpoint is oversized"));
    }
    let signed: SignedCheckpoint =
        serde_json::from_slice(&bytes).map_err(AuditError::CheckpointDecode)?;
    verify_checkpoint_mac(key, &signed.checkpoint, &signed.mac)?;
    validate_checkpoint(&signed.checkpoint, projection_capacity, log_len)?;
    Ok(signed.checkpoint)
}

fn validate_checkpoint(
    checkpoint: &CheckpointBody,
    projection_capacity: usize,
    actual_log_len: u64,
) -> Result<(), AuditError> {
    if checkpoint.version != CHECKPOINT_VERSION {
        return Err(AuditError::CheckpointState(
            "unsupported checkpoint version",
        ));
    }
    if checkpoint.projection_offset > checkpoint.log_len
        || checkpoint.log_len > actual_log_len
        || checkpoint.projection_sequence == 0
        || checkpoint.next_sequence < checkpoint.projection_sequence
        || checkpoint
            .next_sequence
            .saturating_sub(checkpoint.projection_sequence)
            > projection_capacity as u64
        || !valid_hash(&checkpoint.projection_previous_hash)
        || !valid_hash(&checkpoint.tail_hash)
    {
        return Err(AuditError::CheckpointState(
            "checkpoint fields are inconsistent",
        ));
    }
    Ok(())
}

fn write_checkpoint(
    parent: &Path,
    path: &Path,
    key: &[u8],
    checkpoint: &CheckpointBody,
) -> Result<(), AuditError> {
    let signed = SignedCheckpoint {
        checkpoint: checkpoint.clone(),
        mac: checkpoint_mac(key, checkpoint)?,
    };
    let mut bytes = serde_json::to_vec(&signed).map_err(AuditError::Encode)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_CHECKPOINT_BYTES {
        return Err(AuditError::CheckpointState("checkpoint is oversized"));
    }

    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(|source| AuditError::Entropy(source.to_string()))?;
    let name = path
        .file_name()
        .ok_or(AuditError::NoParent)?
        .to_string_lossy();
    let temporary = path.with_file_name(format!(".{name}.tmp-{}", hex(&random)));
    let result = (|| {
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let mut file = options.open(&temporary).map_err(|source| AuditError::Io {
            path: temporary.clone(),
            source,
        })?;
        file.write_all(&bytes)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_data())
            .map_err(|source| AuditError::Io {
                path: temporary.clone(),
                source,
            })?;
        std::fs::rename(&temporary, path).map_err(|source| AuditError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn checkpoint_mac(key: &[u8], checkpoint: &CheckpointBody) -> Result<String, AuditError> {
    let bytes = serde_json::to_vec(checkpoint).map_err(AuditError::Encode)?;
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| AuditError::CheckpointState("checkpoint key is invalid"))?;
    mac.update(&bytes);
    Ok(hex(&mac.finalize().into_bytes()))
}

fn verify_checkpoint_mac(
    key: &[u8],
    checkpoint: &CheckpointBody,
    encoded: &str,
) -> Result<(), AuditError> {
    let bytes = serde_json::to_vec(checkpoint).map_err(AuditError::Encode)?;
    let expected =
        decode_hash(encoded).ok_or(AuditError::CheckpointState("checkpoint MAC is malformed"))?;
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| AuditError::CheckpointState("checkpoint key is invalid"))?;
    mac.update(&bytes);
    mac.verify_slice(&expected)
        .map_err(|_| AuditError::CheckpointAuthentication)
}

fn valid_hash(value: &str) -> bool {
    decode_hash(value).is_some()
}

fn decode_hash(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut result = [0u8; 32];
    // The 64-byte length guard above makes the remainder slice empty, so
    // `as_chunks::<2>()` (clippy 1.98's chunks_exact_to_as_chunks) covers
    // every byte.
    let (pairs, _) = value.as_bytes().as_chunks::<2>();
    for (index, pair) in pairs.iter().enumerate() {
        let high = decode_hex_nibble(pair[0])?;
        let low = decode_hex_nibble(pair[1])?;
        result[index] = high << 4 | low;
    }
    Some(result)
}

fn decode_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn available_bytes(file: &File, path: &Path) -> Result<u64, AuditError> {
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::fstatvfs(file.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(AuditError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        });
    }
    let stats = unsafe { stats.assume_init() };
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

fn event_hash<T: Serialize>(
    sequence: u64,
    previous_hash: &str,
    event: &T,
) -> Result<String, AuditError> {
    let bytes = serde_json::to_vec(&HashMaterial {
        version: STORE_VERSION,
        sequence,
        previous_hash,
        event,
    })
    .map_err(AuditError::Encode)?;
    let digest = Sha256::digest(bytes);
    Ok(hex(&digest))
}

fn hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    use std::fmt::Write as _;
    for byte in bytes {
        write!(&mut result, "{byte:02x}").expect("writing to String cannot fail");
    }
    result
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn restrict_directory(path: &Path) -> Result<(), AuditError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|source| AuditError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(AuditError::UnsafePath(path.to_path_buf()));
    }
    let mut permissions = metadata.permissions();
    if permissions.mode() & 0o077 != 0 {
        permissions.set_mode(permissions.mode() & !0o077);
        std::fs::set_permissions(path, permissions).map_err(|source| AuditError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn validate_private_file(path: &Path, file: &File) -> Result<(), AuditError> {
    let metadata = file.metadata().map_err(|source| AuditError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err(AuditError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("audit path has no parent directory")]
    NoParent,
    #[error("invalid audit store options: {0}")]
    InvalidOptions(&'static str),
    #[error("unsafe audit path ownership, permissions, or file type: {0}")]
    UnsafePath(PathBuf),
    #[error("audit I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("audit event encoding failed: {0}")]
    Encode(serde_json::Error),
    #[error("audit event decoding failed: {0}")]
    Decode(serde_json::Error),
    #[error("unsupported audit store version {0}")]
    Version(u32),
    #[error("audit store is locked by another live instance: {0}")]
    Locked(PathBuf),
    #[error("audit checkpoint authentication failed")]
    CheckpointAuthentication,
    #[error("audit checkpoint decoding failed: {0}")]
    CheckpointDecode(serde_json::Error),
    #[error("invalid audit checkpoint: {0}")]
    CheckpointState(&'static str),
    #[error("audit history verification failed: {0}")]
    BackgroundVerification(String),
    #[error("audit history verification was cancelled")]
    VerificationCancelled,
    #[error("audit history verification worker panicked")]
    VerificationWorkerPanicked,
    #[error("invalid audit segment state: {0}")]
    SegmentState(&'static str),
    #[error("sealed audit segment {index} failed integrity verification")]
    SegmentVerification { index: u64 },
    #[error("audit checkpoint entropy failed: {0}")]
    Entropy(String),
    #[error(
        "audit quota exceeded for {path}: maximum {max_bytes} bytes, attempted {attempted_bytes} bytes"
    )]
    QuotaExceeded {
        path: PathBuf,
        max_bytes: u64,
        attempted_bytes: u64,
    },
    #[error(
        "audit filesystem reserve reached for {path}: {available_bytes} bytes available, {required_bytes} bytes required"
    )]
    LowSpace {
        path: PathBuf,
        available_bytes: u64,
        required_bytes: u64,
    },
    #[error("audit sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("audit sequence space exhausted")]
    SequenceExhausted,
    #[error("audit event is too large: {0} bytes")]
    Oversized(usize),
    #[error("audit event {0} is incomplete")]
    IncompleteRecord(u64),
    #[error("audit event {0} does not extend the previous hash")]
    PreviousHash(u64),
    #[error("audit event {0} failed content-hash verification")]
    Hash(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    type Entry = AuditEntry<String, String, String>;

    fn entry(seq: u64, value: &str) -> Entry {
        Entry {
            seq,
            ts_mono_ms: seq,
            origin: "actor".into(),
            mutation: value.into(),
            effect: "applied".into(),
        }
    }

    fn test_options() -> AuditStoreOptions {
        AuditStoreOptions {
            min_free_bytes: 0,
            ..AuditStoreOptions::default()
        }
    }

    fn open_store(
        path: impl Into<PathBuf>,
    ) -> Result<(ChainedEventStore<Entry>, Vec<Entry>), AuditError> {
        ChainedEventStore::open_with_options(path, DEFAULT_CAPACITY, test_options())
    }

    #[test]
    fn live_projection_detects_gaps_and_eviction() {
        let mut log = AuditLog::new(2);
        log.push_verified(entry(1, "one")).unwrap();
        log.push_verified(entry(2, "two")).unwrap();
        log.push_verified(entry(3, "three")).unwrap();
        assert_eq!(log.oldest_seq(), 2);
        assert_eq!(log.latest_seq(), 3);
        assert_eq!(log.since(1).entries.len(), 2);
        assert!(matches!(
            log.push_verified(entry(5, "gap")),
            Err(AuditError::Sequence {
                expected: 4,
                actual: 5
            })
        ));
    }

    #[test]
    fn durable_store_round_trips_and_continues_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let (mut store, replayed) = open_store(&path).unwrap();
        assert!(replayed.is_empty());
        store.append(&entry(1, "one")).unwrap();
        store.append(&entry(2, "two")).unwrap();
        drop(store);

        let (mut reopened, replayed) = open_store(&path).unwrap();
        assert_eq!(replayed, vec![entry(1, "one"), entry(2, "two")]);
        assert_eq!(reopened.next_sequence(), 3);
        reopened.append(&entry(3, "three")).unwrap();
    }

    #[test]
    fn new_store_and_parent_are_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let (_store, _) = open_store(&path).unwrap();
        let directory_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode();
        let key_mode = std::fs::metadata(sidecar_path(&path, "key").unwrap())
            .unwrap()
            .permissions()
            .mode();
        let checkpoint_mode = std::fs::metadata(sidecar_path(&path, "checkpoint").unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(directory_mode & 0o077, 0);
        assert_eq!(file_mode & 0o077, 0);
        assert_eq!(key_mode & 0o077, 0);
        assert_eq!(checkpoint_mode & 0o077, 0);
    }

    #[test]
    fn tampering_and_incomplete_records_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let (mut store, _) = open_store(&path).unwrap();
        store.append(&entry(1, "one")).unwrap();
        drop(store);

        let mut bytes = std::fs::read(&path).unwrap();
        let byte = bytes.iter_mut().find(|byte| **byte == b'o').unwrap();
        *byte = b'x';
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            open_store(&path),
            Err(AuditError::Hash(1))
                | Err(AuditError::Decode(_))
                | Err(AuditError::PreviousHash(1))
        ));

        std::fs::write(&path, b"{\"version\":1").unwrap();
        assert!(matches!(
            open_store(&path),
            Err(AuditError::IncompleteRecord(1))
        ));
    }

    #[test]
    fn a_second_live_opener_fails_fast_instead_of_corrupting_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let (mut store, _) = open_store(&path).unwrap();
        store.append(&entry(1, "one")).unwrap();
        assert!(matches!(open_store(&path), Err(AuditError::Locked(_))));
        drop(store);

        let (reopened, replayed) = open_store(&path).unwrap();
        assert_eq!(replayed, vec![entry(1, "one")]);
        assert_eq!(reopened.next_sequence(), 2);
    }

    #[test]
    fn authenticated_checkpoint_bounds_synchronous_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let options = AuditStoreOptions {
            checkpoint_interval_events: 1,
            min_free_bytes: 0,
            ..AuditStoreOptions::default()
        };
        let (mut store, _) = ChainedEventStore::open_with_options(&path, 2, options).unwrap();
        for sequence in 1..=5 {
            store
                .append(&entry(sequence, &format!("event-{sequence}")))
                .unwrap();
        }
        drop(store);

        let (mut reopened, replayed) =
            ChainedEventStore::open_with_options(&path, 2, options).unwrap();
        assert!(reopened.checkpoint_accelerated());
        assert_eq!(replayed, vec![entry(4, "event-4"), entry(5, "event-5")]);
        assert_eq!(reopened.next_sequence(), 6);
        reopened.append(&entry(6, "event-6")).unwrap();
    }

    #[test]
    fn audit_log_restores_a_tail_whose_first_sequence_is_after_genesis() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let options = AuditStoreOptions {
            checkpoint_interval_events: 1,
            min_free_bytes: 0,
            ..AuditStoreOptions::default()
        };
        let mut log = AuditLog::<Entry>::open_persistent_with_options(2, &path, options).unwrap();
        for sequence in 1..=5 {
            log.push_verified(entry(sequence, &format!("event-{sequence}")))
                .unwrap();
        }
        drop(log);

        let mut reopened =
            AuditLog::<Entry>::open_persistent_with_options(2, &path, options).unwrap();
        assert_eq!(reopened.oldest_seq(), 4);
        assert_eq!(reopened.latest_seq(), 5);
        assert_eq!(
            reopened.since(0).entries,
            vec![entry(4, "event-4"), entry(5, "event-5")]
        );
        reopened.push_verified(entry(6, "event-6")).unwrap();
    }

    #[test]
    fn checkpoint_mac_tampering_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let (mut store, _) = open_store(&path).unwrap();
        store.append(&entry(1, "one")).unwrap();
        drop(store);

        let checkpoint_path = sidecar_path(&path, "checkpoint").unwrap();
        let mut checkpoint: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&checkpoint_path).unwrap()).unwrap();
        checkpoint["mac"] = serde_json::Value::String("00".repeat(32));
        std::fs::write(&checkpoint_path, serde_json::to_vec(&checkpoint).unwrap()).unwrap();
        assert!(matches!(
            open_store(&path),
            Err(AuditError::CheckpointAuthentication)
        ));
    }

    #[test]
    fn old_prefix_is_fully_verified_before_the_chain_can_advance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let options = AuditStoreOptions {
            checkpoint_interval_events: 1,
            min_free_bytes: 0,
            ..AuditStoreOptions::default()
        };
        let (mut store, _) = ChainedEventStore::open_with_options(&path, 2, options).unwrap();
        for sequence in 1..=5 {
            store
                .append(&entry(sequence, &format!("event-{sequence}")))
                .unwrap();
        }
        drop(store);

        let mut bytes = std::fs::read(&path).unwrap();
        let first = bytes
            .windows(b"event-1".len())
            .position(|window| window == b"event-1")
            .unwrap();
        bytes[first] = b'X';
        std::fs::write(&path, bytes).unwrap();

        let (mut reopened, replayed) =
            ChainedEventStore::open_with_options(&path, 2, options).unwrap();
        assert_eq!(
            replayed.len(),
            2,
            "the authenticated tail restores immediately"
        );
        assert!(matches!(
            reopened.append(&entry(6, "event-6")),
            Err(AuditError::BackgroundVerification(_))
        ));
    }

    #[test]
    fn quota_and_filesystem_reserve_refuse_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let quota_path = dir.path().join("quota/events.jsonl");
        let quota_options = AuditStoreOptions {
            max_store_bytes: (MAX_EVENT_BYTES + 1) as u64,
            min_free_bytes: 0,
            checkpoint_interval_bytes: u64::MAX,
            checkpoint_interval_events: u64::MAX,
            segment_max_bytes: (MAX_EVENT_BYTES + 1) as u64,
            retain_segments: 0,
        };
        let (mut quota_store, _) =
            ChainedEventStore::open_with_options(&quota_path, 2, quota_options).unwrap();
        let payload = "x".repeat(3 * 1024 * 1024);
        quota_store.append(&entry(1, &payload)).unwrap();
        let before = std::fs::metadata(&quota_path).unwrap().len();
        assert!(matches!(
            quota_store.append(&entry(2, &payload)),
            Err(AuditError::QuotaExceeded { .. })
        ));
        assert_eq!(std::fs::metadata(&quota_path).unwrap().len(), before);

        let reserve_path = dir.path().join("reserve/events.jsonl");
        let reserve_options = AuditStoreOptions {
            min_free_bytes: u64::MAX,
            ..AuditStoreOptions::default()
        };
        let (mut reserve_store, _) =
            ChainedEventStore::open_with_options(&reserve_path, 2, reserve_options).unwrap();
        assert!(matches!(
            reserve_store.append(&entry(1, "one")),
            Err(AuditError::LowSpace { .. })
        ));
        assert_eq!(std::fs::metadata(&reserve_path).unwrap().len(), 0);
    }

    #[test]
    fn unsafe_permissions_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let audit_dir = dir.path().join("audit");
        std::fs::create_dir(&audit_dir).unwrap();
        let path = audit_dir.join("events.jsonl");
        std::fs::write(&path, []).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(open_store(path), Err(AuditError::UnsafePath(_))));
    }

    fn segment_options(segment_max_bytes: u64) -> AuditStoreOptions {
        AuditStoreOptions {
            min_free_bytes: 0,
            checkpoint_interval_events: u64::MAX,
            checkpoint_interval_bytes: u64::MAX,
            segment_max_bytes,
            retain_segments: 0,
            ..AuditStoreOptions::default()
        }
    }

    fn append_many(
        store: &mut ChainedEventStore<Entry>,
        from: u64,
        to: u64,
    ) -> Result<(), AuditError> {
        for sequence in from..to {
            store.append(&entry(sequence, &format!("event-{sequence}")))?;
        }
        Ok(())
    }

    #[test]
    fn sealing_continues_the_chain_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        // Tiny segment ceiling so the first few events seal immediately.
        let options = segment_options(1);
        let (mut store, _) = ChainedEventStore::open_with_options(&path, 8, options).unwrap();
        append_many(&mut store, 1, 7).unwrap();
        // Sealing must have happened: the active stream restarts small while
        // sequence numbers continue.
        assert!(store.next_sequence() >= 7);
        let sealed_at_close = store.audit_status().sealed_segments;
        assert!(sealed_at_close >= 1, "sealing did not trigger");
        drop(store);

        let (mut reopened, replayed) =
            ChainedEventStore::<Entry>::open_with_options(&path, 8, options).unwrap();
        assert_eq!(reopened.next_sequence(), 7);
        assert!(
            replayed.iter().all(|entry: &Entry| entry.seq <= 6),
            "replayed tail must come from the current epoch"
        );
        // The chain continues across the seal without a sequence reset.
        append_many(&mut reopened, 7, 10).unwrap();
        assert_eq!(reopened.next_sequence(), 10);
        drop(reopened);

        let (final_store, _) =
            ChainedEventStore::<Entry>::open_with_options(&path, 8, options).unwrap();
        let status = final_store.audit_status();
        assert_eq!(status.next_sequence, 10);
        assert!(status.sealed_segments >= sealed_at_close);
    }

    #[test]
    fn fast_verification_detects_segment_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let options = segment_options(1);
        let (mut store, _) = ChainedEventStore::open_with_options(&path, 8, options).unwrap();
        append_many(&mut store, 1, 6).unwrap();
        drop(store);

        // Corrupt one sealed segment byte.
        let segments_dir = path.with_file_name(segments::SEGMENTS_DIR);
        let segment = std::fs::read_dir(&segments_dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut bytes = std::fs::read(&segment).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&segment, bytes).unwrap();

        let (store, _) = ChainedEventStore::<Entry>::open_with_options(&path, 8, options).unwrap();
        let status = store.audit_status();
        assert!(status.sealed_segments >= 1);
        // The manifest-level fast check reports the corruption.
        let manifest = segments::SegmentManifest::open(
            &path,
            &std::fs::read(path.with_file_name("events.jsonl.key")).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            manifest.verify_fast(),
            Err(AuditError::SegmentVerification { .. })
        ));
    }

    #[test]
    fn retention_prunes_only_exported_segments_and_records_removal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let mut options = segment_options(1);
        options.retain_segments = 2;
        let (mut store, _) = ChainedEventStore::open_with_options(&path, 8, options).unwrap();
        append_many(&mut store, 1, 12).unwrap();
        drop(store);

        let key = std::fs::read(path.with_file_name("events.jsonl.key")).unwrap();
        let mut manifest = segments::SegmentManifest::open(&path, &key).unwrap();
        let sealed = manifest.segments().len();
        assert!(sealed > 2, "expected multiple sealed segments");

        // Pruning without export acknowledgements refuses.
        assert!(manifest.prune(2, true).is_err());

        // After acknowledging an export, pruning removes all but two.
        manifest.mark_exported("/mnt/audit-archive").unwrap();
        let removed = manifest.prune(2, true).unwrap();
        assert_eq!(removed.len(), sealed - 2);
        assert_eq!(manifest.segments().len(), 2);
        assert_eq!(manifest.body().pruned.len(), removed.len());

        // Fast verification still passes for the retained segments.
        assert_eq!(manifest.verify_fast().unwrap(), 2);

        // The manifest itself authenticates: tampering is detected.
        let manifest_file = segments::manifest_path(&path);
        let mut body = std::fs::read(&manifest_file).unwrap();
        body[10] ^= 0x01;
        std::fs::write(&manifest_file, body).unwrap();
        assert!(matches!(
            segments::SegmentManifest::open(&path, &key),
            Err(AuditError::CheckpointAuthentication) | Err(AuditError::CheckpointDecode(_))
        ));
    }

    #[test]
    fn sealing_preserves_chain_integrity_across_decompression() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let options = segment_options(1);
        let (mut store, _) = ChainedEventStore::open_with_options(&path, 8, options).unwrap();
        append_many(&mut store, 1, 8).unwrap();
        drop(store);

        let key = std::fs::read(path.with_file_name("events.jsonl.key")).unwrap();
        let manifest = segments::SegmentManifest::open(&path, &key).unwrap();
        let segments_root = path.with_file_name(segments::SEGMENTS_DIR);
        // Walk the sealed chain: every segment's tail must equal the next
        // segment's previous anchor.
        let records = manifest.segments();
        for pair in records.windows(2) {
            assert_eq!(
                pair[0].tail_hash, pair[1].previous_hash,
                "sealed segments must chain"
            );
            assert_eq!(
                pair[0].last_sequence_exclusive, pair[1].first_sequence,
                "sealed sequences must be contiguous"
            );
        }
        // Decompress one segment and verify its JSONL lines parse.
        let record = &records[0];
        let mut reader = segments::SealedSegmentReader::open(record, &segments_root).unwrap();
        let mut lines = 0usize;
        reader
            .for_each_line(|line| {
                assert_eq!(line.last(), Some(&b'\n'));
                let envelope: serde_json::Value =
                    serde_json::from_slice(&line[..line.len() - 1]).map_err(AuditError::Decode)?;
                assert!(envelope["sequence"].is_u64());
                lines += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            lines as u64,
            record.last_sequence_exclusive - record.first_sequence,
            "every sealed event must be present after decompression"
        );
    }
}
