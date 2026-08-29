# Token Pipeline

Status: **Partial**.

The current token source is handwritten Rust in `aegis-design`. Its
`build.rs` only propagates native-library runtime paths for tests; it does not
generate design tokens. No Style Dictionary or parallel JSON token source is
authoritative today.

## Current flow

| Stage | Owner | Contract |
|------|-------|----------|
| Color definition | `aegis-design::colors` | Semantic color roles and literal values for every appearance |
| Non-color definition | `aegis-design::tokens` | Shape, material, typography, and presentation policy |
| Composition | `aegis-design::themes` and `materials` | Pure factories that convert roles into `lens` options |
| Reuse | `aegis-ui` | Shared component metrics and composition |
| Consumption | Chrome and first-party application crates | Resolved design snapshot; no local replacement palette |
| Verification | Rust unit tests | Exact values, literal placement, scheme invariance, and monotonic scales |

## Token model

Token names describe role, not raw value or component appearance. Prefer
`application_text` or `glass_panel` over `gray_100` or `rounded_large`.
Foundational tokens may feed semantic roles internally, but reusable callers
consume the semantic layer.

A token definition records:

- stable semantic name and type;
- owning foundation and intended consumers;
- light, dark, and accessibility-variant behavior;
- logical versus physical unit;
- invariants shared across appearances;
- deprecation or migration path when replaced.

## Change workflow

1. Identify a repeated semantic role rather than a repeated number.
2. Add or change the typed token and its relationship tests.
3. Update theme or material factories that translate the role.
4. Migrate all in-scope consumers in the same change.
5. Update the relevant foundation and component documentation.
6. Review both appearances, supported scales, and accessibility states.

## Generation boundary

A generated pipeline becomes useful when another runtime or design tool needs
the same contract. At that point one neutral source may feed Rust through
`build.rs` and external formats through a tool such as Style Dictionary.
Adopting that flow requires a separate architectural decision that defines
the sole editable source, deterministic output, schema validation, and
checked-in versus build-only artifacts.

Until then, adding JSON beside Rust would create two sources of truth and is
not allowed. `build.rs` must also remain deterministic, offline, and free of
machine-specific output.

## Adoption work

- Inventory repeated visual literals outside `aegis-design` and classify them
  as shared roles or owner-local values.
- Add deprecation guidance for renamed tokens.
- Define export requirements before selecting a cross-platform token format.
- Extend the color-literal guard from `aegis-design` to consuming UI crates
  with explicit exceptions for source-owned media and protocol values.

See [ADR-0046](../../../adr/0046-design-system-crate.md).
