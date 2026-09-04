# Tessera Design Language

The design language is the shared product contract for every Tessera-owned UI
surface: compositor chrome, first-party system applications, the Dock, HUD,
launcher, command panel, menus, and toasts. Components built by different
crates must read as one product, so the rules in this directory are normative
for contributor-written Tessera UI.

The language takes its material cues from Apple's Liquid Glass (WWDC 2025)
and adapts them to a Wayland compositor: chrome is a floating functional
layer above client content, defined by bent light rather than by painted
fills.

## System map

The design system is layered from low-level decisions to engineering
enforcement. A higher layer may combine lower layers, but it must not invent
private replacements for an adopted lower-layer contract.

| ID | Layer | Question | Entry point |
|----|-------|----------|-------------|
| 1 | Foundations | Which visual and sensory values are available? | [Foundations](foundations/index.md) |
| 2 | Components | Which reusable controls and containers use those values? | [Components](components/index.md) |
| 3 | Patterns | How do components behave across complete interactions? | [Patterns](patterns/index.md) |
| 4 | Guidelines | Which cross-cutting quality rules constrain every layer? | [Guidelines](guidelines/index.md) |
| 5 | Tooling | How are contracts generated, demonstrated, and measured? | [Tooling](tooling/index.md) |

Product specifications such as Liquid Glass and the command panel apply
this shared system to a particular surface. They remain normative, but they
do not replace the system-wide contracts above.

## Maturity

Every system page carries one maturity label:

| Status | Meaning |
|--------|---------|
| Adopted | The contract has a shared implementation and is normative for new work. |
| Partial | A shared contract exists, but coverage or enforcement is incomplete. |
| Draft | The page reserves scope and review criteria; it is not yet an implementation contract. |

An adopted rule changes through code, tests, and documentation in the same
change. A draft becomes adopted only when its ownership and enforcement are
implemented; deleting a local literal is not sufficient by itself.

## Principles

1. **Content first.** Chrome defers to the client's pixels. Materials bend,
   tint, and separate; they do not cover.
2. **Lensing, not scattering.** Separation comes from refraction and
   sculpted light. Broad blurs and milky fills are fallbacks, not the
   product look.
3. **Concentric geometry.** Shapes nest with continuous rounded corners.
   A body's corner radius follows its component token, never a magic number.
4. **Adaptation over fixed appearance.** Materials respond to the content
   beneath them — luminance, motion, and context — instead of carrying one
   fixed light or dark look.
5. **Motion is part of the material.** Springs, merges, and reveals are
   properties of the material itself, choreographed with its optics.
6. **Restraint.** Liquid Glass marks the floating control layer. Content
   surfaces use quiet fills. Painted borders and heavy tints do not belong
   on glass.
7. **One body, one hierarchy.** Interaction emphasis changes the optics and
   content hierarchy inside an existing glass body. Hover and selection never
   introduce a nested glass body or a structural accent outline.

## Product specifications

| Page | Purpose |
|------|---------|
| [Liquid Glass](liquid-glass.md) | The analytic glass material: optical model, lighting, adaptivity, parameters, and usage rules |
| [Surfaces](surfaces.md) | Material inventory for chrome surfaces with tokens and selection rules |
| [Command Panel](command-panel.md) | The three-surface HUD cluster: header band, tabbed main panel, side column, and motion |
| [Persona Portraits](persona.md) | Profile and portrait-content boundaries plus role-based frame styles |

## Engineering ownership

| Layer | Owner |
|------|-------|
| Generic drawing, widgets, layout, and input mechanics | Optics `lens` and `flux` |
| Product tokens, palettes, themes, and material recipes | `tessera-design` |
| Reusable product composition, geometry, and motion vocabulary | `tessera-ui` |
| Domain state and interaction intent | The owning chrome or application crate |
| Compositor integration and pixel-effect plumbing | The compositor and Optics boundary defined by ADR-0139 |

New shared design values use semantic names. Reusable components consume
those values instead of copying literals. Component-specific values remain
local until repeated use demonstrates a product-wide role.

## Related Documentation

- [Design system crate decision](../../adr/0046-design-system-crate.md)
- [Composite component library decision](../../adr/0132-tessera-ui-composite-component-library.md)
- [Animation and effect placement](../../adr/0139-animation-effect-placement.md)
- [Liquid glass lens model and full-resolution capture](../../adr/0094-liquid-glass-lens-model-and-full-resolution-capture.md)
- [Single-body liquid-glass interaction focus](../../adr/0105-single-body-liquid-glass-interaction-focus.md)
- [Glass material roles and region-level backdrop adaptation](../../adr/0120-glass-material-roles-and-region-level-backdrop-adaptation.md)
- [Explicit offscreen composition DAG](../../adr/0143-explicit-offscreen-composition-dag.md)
