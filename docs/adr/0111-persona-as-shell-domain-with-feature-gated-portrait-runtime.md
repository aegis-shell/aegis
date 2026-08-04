# ADR-0111: Persona as a shell domain with a feature-gated portrait runtime

- Status: Accepted
- Date: 2026-08-04

## Context

[ADR-0106](0106-shared-identity-portrait-contract-and-vrm-renderer-boundary.md)
introduced one shared account and portrait contract. [ADR-0109](0109-module-first-security-and-presentation-identity-boundaries.md)
then folded the renderer into `aegis-identity` while retaining that contract as
a standalone crate. The package isolated image decoding, filesystem watching,
and the VRM scene-graph dependency from unrelated shell consumers.

The remaining consumers are the lock screen and command panel. Both are Aegis
presentation surfaces, ship in the same workspace, and change with the shell's
profile and portrait conventions. The package therefore has no independent
release, process, authority, or reuse boundary. Dependency weight alone does
not justify a crate when Cargo features can preserve the same isolation.

The term “identity” also overlaps the security model. Personalized display
name, portrait, and motion behavior describe presentation content; they do not
identify or authorize an Actor.

## Decision

Move the presentation-profile domain into `aegis-shell::persona` and remove
the standalone `aegis-identity` package.

The lightweight `aegis_shell::persona::Profile` contract is available with
the shell's default features. It exposes account-backed presentation defaults
that callers may replace with personalized values. The optional `persona`
feature exposes the portrait source policy, still-image normalization, VRM and
motion backend, and transactional filesystem watcher. `aegis-lock` and
`aegis-command-panel` enable that feature explicitly.

Presentation hosts continue to own portrait geometry, camera composition,
fallback visuals, and motion triggers. `aegis-design` owns only role-based
style data. Authenticated principals, authorization, credentials, and session
proof remain outside the persona module in `aegis-security` or the lock
authentication boundary.

The module and its tests are the executable Aegis persona convention. The
canonical paths and reload behavior remain documented as reference material;
this decision does not introduce a persistence schema for future custom names
or behavior settings.

This decision amends only the standalone-package placement in ADR-0106 and
ADR-0109. Their shared source, rendering, reload, host-composition, and
presentation-versus-security boundaries remain in force.

## Alternatives

### Keep `aegis-identity`

Rejected because it has only same-product presentation consumers and no
independent lifecycle. Its dependency boundary is expressed more precisely by
an optional feature.

### Create `aegis-persona`

Rejected because changing the noun does not create an independent package
boundary. The domain still belongs to the shell product.

### Enable the portrait runtime in every shell build

Rejected because shell consumers that need only the chrome contract, locale,
design integration, or lightweight profile should not compile or link image,
watcher, and scene-graph dependencies.

### Move persona data into `aegis-design` or `aegis-model`

Rejected because design remains data-only and generic shared model types do
not own account lookup, filesystem conventions, GPU resources, or animation.

## Consequences

The workspace has one fewer package and one explicit presentation-domain
owner. Call sites distinguish `persona::Profile` from security identity, and
lightweight shell consumers do not acquire the portrait runtime by default.

The lock screen now depends on `aegis-shell` for persona content as well as on
its existing standalone presentation contracts. If persona later gains an
independent process, release cadence, or external consumers that cannot depend
on the shell contract, that evidence can justify extracting a crate in a new
decision.
