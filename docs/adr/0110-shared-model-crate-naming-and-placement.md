# ADR-0110: Shared model crate naming and placement

- Status: Accepted
- Date: 2026-08-04

## Context

The workspace's lowest shared library was named `aegis-core`. It contains
effect-free state, value types, and deterministic transformations shared by
the compositor server, backend, renderer, shell, configuration, IPC, and
external companion components.

The crate boundary prevents dependency cycles and gives many independently
built consumers one stable contract. The word “core,” however, describes its
position in the dependency graph rather than its ownership. The accompanying
placement rule defined the crate negatively: code without flux, lens, or
Wayland dependencies belonged there. That rule could turn the package into a
default home for unrelated pure helpers even when they have one clear owner.

[ADR-0109](0109-module-first-security-and-presentation-identity-boundaries.md)
requires a concrete reason for a package boundary. This crate satisfies that
rule through its many consumers and its role in preventing cycles; the
problem is its name and placement contract, not its existence.

## Decision

Rename the package to `aegis-model`, the Rust crate path to `aegis_model`, and
the workspace directory to `crates/aegis-model`.

`aegis-model` owns state, value types, and deterministic invariants that are
shared across Aegis components and do not depend on concrete I/O, transport,
renderer, toolkit, or process mechanisms. Effect-free code with one clear
owner remains in that owner's module. Absence of a heavyweight dependency is
necessary but not sufficient for entering the shared model.

The `aegis` package remains the executable composition root. Its existing
`runtime` module continues to own process assembly, I/O coordination, and
effect commit; this decision neither merges the model into the executable nor
creates a separate runtime package.

The former package and Rust names are not retained as compatibility aliases.
Workspace crates and source consumers move to the canonical names in one
change. Wire protocols, serialized schemas, CLI commands, and runtime paths
do not change.

## Alternatives

### Keep `aegis-core`

Rejected because “core” does not identify an owned concern and reinforces a
negative catch-all placement rule.

### Merge the model into `aegis`

Rejected because shared crates already depend on the model while `aegis`
depends on those crates. Merging would create cycles or make reusable model
consumers depend on the executable's Wayland, rendering, and process effects.

### Use `aegis-domain`

Rejected because “domain” collides with the narrower Interaction Domain term.

### Use `aegis-types`

Rejected because the crate also owns deterministic layout, transition,
binding, and model-validation behavior.

### Create `aegis-runtime`

Rejected for this change because the runtime has one production consumer and
no independent delivery or dependency boundary. Module organization remains
sufficient under ADR-0109.

## Consequences

Call sites and dependency manifests expose the model responsibility directly.
The positive placement rule makes review of future additions more specific
and keeps component-local pure helpers out of the shared package.

The rename is source-incompatible for downstream Rust consumers, including
compatible companion workspaces. They must update the package name and Rust
path together when adopting this Aegis revision. Historical ADRs and released
changelog entries retain the former name as the terminology in force when
they were written.
