# ADR-0139: Animation effect placement — Optics mechanism, Aegis policy

- Status: Accepted
- Date: 2026-08-22
- Scope: whole workspace; amends [ADR-0029](0029-animation-and-effect-policy.md)

## Context

Aegis renders through the Optics stack (`flux`, `lens`, `prism`, `iris`).
Work on new visual effects keeps raising the same question: does an effect
belong in `../optics` or in an Aegis crate? Both repos already answer it
independently — Optics ADR-0047 ("flux owns mechanism, the caller owns
policy, the shader owns the material identity") and Aegis
[ADR-0001](0001-scope-and-responsibility-boundary.md) / ADR-0029 ("a
rendering or texture capability missing from flux is added to flux, not
worked around in aegis") — but the combined rule was never stated in one
place, and the Aegis side had begun to duplicate motion *mechanism* in
parallel: three hand-rolled springs with divergent integrators and three
local variants of the reduced-motion rule (`aegis-dock`'s analytic
under-damped spring for tile magnification, the launcher's semi-implicit
Euler reveal spring, per-component exponential decays). Optics had none of
that as a library — deliberately: "flux deliberately does not own an
animation timeline" (`examples/flux/image_animation.c`); lens ships
"mechanism, not animation".

A second force is the release cadence. Every Optics-side change carries a
cross-repo cost: Rust bindings must mirror the C API 1:1, `docs/reference`
and the symbol table must be updated, a test lands under `tests/<lib>/`,
and the Aegis workspace must re-pin the Optics tag and regenerate the
canonical `Cargo.lock`. Effects that begin as Aegis-local pixel hacks
cannot be promoted into Optics later without re-doing all of it.

## Decision

The single classification test for any animation or effect:

> **If it needs to touch pixels, images, or GPU command buffers, it is
> mechanism and it belongs in Optics. If it decides what moves, when it
> starts, or how strong it is, it is policy and it belongs in Aegis.**

### Optics side (mechanism)

- New image operators (drop shadow, glow, tone-map, new blur modes):
  `flux` `effect` module + the `flux-rs` binding.
- New material looks (siblings of liquid glass): `prism`.
- New canvas/UI drawing capabilities, and the future niri-style
  custom-shader path: `lens` (per ADR-0029, through the same
  name-and-parameter mechanism).
- Optics ships no tween/easing/timeline library and adopts none: clocks
  stay host-owned, per its own repeated decision. Aegis does not ask for
  one.
- Flavor ships as copyable recipes under Optics `examples/showcase/`
  (its ADR-0061), never as a flag on a library knob.

### Aegis side (policy + shared motion vocabulary)

- `aegis-ui::motion` is the single shared motion vocabulary (springs,
  decays, easing curves, the reduced-motion rule). Chrome components do
  not hand-roll integrators again; they pick a stiffness/damping pair and
  own their state. The module is pure math on caller-owned state — no
  timeline, no clock, no scheduler, honoring both ADR-0029's rejection of
  an "animation framework that cannot draw" and Optics' host-owned-clock
  boundary.
- Named window transitions, durations, and easing curves: types in
  `aegis-model` (`transition.rs`), lifecycle and triggers in
  `aegis-compositor`, keys in `aegis-config`. Unchanged from ADR-0029.
- Chrome reveal/stagger/spring choreography lives in the owning chrome
  crate on `aegis-ui::motion`; a component with its own state or
  dependency profile gets its own crate on the `aegis-shell` `Chrome`
  contract (ADR-0021 / ADR-0060).
- Effect *parameters* (blur sigma, radii, glass strengths) are data in
  `aegis-design` (`tokens.rs` / `materials.rs`), consumed through the
  `LiquidGlassRegion` contract; the composite pass is plumbed through the
  binary, exactly as the liquid-glass pass is today.
- `aegis-render` keeps its CI-enforced `aegis-model`-only dependency
  floor; render-side effect code takes plain data over those types.

### Intake path for a new visual effect

1. Classify: pixel-touching → Optics; decision → Aegis (table above).
2. Optics-bound: implement in the appropriate library with its ADR,
   mirror the binding, update `docs/reference` + symbol table, add a
   test, tag a release. Aegis then re-pins the tag, regenerates the
   canonical `Cargo.lock`, and lands the call-site parameters in
   `aegis-design`.
3. Aegis-bound: extend `aegis-ui::motion` only when two components would
   otherwise duplicate the same math; otherwise the owning crate keeps it
   local. Never add a rendering dependency to `aegis-ui::motion`.

## Alternatives

- **A shared tween/timeline framework in Aegis.** Rejected: ADR-0029
  already rejected an "animation framework that cannot draw"; a timeline
  that can draw duplicates lens's interpolation role.
- **Adding an animation/timeline library to Optics.** Rejected: Optics
  has repeatedly decided clocks and timelines are host-owned
  (`examples/flux/image_animation.c`, lens "mechanism, not animation");
  it would also make every Optics consumer pay for Aegis's choreography.
- **Keeping per-component springs indefinitely.** Rejected: three
  divergent integrators and three reduced-motion variants is exactly the
  drift the placement rules exist to prevent; the shared module keeps
  each component's feel *parameters* (its stiffness/damping pair) while
  the math and the a11y rule are written once.
- **A new `aegis-effects` crate.** Rejected: an effects crate that
  cannot draw is in the wrong layer (ADR-0029), and one that draws would
  have to sit above `lens`/`flux` — duplicating `aegis-shell` + the
  binary's existing composite seam.

## Consequences

- `aegis-ui::motion` gains `Spring` (analytic, dt-stable),
  `approach`, `decay::toward_zero`, `ease_in_cubic`, and `frame_dt`;
  `aegis-dock` and the launcher spring refactor onto it, deleting their
  local integrators and reduced-motion variants. Their motion feel is
  unchanged (the same stiffness/damping pairs and thresholds are
  preserved).
- Proposing a new Optics-side operator means accepting the cross-repo
  checklist above; an effect is designed Optics-first from the start or
  it is a recipe, not a library feature.
- The placement test is checkable in review: a diff that touches GPU
  work or pixels outside Optics, or that adds a second spring
  integrator inside Aegis chrome, violates this ADR.
- Future work this enables: the niri-style custom-shader lens capability
  (ADR-0029) slots into the Optics side of the table without reopening
  this decision.
