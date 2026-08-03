# ADR-0104: Actor sessions, exact resource grants, and accessibility adapter

- Status: Accepted
- Date: 2026-08-03

## Context

ADR-0102 made semantic observation a single-use precondition for an Actor
action. ADR-0103 moved the capability language into `aegis-authority` and
renamed the compositor boundary to Interaction Domain. Three production
boundaries remained incomplete.

First, a durable Agent principal and a live execution context were still too
easy to conflate. A disconnected, idle, or expired process must lose every
observation, pending action, resource handle, and provider queue without
deleting the durable identity the user approved.

Second, a capability such as `ReadFile` or `AccessNetworkOrigin` describes an
operation family, not authority over `/path/to/file` or
`https://amazon.com`. A boolean host-network switch or a static path mount
would turn a narrow Actor request into ambient process authority and could
not be made exact by naming it a capability.

Third, compositor-owned window roots are not enough for structured
application interaction. Importing AT-SPI directly into the compositor would
put D-Bus, toolkit behavior, untrusted graph traversal, and potentially
blocking calls inside the display server. Treating adapter data as trusted
would let a provider forge global ids, escape a surface, publish cycles, or
replace another provider's tree.

The in-memory mutation journal also did not by itself provide crash-durable,
integrity-checked security history. Persisting raw action payloads, however,
would create a second credential and private-input database.

## Decision

### Live Actor sessions are distinct from durable principals

Every broker connection receives an `ActorSessionId` bound to its connection
id and optional authenticated `ActorPrincipal`. A session has validated TTL
and idle deadlines plus bounded observation and pending-action quotas. The
registry is capacity bounded. Session ids are allocated independently from
transport connection ids; every effect resolves the explicit
connection-to-session binding instead of deriving either identity from the
other.

Expiry is timer-driven as well as request-driven. Expiry, EOF, or principal
removal removes the live session and cascades revocation into observation
leases, exact resource grants, semantic-provider queues, and compositor
cleanup channels. The durable principal remains only when the termination was
not an explicit principal removal.

### Concrete resources require exact, consumable grants

`ActorCapability` answers what an Actor may request. `ActorResource` names the
exact object:

- one normalized absolute filesystem path plus read or write access;
- one normalized HTTP, HTTPS, WS, or WSS origin;
- one bounded secret-request purpose; or
- one payee, three-letter uppercase currency identifier, and maximum payment
  amount.

The authority kernel issues a random 256-bit opaque `ResourceGrantId` bound
to the Actor session, optional principal, required capability, exact resource,
monotonic TTL, and bounded use count. Consumption must repeat the expected
resource, so a leaked bearer id cannot be widened to another object. Payment
authority always requires fresh exact-resource human confirmation. Secret
prompting consumes a one-use grant before presenting chrome.

Issue, consumption, revocation, and timer expiry are durably journaled before
success is reported. Refused issue/consume/revoke operations use a separate
privacy-preserving event: it records only the operation, capability family,
and resource category, with a fixed refusal category. Exact resources and
bearer ids never enter either the mutation or its refusal reason.

Grant consumption re-resolves the live principal or named-scope ceiling and
requires a live privileged lease. Removing a capability therefore stops an
already-issued handle at the consumer boundary. Revocation remains available
as an authority-reducing operation and is still session/owner bound.

A grant is control-plane authority for a compatible resource consumer. It
does not reconfigure a process namespace and does not make an arbitrary
application voluntarily respect policy. Consequently Interaction Domain
sandboxes cannot express host-network sharing or host user-file mounts. They
always receive an isolated network namespace without resolver configuration
and no user-file mounts. Config schema 2 removes `network`,
`readable_paths`, `writable_paths`, and the legacy `[realm_sandbox]` alias.
A future filesystem portal or origin-enforcing network proxy must consume the
exact grant at its own effect boundary.

### Accessibility is an out-of-process, validated provider

