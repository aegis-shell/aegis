# ADR-0109: Module-first security and presentation-identity boundaries

- Status: Accepted
- Date: 2026-08-04

## Context

The workspace separated authority policy, audit persistence, presentation
identity, and VRM rendering into four library crates. The separation made
each responsibility easy to name, but it did not create four independent
dependency or delivery boundaries.

Authority and audit are part of the same trusted security path. The generic
audit store has no independent consumer outside the IPC journal.
Presentation identity depends unconditionally on the VRM renderer, while
every current portrait host also imports both crates directly. The extra
package boundary therefore does not isolate the native scene-graph dependency
or hide the rendering backend.

Presentation identity is also distinct from authenticated Actor identity. A
principal answers who may act. A presentation identity describes the local
user-facing profile shown across shell surfaces: account defaults, a chosen
display name, still or VRM portrait content, and associated motion behavior.
The system account is a default source for that profile, not its security
authority.

This decision amends the crate-boundary portions of
[ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md),
[ADR-0104](0104-actor-sessions-resource-grants-and-accessibility-adapter.md),
and
[ADR-0106](0106-shared-identity-portrait-contract-and-vrm-renderer-boundary.md).
Their authority, audit-integrity, portrait-selection, transactional reload,
and caller-owned composition decisions remain in force.

## Decision

Internal code starts as a module. A separate crate requires at least one
concrete boundary: an independently shipped process or package, meaningful
dependency isolation, a native or unsafe integration seam, prevention of a
dependency cycle, or multiple independently built consumers of a stable
contract.

Merge `aegis-authority` and `aegis-audit` into `aegis-security`. Preserve
their separation as the explicit `authority` and `audit` modules. The crate
owns transport-neutral authorization policy and integrity-checked audit
mechanisms; it does not absorb IPC framing, Wayland enforcement, semantic
tree validation, rendering, or Agent reasoning.

Merge the `aegis-avatar` implementation into the `portrait` module of
`aegis-identity`. The VRM parser, motion library, skinning, and offscreen
renderer become an internal portrait backend. `aegis-identity` remains the
shared presentation-profile boundary for the lock screen, command panel, and
future consumers. It exposes content and behavior without owning the host's
layout or chrome.

Keep `aegis-design` data-only. It owns semantic portrait roles, frame colors,
rings, and fallback styling. Presentation hosts own size, placement, camera
composition, and the interaction that requests a motion. Security principals
remain in `aegis-security::authority` and are never inferred from presentation
profile fields.

This decision establishes ownership for future personalized display names
and behavior preferences but does not select a persistence schema for them.
That schema requires a separate decision when its requirements are known.

## Alternatives

- **Keep one crate per named mechanism.** Rejected because module boundaries
  provide the same source organization without adding package topology where
  no independent dependency or delivery boundary exists.
- **Move the VRM renderer into `aegis-design`.** Rejected because it would
  make every design-token consumer depend on Flux scene-graph and native
  rendering libraries, violating the data-only design-system contract.
- **Move presentation identity into `aegis-shell`.** Rejected because the
  standalone lock screen consumes the same profile and portrait contract.
  Depending on the compositor chrome host would broaden its dependency graph
  and confuse presentation data with one host implementation.
- **Create a general personalization crate now.** Rejected because theme,
  device, input, and application preferences already have separate owners.
  The current evidence supports a presentation-identity profile, not a new
  settings umbrella.

## Consequences

- The workspace loses two library packages while retaining explicit module
  ownership and focused unit tests.
- IPC and compositor code use `aegis-security::authority` and
  `aegis-security::audit`, making the security domain visible at call sites.
- Portrait consumers depend only on `aegis-identity`; the VRM backend no
  longer leaks through their manifests or public type paths.
- `aegis-design` stays lightweight for chrome that never presents a portrait.
- Future presentation-profile configuration must distinguish user-selected
  content from authenticated principals and system account facts.
