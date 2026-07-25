# ADR-0028: Output and monitor model

- Status: Accepted
- Date: 2026-06-23

## Context

The nested backend presents a single host window, so `aegis-core` and the
window manager are written against a single output. The DRM/KMS milestone
([M4](../explanation/roadmap.md#m4-drmkms-backend)) and the workspace model
([ADR-0025](0025-workspace-model.md)) both need a real output abstraction.
The [comparative survey](../explanation/comparative-survey.md#input-and-rendering)
sets the floor a modern compositor is measured against: per-output geometry,
mixed DPI, fractional scale, and color management. niri and macOS both
treat per-output independence as worth its implementation cost.

The renderer composites through flux into a single `VkSurfaceKHR` today. The
output model decides how many surfaces flux presents to, how geometry and
scale are resolved per output, and how workspaces relocate when the output
set changes.

## Decision

ass adopts a **per-output independent model**. An `Output` owns its
geometry (physical pixels), its logical size, its scale factor, its
`Transform`, its `Workspace` list ([ADR-0025](0025-workspace-model.md)), and
its presentation `VkSurfaceKHR` from the backend. The renderer runs once
per output per frame, compositing the current workspace's mapped surfaces
plus the chrome for that output.

**Scale** is per-output and supports both integer scale
(`wl_surface.set_buffer_scale`, already applied at composite time per
[ADR-0020](0020-buffer-scale-applied-at-composite.md)) and fractional scale
through `wp_fractional_scale_v1`. Clients that prefer fractional scale
receive a preferred fractional scale and a destination viewport; the
compositor's own chrome renders at the output's logical size and stays
pixel-perfect, the property niri treats as non-negotiable.

**Mixed DPI** works because each output owns its scale; a surface moved
between outputs at different scales is reconfigured with the new scale and
the client re-rasters. The chrome reads the per-output scale from the
snapshot it already receives.

**Arrangement** is part of the configuration
([ADR-0026](0026-configuration-system.md)): position, scale, transform, and
primary flag per output connector name. Hotplugged outputs adopt a sensible
default (extend to the right at their preferred mode and a scale inferred
from physical size) and are then editable.

**Workspace relocation** on unplug and replug is defined by the workspace
model ([ADR-0025](0025-workspace-model.md)); the output model provides the
stable output id and the presence/absence events that drive it.

The backend trait ([`aegis-backend`](../../crates/aegis-backend)) gains a
multi-output surface: the nested backend exposes one output (the host
window) and the DRM/KMS backend exposes one per connected connector. Both
satisfy the same `Output` contract, so the server, renderer, and shell are
written once.

## Alternatives

- **A single global framebuffer with outputs as viewports.** Rejected: it
  forces a global coordinate space (the GNOME/PaperWM problem niri
  corrects), breaks per-output independent scale, and makes mixed DPI a
  special case instead of the default.
- **Mirror/clone as the primary multi-output mode.** Rejected as the
  default: extension is the common case for desktop use; clone is a mode a
  user opts into, not the model.
- **Renderer-driven output enumeration.** Rejected: the output set is a
  backend and configuration concern, not a renderer concern. flux presents
  to the surfaces the backend supplies; it does not discover outputs.
- **Per-output render threads.** Rejected initially: a single render pass
  over the outputs in sequence is simpler and sufficient; per-output threads
  are a future optimization that does not change the model.

## Consequences

- `aegis-core` gains an `Output` type and the window manager, renderer, and
  chrome are parameterized by the output they operate on. The single-output
  nested backend becomes one `Output` rather than a special case.
- The renderer runs once per output per frame and composites the chrome
  separately for each, which raises per-frame work linearly with output
  count and is the expected cost.
- `wp_fractional_scale_v1` and `wp_viewporter` become required protocol
  support, not optional, which raises the protocol surface but matches what
  clients on HiDPI hardware already expect.
- Hotplug handling and the workspace relocation rule in
  [ADR-0025](0025-workspace-model.md) are coupled; the two milestones are
  ordered workspace-then-output in the
  [roadmap](../explanation/roadmap.md#sequencing-rationale) precisely because
  per-output workspaces need a real output to land on.
- Color management and HDR land as flux capabilities following
  [ADR-0001](0001-scope-and-responsibility-boundary.md), not as compositor
  workarounds; they are out of scope for the initial multi-output milestone
  and arrive with the rendering work that needs them.
