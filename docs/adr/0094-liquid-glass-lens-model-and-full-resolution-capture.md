# ADR-0094: Liquid glass lens model and full-resolution backdrop capture

- Status: Accepted
- Date: 2026-08-01

## Context

The analytic liquid-glass pass composited every glass body from a
quarter-resolution backdrop capture (`BACKDROP_DOWNSAMPLE = 4`), and its
shader lit the rim with a broad additive white band plus a bluish body
haze. Two complaints resulted: a milky halo around every glass body, and
a coarse, low-resolution look under any detailed content. Quarter-res
blur is harmless for the frosted path — dual-Kawase hides it — but the
glass path sampled the same downsampled capture for its "sharp"
refraction, so fine detail under glass was unrecoverable. The Dock
additionally painted a 13% white fill and a 1 px border over the glass,
doubling the milky layer and outlining the silhouette.

Apple's Liquid Glass (WWDC 2025, session 219 and the Human Interface
Guidelines) defines the target behavior: a convex lens that leaves its
interior optically clear, bends light in a curved rim band, adapts its
body tint against the backdrop's luminance, and carries a thin
directional key light rather than a uniform rim wash.

## Decision

Adopt a convex-lens material model for the analytic glass pass and
evaluate it at full physical resolution.

The shader treats each body as a lens slab: flat interior (zero
distortion), spherical-cap rim band of `edge_width` pixels. Refraction
displaces the sharp sample inward along the rim normal — a magnifier —
with per-channel bend for dispersion confined to the rim. Lighting is a
thin key line toward an up-left key light, a direction-weighted sheen, a
shadow-side dark line, and a faint transmitted-light trough; no additive
term spans the rim. The body tint opposes backdrop luminance (pearl over
dark, smoke over bright), and a small dither defeats `rgba8` banding.

`BACKDROP_DOWNSAMPLE` becomes 1: the capture covers the padded union of
backdrop regions at full physical resolution and feeds both the glass
pass and the dual-Kawase frost. Painted foreground layers on glass
become minimal: the Dock's resting material drops to alpha 12 with no
border, so the glass rim supplies edge definition.

The optics-side shader contract and rationale are recorded in Optics
ADR-0046; the design-language rules are recorded in
`docs/dev/design/`.

## Alternatives

- **Keep quarter-resolution capture with better upsampling.** Rejected:
  upsampling cannot recover high-frequency content the capture never
  recorded; the low-resolution smear is inherent to the downsample, not
  to the reconstruction filter.
- **Two captures: quarter-res for frost, full-res for glass.** Rejected:
  the scene renders once per capture, so two captures double the scene
  render for the common case. One full-res capture serves both paths;
  the blur pyramid keeps frost cheap.
- **Keep the painted Dock border and fill.** Rejected: a painted outline
  over an optical rim duplicates the edge and reintroduces the milky
  double layer the redesign removes.
- **Adaptive resolution by region area.** Deferred: complexity without a
  measured need. The region-union clamp already bounds the capture; the
  full-screen launcher case pays one full scene render while open.

## Consequences

- The capture pass renders up to 16x more pixels than before, bounded by
  the region-union clamp; with a live 3D wallpaper the capture stays
  full-screen by design.
- Parameter semantics are unchanged (physical pixels), so the existing
  logical values (`refraction` 8, `edge_width` 18, `glare` 0.55) carry
  over; only their precision improves.
- `light_direction` now consistently means "direction toward the light";
  highlights sit at the top of bodies.
- Rebuilding the optics shader requires care on hosts using ccache:
  ccache does not track `#embed` inputs, so stale objects survive a
  shader edit unless the build runs with `CCACHE_DISABLE=1` or the cache
  is cleared.
- Follow-up: expose per-body thickness (larger elements read thicker in
  Apple's model) and interactive inner glow if chrome adopts lift-up
  interactions.
