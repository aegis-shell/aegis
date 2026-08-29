# ADR-0142: Layered glass — the backdrop compositor owns the frost→glass nesting

- Status: Superseded by ADR-0143
- Date: 2026-08-25
- Scope: `crates/aegis/src/runtime/{rendering,presentation}.rs`; adds an Optics-side material (`prism` backdrop layer, Optics ADR-0079); amends [ADR-0094](0094-liquid-glass-lens-model-and-full-resolution-capture.md)

## Context

A chrome surface is usually not one material. The command panel ([ADR-0080](0080-hud-status-chips-and-sao-command-panel.md) refresh, [ADR-0115](0115-panel-hosted-settings-and-hud-command-panel.md)) is the clearest case: one **fullscreen frosted sheet** carries six floating **liquid-glass bodies** on top of it, and the painted scheme-adaptive scrim sits over both. The compositor's backdrop pass therefore had to produce, from one desktop capture, a stack in which the glass *refracts the frost*.

The pre-existing pipeline could not express that relation. Both effects sampled the same sharp desktop capture: `BlurFilter::apply_regions` frosted it into one image, `LiquidGlassFilter::apply` refracted the same capture into another, and `recompute_effects` drew the two into a transparent composite with the frost first and the glass second. Two images drawn in sequence *look* stacked, but the glass lens had sampled the desktop, not the frost — so through every glass body the frost was optically bypassed. The panel's island bodies showed sharp, unfrosted desktop through their rims, the layer order read as broken at each silhouette, and the "two offscreen renders" the design intended (frost, then glass *over the frost*) had silently become two parallel renders of the same background.

Optics' ADR-0017 capture seam and ADR-0008/0074 effect-intake path deliberately leave effect chaining caller-side; prism's `prism_liquid_glass_desc` takes exactly two inputs (sharp capture + its blur) and cannot see another material's output. Optics ADR-0050 had also rejected *nested* glass *bodies* — but that rejection is about one glass body nesting inside another (nested rims/shadows), not about a glass material nesting over a frost sheet, which is a different relation.

## Decision

Glass-over-frost is a **material relation**, so it belongs in Optics as a named material: the new `prism_backdrop_layer_filter` (prism's "backdrop layer") composes, in one ordered dispatch sequence into one persistent transparent output:

1. **frost** — every `prism_backdrop_frost` rectangle writes the blurred backdrop source-over inside a rounded-rect SDF;
2. **glass** — every `prism_liquid_glass_group` runs the unchanged reference recipe (`liquid_glass.comp`), but the lens samples the **frosted layer image**, so a body bends and frosts the frost beneath it instead of looking past it.

The layer order (all frost beneath all glass) is the material's identity, not caller policy. Group order *within* the glass layer remains the caller's paint order, exactly as in the standalone filter. Per-group backdrop statistics are submitted and read back on the same slot cadence as the standalone material. The shared glass dispatch was extracted into `glass_dispatch.h` so a layered body is evaluated by construction identically to a standalone one — only its sampled backdrop differs.

The material's layer image is a **complete opaque background**, not a floating sheet: the frost pass writes the sharp capture everywhere it dispatches and blends each (optionally tint-washed) frosted rect over that base inside its coverage. A lens may therefore sample anywhere its bend reaches — including outside every frost rect — and always reads a resolved true colour with alpha 1, never a premultiplied half-brightness fragment at an AA edge or a transparent hole under a body whose frost was elided. `prism_backdrop_frost` carries `tint_color`/`tint_strength` so a caller's veil is blended INTO the frost, beneath the glass.

Aegis consumes it as policy along three rules:

1. **One layered apply.** `LauncherBackdrop` owns one `BackdropLayerFilter`; `recompute_effects` issues **one** layered apply (frost rects + glass groups) instead of a standalone glass apply, then blits the single result into the per-slot composite. Frost declarations map through a new `backdrop_frost_in_capture` (rounded shapes into the layer image, not rectangular canvas clips) and the layered dispatch receives the **unfiltered** declaration set: a glass body refracts whatever frost lies beneath it. The historical equal-rect filter survives only in the degraded all-frost fallback, where a body is replaced *by* its frost rect rather than layered over it.
2. **Washes are declared, not painted.** `BackdropRegion` gains an optional [`BackdropWash`] (`aegis_shell::backdrop_wash` converts a paint colour; strength maps from the painted veil's alpha). The command panel's ink/pearl scrim, the launcher's dim, the prism spotlight's veil, and every modal prompt's full-display dim moved from chrome `render()` into their `backdrop_regions` declarations — beneath the analytic glass, blended into the frost — and their painted placements are deleted. A chrome-painted veil sits *between* the frost and the glass: it hides the lens's refraction and splits the stack into "effects below, paint above".
3. **The layer stack is one material.** Output order is unchanged — desktop base → layered composite (wash ⊂ frost ⊂ glass) → panel content (lens) → overlays — but every translucent surface treatment now lives inside the one composite.

Layering semantics of the whole output are unchanged and now honest:

```
desktop base → layered backdrop composite (frost ⊂ glass) → painted scrim + panel content (lens) → overlays
```

## Alternatives

- **Re-capture between the two materials** (capture → frost → draw frost into a second capture target → glass over it). Rejected: doubles the scene render per frame, breaks the `BackdropPlan` capture/material cache split that keeps animation frames cheap, and pushes material composition into the caller — exactly the policy/mechanism split [ADR-0139](0139-animation-effect-placement.md) forbids.
- **Two standalone prism applies, glass second**, keeping the old stack but drawing glass's output over the frost composite. Rejected: it is the status quo — the lens still samples the sharp capture, so the frost is still bypassed wherever glass exists. Draw order cannot fix a sampling relation.
- **A per-group input texture on `prism_liquid_glass_group`**. Rejected: the 160-byte push-constant budget is already bit-packed (Optics liquid-glass implementation note), and a group-scoped input would admit arbitrary graphs the effect ADRs rejected.
- **Removing the panel's fullscreen frost and giving every body more frost strength.** Rejected as a policy answer to an architecture problem: the design language (ADR-0080) wants a legible shared veil *and* analytic bodies; weakening one to hide the seam is a compromise, not a fix.

## Consequences

- Through every glass body the frost is now what the lens refracts: the fullscreen veil and the island bodies read as one stack, and the panel's wash dims glass, not raw desktop. Bodies that declared their own frost rect (dock, HUD chips, prism pane, modal panels) now visibly sit *on* that frost — the declared layering, and slightly softer than the old direct-over-capture render because the body's `frost_strength` composites over an already-frosted backdrop.
- Washes fade by strength instead of a painted alpha, so a mid-reveal wash no longer composites a translucent rect over the glass — the reveal's blur-then-wash ordering is preserved by the wash's strength riding the same reveal curve.
- One fewer offscreen effect image is drawn per frame in the success path (frost and glass share the layer filter's persistent output instead of two separate draws into the composite), and the composite write is a single blit per capture region.
- The glass shader's *sampled* backdrop for layered bodies is the frost, so per-group backdrop statistics (mean luminance, HF energy) still reduce over the sharp capture and blur — the numbers continue to describe the desktop behind the material.
- Optics release cost applies (Rust bindings mirrored 1:1, `tests/prism/integration/test_backdrop_layer.c` is the pixel gate — it fails if the lens is pointed back at the sharp capture); the Aegis workspace re-pins on the next Optics tag.
- A future glass-over-glass request (a popover lens over a dock lens) remains out of scope and would need its own layer material; this filter's layer order is deliberately frost-only beneath glass.
