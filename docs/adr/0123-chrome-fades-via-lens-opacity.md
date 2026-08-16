# ADR-0123: Chrome enter/exit fades go through lens opacity

- Status: Accepted
- Date: 2026-08-15

## Context

Chrome surfaces (command panel, overview, launcher, run prompt, HUD
chips, dock, window switcher) animate in and out with a fade driven by
a per-component progress clock. Because lens historically had no
opacity concept, each surface baked the progress into individual color
alphas by hand — `themes::faded` for theme-bound widgets, local
`fade_color`/`alpha()` helpers for explicit colors, tinted image calls
for the few rasters someone remembered. The mechanism was
uncompletable: raster images drew with a hardcoded opaque tint, slider
and scrollbar skins read their colors straight from the theme, and the
overview never faded its text at all. During a close animation those
elements stayed fully opaque and popped out at teardown, visibly
outliving the surface around them.

Optics ADR-0068 adds the missing primitive: a frame-scoped,
node-stamped opacity switch in lens (`Frame::set_opacity`) that fades
every draw command uniformly at emission.

## Decision

Chrome enter/exit fades set the lens opacity switch at the surface's
render root — optionally per section for staggered reveals — and
restore `1.0` afterwards. Baking a fade progress into individual color
alphas is reserved for static translucency (glass tints, scrims at
rest) and for content brightness (the window switcher's
inactive-window dimming); it is no longer a fade mechanism.
`themes::faded` and the per-crate `fade_color`/`alpha()` fade helpers
are removed so there is exactly one fade path. Animation clocks,
stagger math, and slide offsets are unchanged: only alpha application
moves.

## Alternatives

- **Keep patching call sites.** Each newly reported widget kind would
  get its own fade plumbing, and the next custom-drawn component would
  escape again. The bug class recurs by construction.
- **Compositor-side layer alpha.** Fading the composed layer would
  cover even compositor-drawn content, but lens chrome and compositor
  scene elements interleave per surface, and the scrim/blur regions
  already track their own progress; a second fade domain would
  double-dim or drift.

## Consequences

- New chrome surfaces with enter/exit motion must wrap their build in
  `Frame::set_opacity`; they must not reintroduce per-color fade
  helpers.
- Static translucency keeps using explicit alpha colors — that is
  design data, not motion.
- Compositor-rendered pixels outside lens (overview thumbnails, live
  previews, backdrop blur) keep their own progress plumbing; the lens
  switch governs lens-drawn chrome only.
- `aegis-design` drops `themes::faded`; the shared preview materials
  (`aegis-shell`'s `preview` module, `materials::glass_focus`) stop
  taking a visibility parameter because their consumers fade through
  the opacity switch.
