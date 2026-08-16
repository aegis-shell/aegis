# ADR-0124: Window open/close fade transitions

- Status: Accepted
- Date: 2026-08-16

## Context

[ADR-0029](0029-animation-and-effect-policy.md) established the declarative
transition layer: the model always reports target geometry, the server
interpolates for rendering only, and reduced motion resolves every transition
in one frame. That machinery covers non-interactive geometry changes
(tiling, maximize, IPC geometry) and the minimize/restore dock flight, but a
window's birth and death remain hard cuts: `wl_surface.commit` with a null
buffer clears the shm snapshot immediately, and destroying the toplevel frees
the surface record, so nothing remains to animate.

The roadmap's M9 (Polish and Completeness) lists "window open/close
transitions" as the remaining animation item. An audit found the
workspace-switch slide already implemented and shipped while unrecorded in
the roadmap or CHANGELOG; open/close was genuinely missing.

Two structural facts shape the design:

1. **An opening window is alive.** Its buffer is committed, its record is
   live, and the existing `WindowTransition` rectangle interpolation already
   drives rendering. An open transition is just a transition record plus a
   fade.
2. **A closing window is gone the moment it closes.** The model window is
   unregistered, focus moves away, and — unlike minimize, which deliberately
   stays mapped — the buffer contents are cleared or the record is reclaimed
   within the same dispatch. A close animation therefore needs a *retained
   snapshot* of the last frame, taken before teardown, that outlives the
   window by one animation duration.

## Decision

- **Open (map) transition.** When a toplevel first maps, after placement
  settles, the server records `WindowTransition::open`: the window fades in
  (opacity `0 → 1`, ease-out) while growing from a rect inset by one
  sixteenth per axis to its full mapped rect. Duration 200 ms.
- **Close (unmap/destroy) transition.** When a mapped, unminimized toplevel
  unmaps (null-buffer commit) or is destroyed while still mapped, the server
  snapshots the last committed contents — the CPU-copied BGRA8 pixels for shm
  clients, or a compositor-owned duplicate of the dma-buf — plus the window
  rect into a **closing frame** ("ghost"). The ghost renders at the
  interpolated rect (full rect → inset target, ease-in) with opacity
  `1 → 0`. Duration 180 ms. It paints after the live desktop scene and can
  never occlude interactive content: nothing below a closing window changes
  while it fades.
- **`transition_opacity`.** `SurfaceGeometry` gains an optional
  `transition_opacity` the scene publishes from
  `WindowTransition::opacity_at`. The renderer applies it as a white solid
  paint's alpha over the whole blit on both the shm and dma-buf paths; the
  opaque-region SRC-replace split is skipped while fading (SRC-replace
  ignores paint alpha and would punch opaque holes in the fade). Geometry
  flights (tiling, minimize) stay opaque: only `Open` and `Close` effects
  imply a fade.
- **Ghost lifecycle and bounds.** Closing frames live in a bounded table
  (`MAX_CLOSING_FRAMES = 8`); a burst of destructions drops the oldest
  ghost's fade early rather than growing compositor memory. Each ghost's
  presentation id is never reused, so renderer texture caches cannot collide.
  `settle_finished_transitions` reclaims settled ghosts on the same tick that
  retires other transitions, and `transitions_pending` includes them so the
  frame loop keeps ticking and damage stays full-frame for the duration.
- **Reduced motion and safety.** `[ui] reduced_motion = true` records
  neither transition: opens are instant, closes drop the ghost and remove
  the window immediately. Minimized windows do not spawn ghosts (the
  minimize flight already covers that exit and a ghost would paint a
  minimized window back onto the desktop). Windows that never committed a
  buffer close instantly. Occlusion skips windows with in-flight transitions
  (existing behavior), so a fading window is never treated as opaque
  coverage.
- **The audit corrections.** The workspace-switch slide is recorded as
  implemented (it already was: 220 ms eased strip with per-page clipping).
  ADR-0055's zero-copy dmabuf export is recorded as superseded by the
  portal repository's slot-ring design, which is what actually shipped as
  IPC protocol 25; the dmabuf stream path gains the same security-generation
  snapshot check the SHM path already had, so frames blitted before a
  lock→unlock or VT boundary never reach a consumer.

## Alternatives

- **Scale-only open/close via the existing rect interpolation, no fade.**
  Rejected as the primary effect: without opacity the first and last frames
  of the flight look like a jump-cut scaled window, and close still needs
  the ghost snapshot regardless, so the fade is nearly free once the frame
  is retained.
- **Retain the whole `SurfaceRec` and defer reclamation.** Rejected: the
  record carries protocol state (resource pointers, focus links, children)
  whose teardown semantics would have to be split into "protocol-dead,
  render-alive" halves — a large unsafe surface for a 180 ms effect. A
  minimal snapshot (pixels/dma-buf + rect + id) is the entire ghost.
- **Drive the fade in lens (chrome) rather than in the compositor.**
  Rejected: lens draws chrome; client window pixels are composited by
  flux through `aegis-render`, and [ADR-0123](0123-chrome-fades-via-lens-opacity.md)
  explicitly scopes lens opacity to chrome surfaces.
- **Configurable per-effect durations in v1.** Deferred, consistent with
  the current fixed constants elsewhere (minimize styles, workspace slide);
  a named transition table remains a follow-up on ADR-0029 if user need
  appears.

## Consequences

- Windows visibly fade in on first map and leave a 180 ms fading ghost when
  closed; both collapse to one frame under reduced motion.
- Compositor memory is bounded by eight retained last frames; a ninth
  concurrent close ends the oldest fade early (logged at debug).
- The renderer applies per-window fade opacity on both backing paths; the
  overview and window-switcher mapped draws are unaffected (mapped mode
  ignores transition geometry and opacity — the map owns placement).
- Ghosts hold a duplicated dma-buf descriptor until they settle; the
  renderer imports it once per ghost into its cache, keyed by the ghost's
  unique id, and the entry is retired with the ghost.
- ScreenCast consumers of the dmabuf transport (protocol 25) now observe the
  same lock-boundary frame invalidation the SHM transport always had.
