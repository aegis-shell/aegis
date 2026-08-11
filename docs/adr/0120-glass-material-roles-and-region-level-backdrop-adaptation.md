# ADR-0120: Glass material roles and region-level backdrop adaptation

- Status: Accepted
- Date: 2026-08-12

## Context

The liquid-glass interior is optically flat: refraction lives only in
the rim band (see
[ADR-0094](0094-liquid-glass-lens-model-and-full-resolution-capture.md)).
The reference recipe therefore passes a sharp backdrop through the body
with light frost and a 10–24% per-pixel adaptive tint. Decorative
bodies read well that way; text-bearing bodies did not. White menu text
over a busy terminal — bright prompt lines behind the Dock context menu
— measured about 2:1 contrast, below the WCAG AA floor of 4.5:1. The
per-pixel tint polarity also zebra-striped over mixed content, flipping
row by row behind a single menu.

The material-library migration to prism (Optics ADR-0063) provides
per-group material overrides, and its GPU backdrop-statistics reduction
(Optics ADR-0065) reports each identified region's mean luminance and
high-frequency energy, frame-lagged by the frames-in-flight count.
aegis needed a caller-side policy that turns those numbers into
material strength without dithering the composite.

The backdrop cache key was also monolithic: every parameter change —
including a pure opacity fade — invalidated the capture and re-rendered
the scene into it, and `glass_tint` participated in no key at all, so a
color-scheme flip could leave a stale composite on screen.

## Decision

Give glass roles a material policy, pin the plate polarity of
text-bearing roles, adapt identified regions from measured backdrop
statistics, and split the backdrop cache key along the
capture/material boundary.

`GlassStyle` gains `frost_strength`, `tint_strength`, and `saturation`
— multipliers on the shared optical recipe, where 1.0 is the reference
look — plus `plate_polarity` (0 pins the smoke plate, 1 pins pearl, -1
keeps the shader's per-pixel adaptive polarity). A new `GlassRole::Menu`
covers text-bearing transient surfaces. The dark appearance gives Menu
frost 5.0, tint 3.6, saturation 0.7, smoke-pinned, and Tooltip
3.0/3.0/0.85 smoke-pinned; the light appearance gives Menu 5.0/4.5/0.8
and Tooltip 3.0/3.5/0.9, both pearl-pinned. Every other role keeps the
reference recipe and per-pixel polarity. Frost is decoupled from the
rim: interior scattering carries the legibility budget while the rim
keeps the liquid identity. The recipes are `aegis-design` tokens,
pinned by smoke tests; components never tune them locally.

Polarity is pinned per role against the role's text tone — smoke under
light text, pearl under dark text. Measured backdrop statistics
modulate strength only, never direction.

Adaptation is caller-side policy on top of the prism statistics.
Regions opt in with a stable identity derived from their layer id; the
app context menus (Dock and launcher) and the Dock's hover surface
(tooltip and live preview) adapt today. The compositor
smooths each region's statistics exponentially (rate 2.5/s, with the
first sample snapping so a freshly opened surface adapts within the
stats lag), quantizes the shipped values to 1/32 with hysteresis at
each step boundary, and applies a polarity-aware tint-strength recovery
that eases toward a 0.55 floor as the backdrop approaches the calm
state friendly to the pinned plate. The boosted strengths are the
worst-case budget; the recovery is where the liquid look lives.

The contrast budget for the menu recipe: white menu text (L ≈ 0.95)
needs a plate at L ≤ 0.18 for WCAG AA. With the Menu role's strengths,
a bright text glyph behind the menu lands at plate L ≈ 0.16 (≈ 4.9:1),
and a uniform white backdrop lands at ≈ 0.17 (≈ 4.6:1).

The backdrop cache key splits in two. `BackdropCacheKey` keeps the
capture side: geometry, sigma, scale, model activity, capture regions,
and scene overlays. A new `BackdropMaterialKey` carries the effect
side: frost regions, every liquid-glass parameter including the
adaptation writeback, and `glass_tint`. A material-only change takes
`BackdropPlan::Recompute`, which rebuilds blur, glass, and composite
over the still-valid capture instead of re-rendering the scene.

## Alternatives

- **True per-pixel inversion.** Rejected: degenerate at 50% gray, and
  per-pixel hue flips produce color chaos under text.
- **Backdrop-derived plate polarity for text bodies.** Rejected: a
  pearl lift over dark content pulls the plate toward the light text it
  must contrast with; measured statistics modulate strength only.
- **A fixed boosted recipe without adaptation.** Rejected: the
  worst-case budget applied everywhere keeps bodies milky over calm,
  friendly backdrops where translucency costs nothing.
- **An opaque painted menu body.** Rejected: a fill over the material
  violates the content-first design rule; the legibility budget belongs
  to the material.
- **Folding material parameters into the capture key.** Rejected: every
  opacity fade and adaptation step would re-render the scene — the
  tooltip-fade capture churn this split removes.

## Consequences

- Menus and tooltips hold WCAG AA over the measured worst cases;
  decorative bodies keep the reference recipe unchanged.
- Opacity fades and adaptation steps rebuild only the effect composite;
  the scene — and therefore clients — is no longer re-rendered for
  material motion. This removes the tooltip-fade capture churn.
- `glass_tint` now participates in a cache key, so color-scheme flips
  invalidate the composite correctly; previously they could leave a
  stale composite.
- Adaptation output re-runs the glass composite, so the quantization
  and hysteresis discipline is load-bearing: an emitted value must
  never dither across a step boundary.
- Region identities must stay stable and unique per body; a transient
  surface must never inherit another body's smoothed backdrop.
- Statistics lag by the frames-in-flight count. Surfaces that need
  correct material on their first visible frames rely on the
  first-sample snap; the lag bounds how fast adaptation can react
  afterward.
- aegis depends on the `prism` crate (Optics ADR-0063) in place of
  `flux::LiquidGlass*`; the workspace pins an Optics tag containing
  prism-rs, and the local-override patch table and the terminal
  binary's rpath plumbing carry prism alongside flux and lens.
- The design-language contract for roles, polarity, and adaptation
  lives in `docs/dev/design/liquid-glass.md` and
  `docs/dev/design/surfaces.md`; new text-bearing glass selects `Menu`
  or `Tooltip` rather than tuning strengths locally.
