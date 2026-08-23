//! Sealed audit segments and the authenticated segment manifest (ADR-0137).
//!
//! The active JSONL stream is sealed into an immutable gzip segment when it
//! reaches the configured size. A manifest file, authenticated with the same
//! owner-only HMAC key as the replay checkpoint, records every sealed
//! segment's identity (first/last sequence, tail hash, compressed digest) so
//! the chain stays verifiable across segments without keeping every sealed
//! byte hot. The active stream continues the hash chain from the manifest's
//! tail; global sequence numbers never reset.
//!
//! Retention is explicit: [`SegmentManifest::prune`] only runs with a
//! configured segment budget and records what was removed in the manifest
//! itself, so pruning remains auditable. Nothing here silently deletes
//! history on its own.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use hmac::Mac as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::AuditError;

/// Manifest schema version.
pub const MANIFEST_VERSION: u32 = 1;

/// Maximum accepted manifest size before it is considered corrupt.
pub const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;

/// Directory holding sealed segments, relative to the audit directory.
pub const SEGMENTS_DIR: &str = "segments";

const MANIFEST_FILE: &str = "events-v2.jsonl.manifest";

/// One sealed, immutable segment as recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentRecord {
    /// Seal order; strictly increasing across segments.
    pub index: u64,
    /// Global sequence of the first event in the segment (inclusive).
    pub first_sequence: u64,
    /// Global sequence one past the last event in the segment.
    pub last_sequence_exclusive: u64,
    /// Chain hash of the event before `first_sequence` (the segment's
    /// incoming chain anchor).
    pub previous_hash: String,
    /// Chain hash of the segment's final event.
    pub tail_hash: String,
    /// Original uncompressed byte length of the sealed stream.
    pub original_bytes: u64,
    /// Compressed on-disk size in bytes.
    pub compressed_bytes: u64,
    /// SHA-256 of the compressed file contents, hex encoded.
    pub compressed_sha256: String,
    /// Wall-clock Unix time (seconds) when the segment was sealed.
    pub sealed_at_unix: u64,
    /// Non-empty export destination recorded by `aegis audit export`, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported_to: Option<String>,
}

/// A segment that was intentionally removed under a retention policy. The
/// cryptographic identity is retained so the removal itself stays auditable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrunedSegment {
    pub index: u64,
    pub first_sequence: u64,
    pub last_sequence_exclusive: u64,
    pub tail_hash: String,
    pub compressed_sha256: String,
    pub pruned_at_unix: u64,
    /// Recorded export destination, if the segment was exported first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported_to: Option<String>,
}

/// The authenticated manifest body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestBody {
    pub version: u32,
    /// Chain hash the active stream must extend (`GENESIS_HASH` when empty).
    pub tail_hash: String,
    /// Global sequence the active stream continues from.
    pub next_sequence: u64,
    /// Sealed segments still present on disk, in seal order.
    pub segments: Vec<SegmentRecord>,
    /// Segments removed under an explicit retention policy, oldest first.
    pub pruned: Vec<PrunedSegment>,
}