`aegis-semantic` owns the trust seam for complete accessibility-tree
revisions. A provider is authenticated by its broker principal. Node ids are
nonzero and window-local; the compositor forms a global semantic identity as
`{ window, local }`. Publication rejects oversized trees or fields, duplicate
ids, missing parents, cycles, excessive depth, geometry outside the owning
surface, stale revisions, and provider takeover.

`aegis-atspi` owns AT-SPI and toolkit calls in a separate process. In a
production direct session the compositor supervises it and provisions a
compositor-lifetime system principal. The cleartext credential crosses an
inherited stdin pipe and is never written to disk, placed in argv, or exported
in the environment. Nested sessions do not connect the host AT-SPI bus,
because an outer-desktop object could otherwise be confused with an inner
window. The compositor accepts only a non-symlink executable sibling with
matching ownership and a non-group/world-writable binary and parent
directory. It clears the child environment, then forwards only the runtime
directory, session/AT-SPI bus addresses, locale, and logging selector.

The provider does not bind trees by title alone. The compositor captures the
Wayland client's Unix process id from `wl_client` kernel credentials while
the connection is live. A provider-only IPC response supplies that binding;
the adapter resolves the AT-SPI application's Unix process id through the
accessibility D-Bus and requires equality before using an exact non-empty
title to disambiguate a toplevel. General window observation never exposes
the process id. A missing or ambiguous match publishes nothing.
Provider capabilities are reserved for this compositor-provisioned ephemeral
system principal and are excluded from ordinary Agent pairing and durable
administrator registration. The adapter refuses unsupervised startup.

The adapter publishes only changed complete revisions. Password content is
never requested or published. Text actions on password fields and oversized
text are not declared when the adapter cannot prove a complete precondition.
An accessibility target accepts exactly one semantic action. Immediately
before toolkit dispatch the adapter re-reads and hashes the live target's
role, state, bounds, names, bounded value/private fingerprint, and declared
actions. A mismatch, provider failure, full queue, disconnect, or timeout is a
refusal; only provider-confirmed success creates a compositor commit receipt.

This is optimistic validation across a process boundary. AT-SPI does not
offer an atomic compare-and-act transaction, so a toolkit can still change
state in the interval between the final read and its action method. A native
application protocol may offer a stronger transaction; Aegis does not claim
rollback of application-owned business state.

### Security decisions use privacy-minimized durable events

`aegis-audit` owns a generic bounded live projection and owner-only,
append-only JSONL persistence. Each record has a monotonic sequence and a
SHA-256 link to the previous record. Open verifies ownership, permissions,
record bounds, JSON, sequence continuity, and the complete hash chain; append
synchronizes the record before success returns. Creation also synchronizes
the containing directory before the store is accepted, making the initial
filename durable as well as later records.

The production runtime requires a resolvable XDG data directory and never
downgrades this store to memory. Authorization lifecycle events and refusals
are durable before an IPC success or error response. Any append, flush, or
sync failure fail-stops the whole compositor, including when a connection
worker detects it; continuing would create an unaudited authority history.

The compositor does not delete or silently rotate this history. Operators
own quota sizing and lossless archival/export; exhausting the active store is
therefore an intentional fail-stop condition. The unkeyed chain detects
corruption and edits that do not recompute it, but hostile-owner tamper
evidence requires exporting records or signed checkpoints to a separately
administered system.

The IPC journal supplies the domain event payload. It records authentication,
Actor-session, resource-grant, Interaction Domain, settings, command, and
Actor-action decisions plus privacy-minimized high-risk capability use.
It never stores observation or resource bearer ids, notification/typed text,
assigned values, key/button codes, pointer coordinates, screenshot or exact
resource paths, network origins, secret purposes, payees, or amounts.
Commands and semantic actions are reduced to target ids, action shapes,
UTF-8 byte counts, and low-level action counts. Capability events retain an
explicit operation category, such as stream start/stop or idle-inhibit
enable/disable, without the endpoint payload. Routine snapshot polling is not
durably logged.

