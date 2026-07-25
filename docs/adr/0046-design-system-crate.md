# ADR-0046: Product design system as a data-only crate

- Status: Accepted
- Date: 2026-07-22

## Context

[ADR-0021](0021-chrome-component-trait.md) made compositor chrome pluggable,
and [ADR-0044](0044-dock-and-control-center-crates.md) plus
[ADR-0045](0045-statusbar-crate-and-sni-tray.md) promoted major surfaces into
independent crates. That separation exposed product styling that had no
shared owner. Menu themes, frosted-glass materials, card surfaces, and
related visual values were repeated across `aegis-shell`, `aegis-dock`,
`aegis-ctl-center`, and `aegis-statusbar`. ADR-0045 already identified the
duplicated popover material as follow-up work.

The responsibility boundary in
[ADR-0001](0001-scope-and-responsibility-boundary.md) assigns generic UI
capabilities to lens. Product appearance is a different concern: putting ass
colors and materials in lens would couple a general UI engine to one shell,
while leaving them in individual components permits silent visual drift.

The repeated code does not yet establish a shared widget boundary. The
menus have different data and action models, and most repeated values are
presentation data rather than common retained state or interaction behavior.

## Decision

Create `aegis-design`, an internal workspace crate that depends only on lens.
It owns semantic product tokens and pure factories that return lens `Theme`,
`OverlayOpts`, and `LayoutOpts` values.

`aegis-design` is data-only. It never receives a lens `Frame` or `Input`, keeps
component state, or emits application intents. Its API names semantic roles
such as menu text, popover surface, and application accent instead of
exporting numbered palette values or arbitrary parameter bags.

Do not create an `ass-widgets` crate at this stage. Generic controls,
hit-testing, and popover placement belong in lens. Product components and
their behavior stay in their existing owner crates. A future shared widget
crate requires repeated structure, state, and interaction across at least
three product surfaces, plus evidence that the capability is too
product-specific for lens.

Adopt the design system incrementally. The first migration preserves the
existing pixels while centralizing the exact duplicate menu theme and
popover material, the Dock panel material, and the Control Center base theme
and card material. Component-specific layout and animation values remain
local until they become genuine product-wide tokens.

## Alternatives

- **Keep styling private to every component.** Rejected: exact duplicates
  already exist across crate boundaries, and changing one copy does not
  update the others.
- **Put product styling in `aegis-shell`.** Rejected: `aegis-shell` is the chrome
  host and component contract. A dedicated dependency keeps appearance
  reusable without making the contract crate the owner of every visual
  policy.
- **Create `ass-widgets` together with the design crate.** Rejected: current
  reuse is presentation data, not common widget state and interaction. This
  would establish a second UI toolkit before its API is known.
- **Put ass tokens and materials in lens.** Rejected: lens owns generic UI
  capabilities, not one product's visual identity.

## Consequences

- The dependency direction is `lens` ← `aegis-design` ← the shell and
  component crates. `aegis-design` has no dependency on `aegis-shell`,
  `aegis-core`, or a component crate.
- Shared visual changes have one semantic owner and can be tested without
  rendering a business component.
- Generic helper duplication such as rectangle hit-testing and popover
  placement is not hidden in the design crate. It remains follow-up work for
  lens.
- The default appearance is compile-time product policy. A future
  configurable appearance can add another `Design` snapshot without
  changing the material and theme factory boundary.
- A shared widget crate remains a measured future extraction rather than a
  default destination for reused code.
