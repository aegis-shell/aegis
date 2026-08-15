//! Hash-chained event storage and bounded live audit projections.
//!
//! The event store is deliberately generic: domain crates own event schemas
//! and reducers, while this crate owns ordering, durability, file safety, and
//! hash-chain verification.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Default live projection size. Durable storage, when configured, is not
/// truncated when this projection evicts its oldest entry.
pub const DEFAULT_CAPACITY: usize = 4_096;

/// Maximum serialized event envelope accepted from disk or for append.
pub const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;

const STORE_VERSION: u32 = 1;
const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

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
    T: Clone + Sequence + Serialize + DeserializeOwned,
{
    /// Open a verified durable stream and rebuild its bounded live
    /// projection. Any unsafe file or invalid record aborts initialization.
    pub fn open_persistent(capacity: usize, path: impl Into<PathBuf>) -> Result<Self, AuditError> {
        let (store, entries) = ChainedEventStore::open(path)?;
        let mut log = Self::new(capacity);
        for entry in entries {
            log.push_memory(entry)?;
        }
        debug_assert_eq!(log.next_seq, store.next_sequence());
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

/// Owner-only append-only event store with a SHA-256 hash chain.
pub struct ChainedEventStore<T> {
    path: PathBuf,
    writer: BufWriter<File>,
    next_sequence: u64,
    tail_hash: String,
    _event: std::marker::PhantomData<T>,
}

impl<T> std::fmt::Debug for ChainedEventStore<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChainedEventStore")
            .field("path", &self.path)
            .field("next_sequence", &self.next_sequence)
            .field("tail_hash", &self.tail_hash)
            .finish_non_exhaustive()
    }
}

impl<T> ChainedEventStore<T>
where
    T: Clone + Sequence + Serialize + DeserializeOwned,
{
    /// Open and verify an existing store, or create a new owner-only store.
    /// The returned events are complete and hash-verified in sequence order.
    ///
    /// A successful open holds the store's exclusive advisory lock for the
    /// store's lifetime. A second live opener fails fast with
    /// [`AuditError::Locked`] instead of interleaving appends from stale
    /// sequence state, which would corrupt the chain seen by the next open.
    pub fn open(path: impl Into<PathBuf>) -> Result<(Self, Vec<T>), AuditError> {
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
        let (events, next_sequence, tail_hash) = replay(&path, replay_file)?;
        Ok((
            Self {
                path,
                writer: BufWriter::new(file),
                next_sequence,
                tail_hash,
                _event: std::marker::PhantomData,
            },
            events,
        ))
    }

    /// Append and synchronously persist one event. Sequence and chain state
    /// advance only after the bytes reach the kernel's durable-file boundary.
    pub fn append(&mut self, event: &T) -> Result<(), AuditError> {
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
        Ok(())
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn tail_hash(&self) -> &str {
        &self.tail_hash
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

fn replay<T>(path: &Path, file: File) -> Result<(Vec<T>, u64, String), AuditError>
where
    T: Sequence + Serialize + DeserializeOwned,
{
    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut expected_sequence = 1u64;
    let mut previous_hash = GENESIS_HASH.to_owned();
    let mut line = Vec::new();
    loop {
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
        events.push(envelope.event);
    }
    Ok((events, expected_sequence, previous_hash))
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
        let (mut store, replayed) = ChainedEventStore::<Entry>::open(&path).unwrap();
        assert!(replayed.is_empty());
        store.append(&entry(1, "one")).unwrap();
        store.append(&entry(2, "two")).unwrap();
        drop(store);

        let (mut reopened, replayed) = ChainedEventStore::<Entry>::open(&path).unwrap();
        assert_eq!(replayed, vec![entry(1, "one"), entry(2, "two")]);
        assert_eq!(reopened.next_sequence(), 3);
        reopened.append(&entry(3, "three")).unwrap();
    }

    #[test]
    fn new_store_and_parent_are_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let (_store, _) = ChainedEventStore::<Entry>::open(&path).unwrap();
        let directory_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(directory_mode & 0o077, 0);
        assert_eq!(file_mode & 0o077, 0);
    }

    #[test]
    fn tampering_and_incomplete_records_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let (mut store, _) = ChainedEventStore::<Entry>::open(&path).unwrap();
        store.append(&entry(1, "one")).unwrap();
        drop(store);

        let mut bytes = std::fs::read(&path).unwrap();
        let byte = bytes.iter_mut().find(|byte| **byte == b'o').unwrap();
        *byte = b'x';
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            ChainedEventStore::<Entry>::open(&path),
            Err(AuditError::Hash(1))
                | Err(AuditError::Decode(_))
                | Err(AuditError::PreviousHash(1))
        ));

        std::fs::write(&path, b"{\"version\":1").unwrap();
        assert!(matches!(
            ChainedEventStore::<Entry>::open(&path),
            Err(AuditError::IncompleteRecord(1))
        ));
    }

    #[test]
    fn a_second_live_opener_fails_fast_instead_of_corrupting_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/events.jsonl");
        let (mut store, _) = ChainedEventStore::<Entry>::open(&path).unwrap();
        store.append(&entry(1, "one")).unwrap();
        assert!(matches!(
            ChainedEventStore::<Entry>::open(&path),
            Err(AuditError::Locked(_))
        ));
        drop(store);

        let (reopened, replayed) = ChainedEventStore::<Entry>::open(&path).unwrap();
        assert_eq!(replayed, vec![entry(1, "one")]);
        assert_eq!(reopened.next_sequence(), 2);
    }

    #[test]
    fn unsafe_permissions_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let audit_dir = dir.path().join("audit");
        std::fs::create_dir(&audit_dir).unwrap();
        let path = audit_dir.join("events.jsonl");
        std::fs::write(&path, []).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            ChainedEventStore::<Entry>::open(path),
            Err(AuditError::UnsafePath(_))
        ));
    }
}