impl ManifestBody {
    fn empty() -> Self {
        Self {
            version: MANIFEST_VERSION,
            tail_hash: super::GENESIS_HASH.to_owned(),
            next_sequence: 1,
            segments: Vec::new(),
            pruned: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SignedManifest {
    manifest: ManifestBody,
    mac: String,
}

/// Summary of a manifest for status reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditStatus {
    pub next_sequence: u64,
    pub tail_hash: String,
    pub sealed_segments: usize,
    pub pruned_segments: usize,
    pub sealed_original_bytes: u64,
    pub sealed_compressed_bytes: u64,
    pub active_bytes: u64,
    /// Sum of sealed compressed bytes plus the active stream length.
    pub total_bytes: u64,
    pub last_export_destination: Option<String>,
}

/// Locate the manifest and segments directory for an active-stream path.
pub fn manifest_path(active_path: &Path) -> PathBuf {
    active_path.with_file_name(MANIFEST_FILE)
}

fn segments_dir(active_path: &Path) -> PathBuf {
    active_path.with_file_name(SEGMENTS_DIR)
}

fn segment_file_name(record: &SegmentRecord) -> String {
    format!(
        "{:05}-{}-{}.jsonl.gz",
        record.index, record.first_sequence, record.tail_hash
    )
}

/// The manifest, opened and authenticated against the store's HMAC key.
pub struct SegmentManifest {
    path: PathBuf,
    segments: PathBuf,
    key: Vec<u8>,
    body: ManifestBody,
}

impl SegmentManifest {
    /// Load and authenticate the manifest for `active_path`, creating an
    /// empty one if none exists. A missing manifest alongside an existing
    /// non-empty active stream is accepted: the stream is then a pre-ADR-0137
    /// store and `seal_active` will fold it into the manifest on first seal.
    pub fn open(active_path: &Path, key: &[u8]) -> Result<Self, AuditError> {
        let path = manifest_path(active_path);
        let segments = segments_dir(active_path);
        if !path_exists(&path)? {
            return Ok(Self {
                path,
                segments,
                key: key.to_vec(),
                body: ManifestBody::empty(),
            });
        }
        validate_private_file(&path)?;
        let file = File::open(&path).map_err(|source| AuditError::Io {
            path: path.clone(),
            source,
        })?;
        let mut bytes = Vec::new();
        file.take(MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| AuditError::Io {
                path: path.clone(),
                source,
            })?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(AuditError::SegmentState("manifest is oversized"));
        }
        let signed: SignedManifest =
            serde_json::from_slice(&bytes).map_err(AuditError::CheckpointDecode)?;
        verify_mac(key, &signed.manifest, &signed.mac)?;
        if signed.manifest.version != MANIFEST_VERSION {
            return Err(AuditError::SegmentState("unsupported manifest version"));
        }
        Ok(Self {
            path,
            segments,
            key: key.to_vec(),
            body: signed.manifest,
        })
    }

    /// The authenticated manifest body.
    pub fn body(&self) -> &ManifestBody {
        &self.body
    }

    /// Chain hash the active stream must extend.
    pub fn tail_hash(&self) -> &str {
        &self.body.tail_hash
    }

    /// Global sequence the active stream continues from.
    pub fn next_sequence(&self) -> u64 {
        self.body.next_sequence
    }

    /// Sealed segments still on disk, in seal order.
    pub fn segments(&self) -> &[SegmentRecord] {
        &self.body.segments
    }

    /// Compute a status summary including the active stream's size.
    pub fn status(&self, active_bytes: u64) -> AuditStatus {
        let sealed_compressed_bytes = self
            .body
            .segments
            .iter()
            .map(|record| record.compressed_bytes)
            .sum();
        AuditStatus {
            next_sequence: self.body.next_sequence,
            tail_hash: self.body.tail_hash.clone(),
            sealed_segments: self.body.segments.len(),
            pruned_segments: self.body.pruned.len(),
            sealed_original_bytes: self
                .body
                .segments
                .iter()
                .map(|record| record.original_bytes)
                .sum(),
            sealed_compressed_bytes,
            active_bytes,
            total_bytes: sealed_compressed_bytes.saturating_add(active_bytes),
            last_export_destination: self
                .body
                .segments
                .iter()
                .rev()
                .find_map(|record| record.exported_to.clone()),
        }
    }

    /// Seal the current active stream into a compressed segment and start a
    /// fresh active stream that continues the chain. The active stream is
    /// hash-verified end to end while being compressed, so a corrupt active
    /// stream is never sealed. `first_sequence` is the global sequence of the
    /// active stream's first event and `previous_hash` is the chain anchor it
    /// extends.
    ///
    /// Returns the sealed record and the expected `(next_sequence,
    /// tail_hash)` for the new active stream. The active file itself is
    /// truncated to zero by the caller after this returns.
    #[allow(clippy::too_many_arguments)]
    pub fn seal_active(
        &mut self,
        active_path: &Path,
        first_sequence: u64,
        previous_hash: &str,
        next_sequence: u64,
        tail_hash: &str,
        active_len: u64,
    ) -> Result<SegmentRecord, AuditError> {
        if active_len == 0 {
            return Err(AuditError::SegmentState("active stream is empty"));
        }
        if next_sequence <= first_sequence {
            return Err(AuditError::SegmentState("segment has no events"));
        }
        std::fs::create_dir_all(&self.segments).map_err(|source| AuditError::Io {
            path: self.segments.clone(),
            source,
        })?;
        restrict_directory(&self.segments)?;

        let index = self
            .body
            .segments
            .last()
            .map_or(0, |record| record.index)
            .saturating_add(1)
            .max(
                self.body
                    .pruned
                    .last()
                    .map_or(0, |p| p.index)
                    .saturating_add(1),
            );
        let record = SegmentRecord {
            index,
            first_sequence,
            last_sequence_exclusive: next_sequence,
            previous_hash: previous_hash.to_owned(),
            tail_hash: tail_hash.to_owned(),
            original_bytes: active_len,
            compressed_bytes: 0,
            compressed_sha256: String::new(),
            sealed_at_unix: unix_now()?,
            exported_to: None,
        };
        let destination = self.segments.join(segment_file_name(&record));
        let (compressed_bytes, digest) = compress_verified(active_path, &destination)?;

        let mut record = record;
        record.compressed_bytes = compressed_bytes;
        record.compressed_sha256 = digest;
        self.body.segments.push(record.clone());
        self.body.tail_hash = tail_hash.to_owned();
        self.body.next_sequence = next_sequence;
        self.persist()?;
        Ok(record)
    }

    /// Record that every currently sealed segment was exported to
    /// `destination`, persisting the acknowledgement in the manifest.
    pub fn mark_exported(&mut self, destination: &str) -> Result<usize, AuditError> {
        if destination.trim().is_empty() {
            return Err(AuditError::SegmentState("export destination is empty"));
        }
        let count = self.body.segments.len();
        for record in &mut self.body.segments {
            record.exported_to = Some(destination.to_owned());
        }
        self.persist()?;
        Ok(count)
    }

    /// Apply a retention policy: keep at most `keep` sealed segments (the
    /// newest), deleting older ones from disk and recording the removal in
    /// the manifest. Requires every removed segment to carry an export
    /// acknowledgement unless `require_export` is false. Returns the removed
    /// records. `keep = 0` means "keep everything" and is a no-op.
    pub fn prune(
        &mut self,
        keep: usize,
        require_export: bool,
    ) -> Result<Vec<SegmentRecord>, AuditError> {
        if keep == 0 {
            return Ok(Vec::new());
        }
        let excess = self.body.segments.len().saturating_sub(keep);
        if excess == 0 {
            return Ok(Vec::new());
        }
        let (remove, _) = self.body.segments.split_at(excess);
        for record in remove {
            if require_export && record.exported_to.is_none() {
                return Err(AuditError::SegmentState(
                    "refusing to prune a segment without an export acknowledgement",
                ));
            }
        }
        let now = unix_now()?;
        let mut removals: Vec<PrunedSegment> = Vec::with_capacity(excess);
        let mut removed: Vec<SegmentRecord> = Vec::with_capacity(excess);
        let mut kept: Vec<SegmentRecord> = Vec::with_capacity(keep);
        for (position, record) in std::mem::take(&mut self.body.segments)
            .into_iter()
            .enumerate()
        {
            if position < excess {
                let file = self.segments.join(segment_file_name(&record));
                // The manifest is the authority; a segment file that is
                // already absent is still recorded as pruned.
                if path_exists(&file)? {
                    std::fs::remove_file(&file).map_err(|source| AuditError::Io {
                        path: file.clone(),
                        source,
                    })?;
                }
                removals.push(PrunedSegment {
                    index: record.index,
                    first_sequence: record.first_sequence,
                    last_sequence_exclusive: record.last_sequence_exclusive,
                    tail_hash: record.tail_hash.clone(),
                    compressed_sha256: record.compressed_sha256.clone(),
                    pruned_at_unix: now,
                    exported_to: record.exported_to.clone(),
                });
                removed.push(record);
            } else {
                kept.push(record);
            }
        }
        self.body.pruned.extend(removals);
        self.body.segments = kept;
        self.persist()?;
        Ok(removed)
    }

    /// Fast integrity check of every sealed segment: file presence, size, and
    /// compressed-content SHA-256 against the manifest. Returns the number of
    /// segments verified.
    pub fn verify_fast(&self) -> Result<usize, AuditError> {
        for record in &self.body.segments {
            let file = self.segments.join(segment_file_name(record));
            let metadata = std::fs::metadata(&file).map_err(|source| AuditError::Io {
                path: file.clone(),
                source,
            })?;
            if !metadata.file_type().is_file() {
                return Err(AuditError::SegmentState("segment is not a regular file"));
            }
            if metadata.len() != record.compressed_bytes {
                return Err(AuditError::SegmentState(
                    "segment size does not match the manifest",
                ));
            }
            let digest = sha256_file(&file)?;
            if !constant_time_eq(digest.as_bytes(), record.compressed_sha256.as_bytes()) {
                return Err(AuditError::SegmentVerification {
                    index: record.index,
                });
            }
        }
        Ok(self.body.segments.len())
    }

    fn persist(&self) -> Result<(), AuditError> {
        let signed = SignedManifest {
            manifest: self.body.clone(),
            mac: compute_mac(&self.key, &self.body)?,
        };
        let mut bytes = serde_json::to_vec(&signed).map_err(AuditError::Encode)?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(AuditError::SegmentState("manifest is oversized"));
        }
        write_private_atomic(
            self.path.parent().unwrap_or(Path::new(".")),
            &self.path,
            &bytes,
        )
    }
}

