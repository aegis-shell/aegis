# ADR-0143: Explicit offscreen composition DAG for cumulative layers

- Status: Accepted
- Date: 2026-08-25
- Scope: `aegis-shell`, `aegis-render`, and the compositor backdrop executor;
  supersedes [ADR-0142](0142-layered-glass-backdrop-compositor.md)

## Context

ADR-0142 repaired frost beneath glass by treating the pair as one fused
Prism material. That fixed the immediate optical defect, but Aegis still
owned one flat capture, one blur filter, one backdrop-layer filter, and one
list of regions. A later lens could not sample the resolved result of an
earlier lens. Supporting cover blur, then glass, then glass again would
have required another named special case.

The nesting relation is not specific to glass. Any offscreen result may
feed another raster, compute, copy, or material pass. The complete
dependency structure is also required to compute accumulated sampling
footprints, reject cycles, cache frame-slot outputs, and avoid unnecessary
intermediate targets.

Optics ADR-0080 now provides a generic composition DAG above Flux. It
deliberately plans structure and resources without owning Aegis product
policy or GPU commands.

## Decision

Aegis expresses cumulative backdrop work as explicit image dependencies.

1. `BackdropLayer` declares a stable `BackdropLayerId`, a source
   (`Scene` or another layer id), blur sigma, frost regions, and glass
   bodies. Components derive ids from stable layer names. Duplicate ids,
   missing sources, and cycles reject the graph for that frame and use the
   existing translucent fallback.
2. Existing `Chrome::backdrop_blur_sigma`, `backdrop_regions`, and
   `liquid_glass_regions` declarations remain compatible. `Shell` fuses
   them into the reserved root layer. Components use `backdrop_layers`
   only when they intentionally need another resolved input.
3. The compositor compiles declarations with Optics
   `flux-composition-graph`. Stable topological order is the execution and
   paint order. UI ownership, widget ancestry, and clipping never create
   dependencies implicitly.
4. Every layer edge expands reverse ROI by the layer's conservative 3σ
   sampling radius. The scene capture is the union that reaches the source
   after every edge is propagated, so a three-level chain accumulates all
   three radii while disconnected bodies remain disconnected.
5. One reusable Prism backdrop operator evaluates frost beneath glass
   inside each graph node. This preserves ADR-0142's local material
   identity. A node that feeds another node additionally resolves its input
   plus transparent composite into a source image; a final-only node does
   not allocate that extra image.
6. Capture images, transparent composites, and required resolved images are
   indexed by Flux frame slot. An unchanged stack reuses the slot cache.
   Source-only changes preserve the capture/material key split and rewrite
   only intersecting connected capture regions. Geometry, topology, sigma,
   or material changes invalidate the affected slot's stack.
7. The output remains `desktop base → layer composites in topological order
   → Lens chrome → overlays`. Drawing a composite is not a sampling edge;
   only `BackdropLayerSource::Layer` exposes the resolved result to a later
   material.

## Alternatives

- **Add cover-glass-glass to Prism as another material.** Rejected: every
  new depth or branch would need another material name and shader contract.
- **Infer nested offscreen work from component or widget nesting.**
  Rejected: presentation ownership is not an image dependency, and a layout
  refactor must not silently change GPU passes or memory.
- **Recapture the desktop for every level.** Rejected: later layers need the
  preceding resolved image, not another view of the original desktop, and
  repeated scene rendering multiplies client acquisition and draw work.
- **Always allocate a resolved target for every layer.** Rejected: only
  nodes with outgoing sampling edges need one. Final composites are already
  sufficient for output composition and caching.
- **Fuse the complete DAG into one material dispatch.** Rejected as a
  requirement: arbitrary branches, cache boundaries, and non-material
  operators cannot share one shader. Proven local fusion remains allowed.

## Consequences

- Cover blur → glass → glass and arbitrary finite branches are now one
  declaration model. No glass-specific nesting code is required.
- GPU work remains proportional to the number of changing levels over the
  affected ROI. Three full-screen changing levels are approximately three
  levels of blur/material work; the architecture does not pretend otherwise.
- Reverse ROI, disconnected-region planning, frame-slot caches, and
  final-only resolved-target elision bound the current backdrop executor's
  common case. Optics also exposes transient-lifetime assignments, per-node
  forward damage, and validity decisions for executors that can alias
  intermediates or skip unchanged branches independently; Aegis can adopt
  those without changing the declaration model.
- Aegis retains policy: which surfaces may stack, their ids, material roles,
  and paint order. Optics retains mechanism: DAG validation, region
  propagation, lifetime planning, and image operators.
- The old “never stack glass on glass” product default remains good visual
  guidance, but it is no longer an architectural limitation. Intentional
  cumulative glass must declare an explicit layer edge and justify its
  readability and cost.
- Joint development temporarily uses the local Optics patch. Promotion waits
  for an Optics release containing `flux-composition-graph`, followed by the
  normal canonical dependency and lockfile update.
