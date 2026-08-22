# ADR-0136: Authenticated bounded audit replay and storage guards

- Status: Accepted
- Date: 2026-08-22

## Context

The durable authority history introduced by ADR-0104 was reopened by reading,
decoding, and hash-verifying every JSONL record synchronously. The production
runtime performed that work before starting IPC and presenting its first
frame. It also accumulated all decoded records in a temporary vector before
retaining only the newest 4096 entries for the live projection.

A 1.22 GiB store containing 2.66 million records therefore added roughly nine
seconds to session startup and transiently required gigabytes of memory. The
delay grew linearly with retained history even though live consumers can only
observe the bounded projection. Routine AT-SPI polling caused this particular
store's abnormal growth and is removed by ADR-0135, but an authority audit
design must remain safe and responsive when genuine decisions accumulate.

Unbounded local retention also allowed the audit file to consume the root
filesystem before the next durable append correctly fail-stopped the
compositor. Infinite lossless retention and finite local storage cannot both
be guaranteed by the compositor; deleting evidence automatically is not an
acceptable reconciliation.

## Decision

The event store maintains an authenticated replay checkpoint beside the
append-only JSONL stream. The checkpoint records the durable byte length,
next sequence and tail hash, plus the byte offset, sequence, and previous hash
needed to reconstruct the newest live-projection records. A random 256-bit
owner-only local key authenticates the checkpoint with HMAC-SHA-256. The key
and checkpoint are regular, single-link, non-symlink files with owner-only
permissions.

On reopen, the startup-critical path authenticates the checkpoint and
hash-verifies the live projection from its recorded boundary through the
uncheckpointed tail. Memory remains bounded by the live projection capacity;
replay never accumulates the complete history. The uncheckpointed tail is
bounded by both byte and event intervals.

An authenticated checkpoint does not replace complete verification. When the
projection starts after genesis, a dedicated worker rereads the complete
chain from genesis and proves that it reaches the authenticated projection
boundary. Startup and read-only projection access may continue while that
scan runs, but every append waits at a verification gate. A failed historical
scan permanently refuses new records for that store instance and therefore
cannot extend a corrupt authority history. Clean shutdown cancels and joins a
pending verifier at a record boundary rather than waiting for the entire
history.

The first open of an existing store without a checkpoint performs one
complete streaming verification synchronously and atomically establishes the
checkpoint. This one-time migration cost is required to create a trusted
anchor from an uncheckpointed history. An invalid or unauthenticated
checkpoint fails closed; operators can preserve the JSONL stream and move the
checkpoint and key aside together to force a complete rebuild.

An append synchronizes the JSONL record before publishing a replacement
checkpoint. Checkpoint replacement uses a new owner-only file, file sync,
atomic rename, and containing-directory sync. A crash can therefore leave a
valid older checkpoint and a bounded extra tail, but cannot advertise a
checkpoint newer than the durable event stream.

The store enforces two pre-write guards:

- a hard active-stream byte ceiling, 2048 MiB by default; and
- a filesystem free-space reserve, 512 MiB by default.

Crossing either guard refuses the next event before writing bytes. The
compositor retains ADR-0104's fail-stop behavior for any durable-audit
failure. It never truncates, overwrites, or silently rotates an event. An
operator must losslessly archive the complete chain and intentionally start a
new active stream before the configured ceiling is reached.

The startup-only `[audit]` configuration table exposes `max_store_mib`,
`min_free_mib`, and `checkpoint_interval_mib`. Defaults bound the ordinary
case without pretending to replace deployment-specific capacity planning or
archival.

The local HMAC protects the startup optimization against accidental or blind
checkpoint substitution under ADR-0104's existing owner trust boundary. It
does not claim hostile-owner tamper evidence. Deployments requiring that
property must continuously export records or independently signed anchors to
a separately administered system.

## Alternatives

**Continue complete synchronous replay with a streaming projection.** This
would fix peak memory but preserve startup time proportional to all retained
history.

**Trust an unsigned offset and tail hash.** Rejected because editing the
sidecar could skip arbitrary history on the startup path. Authenticating the
anchor is cheap and keeps checkpoint corruption fail-closed.

**Treat the checkpoint as complete verification.** Rejected because the
older prefix would never be reread after checkpoint creation. Background
verification plus the append gate removes that blind spot without blocking
the first frame.

**Automatically rotate, truncate, or delete old records.** Rejected because
the compositor cannot silently discard the authority history it promises to
retain. A hard pre-write guard protects the host while making the need for
operator archival explicit.

**Disable persistence or defer failed writes.** Rejected because continuing
after an authority decision that could not be recorded violates ADR-0104's
fail-closed contract.

## Consequences

- Normal restarts do work proportional to the bounded live projection and
  uncheckpointed tail instead of the full history. A legacy store pays one
  complete scan on its first upgraded start.
- Peak replay memory is bounded by 4096 decoded events plus one bounded input
  record, independent of total history length.
- Complete-chain corruption still prevents the history from being extended,
  although detection may occur after the first frame when an authenticated
  checkpoint exists.
- The audit directory gains `events-v2.jsonl.checkpoint` and
  `events-v2.jsonl.key`. They form local verification state; backing up or
  moving an active store should preserve all three files together.
- Capacity exhaustion is predictable and leaves the configured filesystem
  reserve intact, but deployments still need monitoring and lossless
  archival. The compositor cannot manufacture infinite local retention.