fn unix_now() -> Result<u64, AuditError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|source| AuditError::Io {
            path: PathBuf::new(),
            source: std::io::Error::other(source),
        })
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

fn validate_private_file(path: &Path) -> Result<(), AuditError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|source| AuditError::Io {
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

/// Compress `source` to `destination` while computing the SHA-256 of the
/// compressed bytes. Returns `(compressed_len, hex_digest)`.
fn compress_verified(source: &Path, destination: &Path) -> Result<(u64, String), AuditError> {
    let input = File::open(source).map_err(|source_error| AuditError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    let mut reader = BufReader::new(input);
    let mut compressor = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source_error| AuditError::Io {
                path: source.to_path_buf(),
                source: source_error,
            })?;
        if read == 0 {
            break;
        }
        compressor
            .write_all(&buffer[..read])
            .map_err(|source_error| AuditError::Io {
                path: destination.to_path_buf(),
                source: std::io::Error::other(source_error),
            })?;
    }
    let compressed = compressor.finish().map_err(|source_error| AuditError::Io {
        path: destination.to_path_buf(),
        source: std::io::Error::other(source_error),
    })?;
    let compressed_digest = Sha256::digest(&compressed);
    write_private_atomic(
        destination.parent().unwrap_or(Path::new(".")),
        destination,
        &compressed,
    )?;
    Ok((compressed.len() as u64, hex(&compressed_digest)))
}