Framing zeroizes serialized and received JSON buffers. After writing a
response, the server also zeroizes typed secret and newly issued credential
copies. Secret prompt results zeroize on drop and expose an explicit
`zeroize()` operation so consumers can end the sensitive lifetime immediately.
Handshake credential and issued-identity types also zeroize on drop and
redact debug output. Registry and bridge identity files are bounded,
owner-only, non-symlink, single-link regular files whose principals,
credentials, ceilings, and duplicate constraints are validated before use;
credential-digest comparison is constant-time.

The transport admits at most 256 concurrent connections, times out an
incomplete handshake, and gives every writer a bounded 64-item inbox.
Connection request threads backpressure, while compositor event, journal, and
frame producers use non-blocking delivery. A full subscription lane closes
that connection rather than blocking the compositor, growing memory, or
silently losing an event.

Before persistence, every arbitrary refusal string is zeroized and replaced
with a fixed category derived from the privacy-minimized mutation. This keeps
filesystem/toolkit errors from reintroducing paths or text that the mutation
schema deliberately omitted.

The log is durable decision history, not a checkpoint of external Wayland
processes. It can be replayed into a reducer for audit or projection recovery;
it cannot resurrect application-owned state after a compositor restart.

Asynchronous pixel delivery retains a re-resolvable scope binding rather than
a handshake snapshot. Capture delivery and every stream frame re-check the
live principal or named scope, lock/VT gate, lease, operation capability, and
window target allowlist. Revocation during rendering or transfer fails closed.

### Crate ownership

| Crate | Owns | Must not own |
|---|---|---|
| `aegis-authority` | Actor sessions, capabilities, scopes, exact resource grants, observation leases, action validation | Wire framing, toolkit calls, effect commit |
| `aegis-semantic` | Provider/tree validation, window namespacing, semantic routing | D-Bus, IPC, compositor effects |
| `aegis-atspi` | AT-SPI discovery, bounded projection, final live recheck, toolkit action | Compositor policy, rendering, Agent reasoning |
| `aegis-audit` | Generic chained persistence and bounded projection | IPC vocabulary, GUI policy, private raw inputs |
| `aegis-ipc` | Protocol 24 schema and transport adapters | Canonical authority decisions or Wayland effects |
| `aegis-compositor` and `aegis` | Main-loop state, routing, supervision, effect commit, lifecycle cascade | Agent planning and memory |

## Alternatives

### Keep connection id as the only lifetime

Rejected because it has no explicit policy, quota, idle deadline, or reusable
termination cascade, and it conflates transport bookkeeping with an Actor
execution context.

### Put paths and origins directly in capabilities

Rejected because it creates unbounded capability variants, mixes operation
families with resources, and still lacks a consumable authority at the effect
boundary.

### Enable a sandbox's host network after an origin grant

Rejected because a network namespace switch cannot enforce an HTTP origin.
Calling it exact would create a false security boundary.

### Run AT-SPI in the compositor

Rejected because untrusted graph walking and D-Bus/toolkit latency would
expand both the trusted computing base and the compositor failure domain.

### Persist complete action payloads for replay

Rejected because passwords, text, coordinates, paths, and payment details are
not necessary to audit authority decisions and would materially increase the
impact of log disclosure.

## Consequences

IPC protocol 24 is intentionally incompatible with older clients. Config
schema 2 intentionally rejects the removed ambient sandbox fields. Packages
must install `aegis-atspi` beside `aegis`; the compositor locates only a
trusted sibling executable and supervises it for the production session.

Observation and action remain independent. A structured target can be acted
on only when its authenticated provider is live, the Actor session and exact
capabilities remain valid, the observation is fresh, both compositor and
adapter preconditions agree, and the provider confirms success. Failure at
any layer is visible as a refused, privacy-minimized event rather than a
partial or optimistic success receipt.

Filesystem and exact-origin grants now have a safe authority representation,
but they deliberately produce no ambient access by themselves. New resource
services must integrate the consume operation before claiming end-to-end file
or network support.
