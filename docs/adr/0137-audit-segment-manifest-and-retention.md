# ADR-0137: Audit segment manifest and explicit retention

- Status: Accepted
- Date: 2026-08-22

## Context

ADR-0136 bounded audit replay cost and added pre-write storage guards, but
the retention story remained manual: the active stream grows until the hard
ceiling refuses appends, and an operator who wants to reclaim space has to
move the whole JSONL file aside by hand. Nothing in the product could answer
"how much audit history is on disk", verify archived history, or free space
without discarding chain context.

The file also never shrinks: JSONL audit text compresses extremely well
(real stores are over 95% redundant), yet every byte stays hot. A desktop
product needs bounded disk use with an auditable lifecycle, without ever
letting the compositor silently destroy authority history.

## Decision

The durable audit store gains sealed segments, an authenticated manifest,
and operator-driven retention (amending ADR-0104 and ADR-0136).

**Sealing.** When the active stream reaches `[audit] segment_max_mib` (64
MiB by default), the store compresses it into an immutable gzip segment
under `audit/segments/` — verifying the hash chain end to end while
compressing, so a corrupt stream is never sealed. The active stream then
restarts empty and continues the chain from the sealed tail; global sequence
numbers never reset. A crash mid-seal leaves the worst case of ADR-0136: a
valid older checkpoint and a bounded extra tail.

**The manifest.** `events-v2.jsonl.manifest` is an HMAC-authenticated record
(using the existing owner-only key) of every sealed segment: seal index,
first/last sequence, incoming and tail chain hashes, original and compressed
sizes, compressed SHA-256, seal time, and export acknowledgements. The
manifest is rewritten atomically (fresh owner-only file, fsync, rename,
directory sync) on every seal, export, and prune. Startup authenticates the
manifest, verifies every sealed segment by compressed digest, and fails
closed on any mismatch — sealed history gates the session exactly like the
active stream.

**Retention is explicit and auditable.** Nothing is deleted automatically
without operator configuration. `[audit] retain_segments` (default `0` =
keep everything) bounds how many sealed segments survive; `aegis audit
export <destination>` records an acknowledgement for every sealed segment,
and pruning — whether automatic after sealing or explicit via `aegis audit
prune <keep>` — refuses to remove any segment that lacks one unless the
operator passes `--force`. Every removal is recorded permanently in the
manifest's `pruned` history with the segment's cryptographic identity, so
"what existed and who removed it" survives the deletion itself.

**Operator surface.** `aegis audit status` reports sequence, sizes, sealed
and pruned counts, and the last export destination; `aegis audit verify`
checks segments against the manifest, or with `--full` decompresses every
segment and replays the complete chain; `export` and `prune` manage the
lifecycle. These commands operate on the local store only — a running
compositor holds the advisory lock, so they run while the session is
stopped.

With `retain_segments` configured and exports acknowledged, the store
reaches a steady state (active segment + N sealed segments) instead of
marching toward the fail-stop ceiling. Fail-stop remains the honest behavior
when no retention is configured and the ceiling is hit.

## Alternatives

**Rotate by truncating the active file and starting a new chain.** Rejected:
it either resets sequence numbers (breaking the monotonic journal contract)
or leaves the chain anchor unverifiable, and destruction is invisible.

**Compress the active stream in place.** Rejected: the append path requires
a plain JSONL tail; an in-place compressed region would complicate every
reader and the crash-recovery reasoning for marginal benefit.

**Automatic deletion after a time window.** Rejected: time-based deletion
deletes evidence without recording that it did so, and a laptop that was
off for a month would destroy history on boot. Segment-count retention with
export acknowledgement and manifest-recorded removal is explicit.

**Signed checkpoints to a remote system.** Out of scope here; the manifest
HMAC protects against accidental or blind tampering under the owner trust
model, and ADR-0104's requirement — export records or independently signed
anchors to a separately administered system for hostile-owner evidence —
remains the answer for deployments that need that property.

## Consequences

- Disk use becomes steady-state under a retention policy instead of
  fail-stop-only; compression typically reclaims >90% of sealed volume.
- Startup does fast digest verification of sealed segments (cheap) and full
  decompression only on explicit `aegis audit verify --full`.
- The audit directory gains `segments/` and `events-v2.jsonl.manifest`;
  backups must preserve the active stream, both checkpoint sidecars, the
  manifest, and the key together.
- The `max_store_mib` ceiling now bounds the whole history (sealed plus
  active) as observed at startup, matching its documented meaning.
- Operators of forensic deployments keep `retain_segments = 0` and monitor
  with `aegis audit status`; desktop users can set a small retention with
  an export target and never think about the ceiling again.