fn sha256_file(path: &Path) -> Result<String, AuditError> {
    let mut file = File::open(path).map_err(|source| AuditError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| AuditError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn write_private_atomic(parent: &Path, path: &Path, bytes: &[u8]) -> Result<(), AuditError> {
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
        file.write_all(bytes)
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

fn compute_mac(key: &[u8], body: &ManifestBody) -> Result<String, AuditError> {
    let bytes = serde_json::to_vec(body).map_err(AuditError::Encode)?;
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| AuditError::CheckpointState("manifest key is invalid"))?;
    mac.update(&bytes);
    Ok(hex(&mac.finalize().into_bytes()))
}

fn verify_mac(key: &[u8], body: &ManifestBody, encoded: &str) -> Result<(), AuditError> {
    let bytes = serde_json::to_vec(body).map_err(AuditError::Encode)?;
    let expected =
        decode_hash(encoded).ok_or(AuditError::SegmentState("manifest MAC is malformed"))?;
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| AuditError::CheckpointState("manifest key is invalid"))?;
    mac.update(&bytes);
    mac.verify_slice(&expected)
        .map_err(|_| AuditError::CheckpointAuthentication)
}

fn hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    use std::fmt::Write as _;
    for byte in bytes {
        write!(&mut result, "{byte:02x}").expect("writing to String cannot fail");
    }
    result
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

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

/// Read sealed records for a full decompress-and-verify pass. Exposed for
/// `aegis audit verify --full`: the caller supplies the per-line envelope
/// check by iterating the decompressed lines in order.
pub struct SealedSegmentReader {
    file: BufReader<File>,
    index: u64,
}

impl SealedSegmentReader {
    /// Open and fast-check a sealed segment recorded in the manifest.
    pub fn open(record: &SegmentRecord, segments: &Path) -> Result<Self, AuditError> {
        let file_path = segments.join(segment_file_name(record));
        let metadata = std::fs::metadata(&file_path).map_err(|source| AuditError::Io {
            path: file_path.clone(),
            source,
        })?;
        if metadata.len() != record.compressed_bytes {
            return Err(AuditError::SegmentState(
                "segment size does not match the manifest",
            ));
        }
        let digest = sha256_file(&file_path)?;
        if !constant_time_eq(digest.as_bytes(), record.compressed_sha256.as_bytes()) {
            return Err(AuditError::SegmentVerification {
                index: record.index,
            });
        }
        let file = File::open(&file_path).map_err(|source| AuditError::Io {
            path: file_path.clone(),
            source,
        })?;
        Ok(Self {
            file: BufReader::new(file),
            index: record.index,
        })
    }

    /// Segment seal index, for diagnostics.
    pub fn index(&self) -> u64 {
        self.index
    }

    /// Stream the decompressed raw JSONL lines through `sink`. The sink
    /// returns an error to abort verification.
    /// Stream the decompressed raw JSONL lines through `sink`. The sink
    /// returns an error to abort verification.
    pub fn for_each_line<F>(&mut self, mut sink: F) -> Result<(), AuditError>
    where
        F: FnMut(&[u8]) -> Result<(), AuditError>,
    {
        let decoder = flate2::read::MultiGzDecoder::new(self.file.by_ref());
        let mut reader = BufReader::new(decoder);
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader
                .read_until(b'\n', &mut line)
                .map_err(|source| AuditError::Io {
                    path: PathBuf::new(),
                    source,
                })?;
            if read == 0 {
                break;
            }
            if line.last() != Some(&b'\n') {
                return Err(AuditError::IncompleteRecord(0));
            }
            sink(&line)?;
        }
        Ok(())
    }
}
