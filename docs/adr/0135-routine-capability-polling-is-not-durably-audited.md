# ADR-0135: Routine capability polling is not durably audited

- Status: Accepted
- Date: 2026-08-22

## Context

The durable audit store
(`$XDG_DATA_HOME/aegis/audit/events-v2.jsonl`, ADR-0104) fail-stops the
compositor when an append cannot be persisted. A full session terminated
that way: the root filesystem reached 100%, the next audit append returned
`ENOSPC`, and the compositor aborted by design.

The store did not fill from authority decisions. 99.5% of its 2.66 million
records came from two heartbeat loops of the supervised AT-SPI adapter:

- `NextAccessibilityAction` long-polls every 100 ms; every timed-out poll
  was journaled as `CapabilityUse { DispatchAccessibilityAction, Await }`
  (~10 events/s).
- `GetAccessibilityWindows` re-queries bindings every 750 ms; every
  successful query was journaled as
  `CapabilityUse { PublishAccessibilityTree, Observe }` (~1.3 events/s).

At ~492 bytes per hash-chained record that is ~5.4 KiB/s — roughly
460 MiB/day — of records that each say "the adapter asked for work, found
none, and changed nothing." Neither event records a decision, an authority
transition, or a refusal; they are transport keep-alive at IPC cadence
written at fsync durability.

[ADR-0033](0033-mutation-journal.md) already draws the right line for the
live journal: it reports what the compositor *decided*, and "routine
snapshot polling is not durably logged" is the standing description of the
durable store's vocabulary. The dispatch code did not honor that boundary
for these two endpoints.

## Decision

A request is durably audited when the compositor decides something —
applies, delivers, refuses, or transitions authority. A request whose
successful outcome is "nothing happened" is connection maintenance and is
not durably audited, no matter how often the client repeats it.

Concretely, for the two polling endpoints:

- `NextAccessibilityAction` journals the `Await` capability use when it
  **delivers** an action (`Ok(Some)`) or **refuses** (authorization gate or
  handler error). A timed-out poll (`Ok(None)`) journals nothing.
- `GetAccessibilityWindows` journals the `Observe` capability use only on
  **refusal** (an out-of-scope process attempting to bind). A successful
  scan query journals nothing; the tree revisions it feeds are already
  audited at `PublishAccessibilityTree`.

Refusals stay auditable at full fidelity: a refusal is a decision and a
potential security signal, and it is bounded by attacker behavior, not by
wall-clock cadence. Steady-state throughput is therefore independent of
session length: a quiet session writes zero durable records, and the store
grows only when authority is actually exercised.

## Alternatives

**Silently rotate, truncate, or delete `events-v2.jsonl`.** Rejected: the
store is a hash-chained, fail-closed authority history. Lossless archival is
operator-owned, and any automatic discard would rewrite or remove the chain
the design exists to protect. ADR-0136 adds pre-write capacity and free-space
guards without discarding a record.

**Downgrade the append to non-durable (drop the fsync) for heartbeats.**
Rejected: it preserves the unbounded growth, only slower, and silently
weakens the durability guarantee the audit vocabulary promises.

**Slow the adapter down (longer poll/scan intervals).** Rejected as the
primary fix: latency of semantic action dispatch and tree freshness would
regress, and the growth would remain linear in session time — merely with a
smaller constant.

**Keep journaling but aggregate heartbeats into periodic summaries.**
Rejected: the aggregated records would still grow with time, would need
their own compaction story, and would carry no decision content. Recording
"nothing happened" less often is still recording nothing.

## Consequences

- The audit store grows only with real decisions; a typical interactive
  session accumulates kilobytes, not gigabytes. The ENOSPC fail-stop
  remains exactly as specified for genuine quota exhaustion.
- The journal's `Await`/`Observe` capability-use vocabulary is unchanged;
  only its frequency contract narrows. Consumers replaying the journal see
  delivery and refusal events as before.
- This is the durable-store boundary, not a change to the live ring
  broadcast: `persist_and_broadcast` callers are unaffected, and live
  subscribers never saw per-poll entries because dispatch-side capability
  audits are not broadcast.
- `aegis-ipc` regression tests pin the boundary: timed-out polls and
  successful scan queries produce no audit calls, while delivery, handler
  refusal, and authorization refusal still do.
- Operators remain responsible for archival (ADR-0104, ADR-0136). A
  deployment that genuinely makes authority decisions at high volume still
  needs an export policy; that demand is now visible in the store's contents
  instead of being masked by heartbeats.
