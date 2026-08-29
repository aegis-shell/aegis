# ADR-0144: Product-semantic design vocabulary

- Status: Accepted
- Date: 2026-08-25

## Context

The command panel's original visual exploration used Sword Art Online as an
inspiration source. ADR-0081 removed that source name from the product surface
but deliberately retained `Sao` as the internal name of a token set.
ADR-0114 later preserved those tokens while replacing the command panel's
palette.

That distinction did not hold. An inspiration-source name in a public Rust
type, theme factory, material factory, test, or current design document still
looks like a supported design-system concept. It gives an obsolete experiment
API permanence, obscures the role of each value, and makes later components
more likely to depend on a visual reference instead of the product's own
semantics.

## Decision

Aegis design-system vocabulary names product roles and behavior, not external
inspiration sources or visual-exploration codenames.

- Remove the retired `Sao` token type and its `themes::sao`,
  `themes::sao_muted`, and `materials::sao_panel` factories. Do not provide
  compatibility aliases.
- Use semantic names such as background, surface, selection, separator, and
  critical for active tokens, APIs, tests, and current design documentation.
- Inspiration sources may guide exploration, but their names do not become
  active source identifiers or supported design-system roles.
- Historical ADRs and released CHANGELOG entries retain their original wording
  so the decision trail remains accurate.

This decision supersedes Decision 3 of ADR-0081 and the `Sao` token-retention
clause of ADR-0114. The other decisions in those records remain in force.

## Alternatives

- **Keep the internal codename but stop using it.** Rejected: an exported,
  tested API advertises continued support even when no component consumes it.
- **Rename the palette one-for-one.** Rejected: the palette is obsolete, so a
  mechanical rename would retain roles with no current product meaning.
- **Keep deprecated aliases.** Rejected: Aegis is pre-1.0, there are no active
  consumers, and aliases would preserve exactly the vocabulary this decision
  removes.

## Consequences

- Active code and current design documentation no longer contain the retired
  inspiration-specific vocabulary.
- Components consume scheme-adaptive semantic roles from `aegis-design`.
- Removing the exported Rust items is an intentional pre-1.0 API break.
- Repository searches can still find the old term in immutable historical ADRs
  and CHANGELOG entries; those matches describe history rather than supported
  code or design guidance.
