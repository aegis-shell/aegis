# ADR-0105: Single-body Liquid Glass interaction focus

- Status: Accepted
- Date: 2026-08-03

## Context

Dock live previews and the window switcher need to distinguish a hovered or
selected window inside a Liquid Glass panel. The previous blue-white
selection frame read as a painted sticker rather than a property of the
material. Replacing it with a separately rendered glass card would violate
the design-language rule against glass-on-glass, duplicate the rim and
shadow, and make the same interaction look different between components.

The selection treatment must remain visible over arbitrary application
content without borrowing an application accent. It must also preserve real
window pixels as the visual subject, provide immediate pointer feedback, and
move coherently with the existing spring-driven switcher selection.

The Dock had a separate interaction defect: magnification can place open-app
icons below the preview panel with an uncovered visual gap. The interpolated
bridge covered that gap, but the preview hover region did not include the
frozen magnified owner bounds themselves. Pointer ownership could therefore
drop at the icon-to-bridge transition before the user selected a window.

## Decision

Use one optical focus field inside the existing parent glass body for
structural selection. `LiquidGlassRegion` carries at most one
`LiquidGlassFocus`: bounds, corner radius, and strength. The compositor maps
it into the same Optics group as the parent body. The field modifies local
clarity and directional light only; it creates no coverage, silhouette, rim,
shadow, or second material body.

`aegis-design` owns the shared interaction contract. Hover uses a neutral
white wash at alpha 8. Selection uses the optical field at strength 1 plus an
alpha-12 neutral fallback. Nonfocused preview content renders at 80% opacity,
while the focused preview remains at full content opacity. Structural focus
never uses the application accent or a painted border. Dock preview panels
and window switcher panels use the shared 18 px `glass_panel` radius; their
focus cards use the shared `control` radius.

The focus bounds must remain inside a single parent body. A body using
smooth-union merging cannot carry a focus field in the same frame. Bounds,
radius, strength, selected identity, and inactive opacity are part of the
exact backdrop cache key.

Dock hover ownership includes the current magnified bounds of the icon that
opened the preview, the panel, and the interpolated bridge between them. The
preview therefore remains interactive across the entire pointer path even
when the painted pixels do not cover every point.

## Alternatives

- **Nested analytic glass card.** Rejected because it duplicates material
  bodies, rims, and shadows inside a surface whose hierarchy should remain
  content inside one body.
- **Blue-white or accent outline.** Rejected because an outline is a painted
  control-state convention, conflicts with the optical material, and spends
  semantic accent on structure rather than meaning.
- **Painted wash only.** Rejected because higher alpha obscures preview
  content, while low alpha alone does not remain sufficiently legible over
  every backdrop.
- **Sibling dimming only.** Rejected because it lacks a local target cue when
  only one preview is present and makes selection depend on surrounding
  content.

## Consequences

- Dock previews and the window switcher share one selection hierarchy and
  one set of semantic design tokens.
- The runtime draws preview windows per card so selected and nonselected
  content can have different opacity; this is additional draw orchestration,
  not an additional scene capture.
- Focus motion invalidates the liquid-glass effect cache exactly, as body
  motion already does.
- Optics must support the single-body focus field and reject focus combined
  with smooth union. The mechanism is recorded in Optics ADR-0050.
- Components adding structural selection inside glass must reuse this
  contract instead of introducing component-local colors, borders, or glass
  bodies.
