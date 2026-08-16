# ADR-0125: Agent primitive families and the shared agent client layer

- Status: Accepted
- Date: 2026-08-16

## Context

Aegis exposes three agent surfaces over the compositor's capability
broker: the MCP bridge (`aegis-mcp`), the AT-SPI accessibility adapter
(`aegis-atspi`), and the owner CLI (`aegis-commands`). Each consumer
re-implements the same client plumbing: credential storage or injection,
the pairing handshake discipline, post-handshake timeout setup,
per-instance state recovery, and observation-lease retention. A fourth
consumer would copy the plumbing again.

The protocol's mutation surface has grown into four disjoint paths with
different guarantees. `Do` commands are fire-and-forget: the reply
acknowledges queuing, and completion is observable only by polling the
journal. Interaction Domain actions, settings actions, and actor actions
are synchronous and return authoritative receipts, each with its own
precondition currency (`expected_revision` or a single-use observation
token). Principal-bound agent connections are refused `Subscribe` and
`SubscribeJournal`, so agents must poll `GetJournal` to verify that
queued commands landed.

Two pressures follow. First, new agent capabilities arrive as new
top-level request shapes instead of new combinations of a small
vocabulary, so every addition touches the schema, the dispatch table,
the runtime channel, and the main-loop drain independently. Second,
agent consumers pay round trips and poll loops for guarantees the
compositor already maintains internally.

The security model constrains any redesign: pairing prompts, runtime
grants, and the audit journal are all keyed on named operations
(`ActorCapability`) crossed with object axes (`ActorScope`). A generic
untyped mutation verb would make authorization prompts meaningless and
is rejected as a direction.

## Decision

Adopt two complementary layers.

First, factor the agent client plumbing into a shared crate,
`aegis-agent`. It owns credential sources (a paired, on-disk identity
store and a launcher-injected stdin credential), the pairing handshake
discipline (generous handshake timeout for the interactive prompt,
issued-credential persist-or-confirm, post-handshake I/O timeout),
per-instance state stores with recovery locks, managed Interaction
Domain lifecycle and crash recovery, and observation-lease retention.
`aegis-mcp` and `aegis-atspi` consume it. Consumers keep their own
surfaces: the MCP tool catalog and the AT-SPI scan/dispatch loop are
unchanged contracts.

Second, define four **primitive families** as the canonical
classification of the agent-facing protocol, and close the two gaps
that the classification exposes:

- **Observe** covers queries and token-issuing observation. A new
  `Observe` request returns a consistent multi-domain snapshot (windows,
  workspaces, outputs, Interaction Domains, journal cursor) in one
  round trip, replacing per-call fan-out.
- **Transact** covers authorized state mutation. A new `Transact`
  request carries an optional journal-cursor precondition plus an
  ordered batch of mutation ops drawn from the existing `Command`
  vocabulary (window, workspace, notification, and launch operations).
  The compositor preflights authorization and validation for every op
  before applying any, applies the batch in order on the main loop
  through the same chokepoint as `Do`, and returns a per-op receipt
  with journal sequence numbers. `Do` remains for compatibility.
- **Inject** covers input delivery. The existing `InjectInput`
  (fire-and-forget, physical seat) and `ActInInteractionDomain`
  (observation-token precondition, domain seat) are the two inject
  verbs; no new wire verb is added.
- **Subscribe** covers pushed invalidations. Principal-bound agent
  connections may now `Subscribe` and `SubscribeJournal`; delivery is
  filtered through the connection's live scope (coarse events gated by
  the matching observe operation, journal entries passed through the
  existing subject/scope journal filter) and lanes remain fail-closed.

The capability model is unchanged: authorization stays at named
`ActorCapability` operations crossed with `ActorScope` object axes, and
each op in a `Transact` batch is authorized exactly as the equivalent
`Command`. New capabilities should arrive as new ops inside a family,
not as new top-level request shapes. All wire changes are additive
under the protocol's versioning discipline (serde defaults, tagged
enums, no renames).

High-level agent surfaces stay high-level. The MCP bridge keeps its
named tools and compiles them down to the primitives; the primitives
are not exposed to model clients as raw verbs.

## Alternatives

- **A single generic `mutate` verb with an untyped payload.** Rejected:
  authorization prompts, the pairing capability picker, and the audit
  journal all require named operations; a generic verb collapses the
  fail-closed scope model.
- **Rewrite `Command` handling onto `Transact` and remove `Do`.**
  Rejected for now: the protocol's additive versioning discipline keeps
  older peers working, and `Do` costs little to retain. New consumers
  use `Transact`; internal desugaring may follow in a later protocol
  revision.
- **Expose the primitive families directly as MCP tools.** Rejected:
  named, well-described tools are the model-facing contract that makes
  agents reliable; the primitives are the layer beneath that contract.
- **A shared agent daemon process multiplexing all agent surfaces.**
  Deferred: a shared crate delivers the same deduplication while
  keeping process isolation and the compositor's existing supervision;
  the crate leaves a daemon possible later.

## Consequences

- New agent surfaces (a second accessibility provider, another bridge)
  are built from the shared client crate instead of copied plumbing.
- Most new agent capabilities become new `Transact` op variants or new
  `Observe` fields — one schema site, one authorization site, one
  main-loop application site — instead of new request pipelines.
- Agents can verify mutations from `Transact` receipts and pushed
  journal events instead of polling `GetJournal`.
- The protocol moves to version 28; older clients are unaffected.
- `docs/reference/ipc.md` and the glossary gain the primitive-family
  vocabulary; `aegis-mcp` and `aegis-atspi` documentation must reflect
  the shared client dependency.
- Follow-up work: migrate the CLI's multi-call flows to `Observe` and
  `Transact` where they reduce round trips; evaluate desugaring `Do`
  internally in a later protocol revision.
