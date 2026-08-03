# ADR-0101: Dual presentation paths and conservative KMS plane allocation

- Status: Accepted
- Date: 2026-08-03

## Context

The direct DRM backend can present either a Flux-rendered desktop dma-buf or
an eligible client dma-buf. These sources have different cost and lifetime
rules. Compositing supports arbitrary stacking, shell chrome, capture, and
live backdrop effects, while direct scanout avoids compositor GPU work for a
single opaque surface that already covers the output.

KMS exposes primary, cursor, and overlay planes, but plane availability alone
does not make a layer safe to offload. An atomic plan must also satisfy format,
modifier, source and destination geometry, z-order, alpha, color, damage, and
explicit synchronization constraints. Liquid-glass output depends on the
already-composited scene and is not an independent overlay candidate.

A fullscreen client can own the primary plane when no other visible work is
required. Opening the window switcher changes that scene immediately: the
switcher, previews, and backdrop effect require a composited output even
though keyboard focus remains on the original client until the user commits
the selection.

For the broader presentation model, see
[Architecture](../explanation/architecture.md#presentation-plans-and-plane-roles).

## Decision

Treat compositing and direct scanout as two explicit primary-plane
presentation plans behind one atomic KMS commit boundary:

- The **composited plan** renders wallpaper, client surface trees, effects,
  shell chrome, protocol overlays, and capture work into a compositor-owned
  dma-buf. The resulting framebuffer owns the primary plane.
- The **direct plan** imports one unoccluded client dma-buf and assigns it to
  the primary plane without running a Flux output frame. Eligibility depends
  on actual physical coverage, opacity, transform, viewport, format, modifier,
  synchronization, and the absence of every composition requirement. The XDG
  fullscreen or maximized state bit is not sufficient and is not required.
- Presentation state records the source of the last *successful* primary-plane
  commit. Planning or attempting a fallback does not advance state. Leaving
  direct scanout forces a full compositor redraw and invalidates cached
  backdrop inputs before the successful compositor commit reclaims the plane.

The window switcher always selects the composited plan while it is opening,
visible, or closing. It does not unset, minimize, or otherwise mutate the
current client's fullscreen state. The switcher previews candidates without
moving Wayland keyboard focus; committing the selection raises and activates
the target atomically. This makes the fullscreen surface yield physical plane
ownership without changing its protocol state or causing a configure round
trip merely to show compositor UI.

Use KMS plane roles as follows:

- Assign exactly one **primary plane** to every active CRTC. It carries either
  the compositor framebuffer or the direct client framebuffer, never both.
- Assign one compatible **cursor plane** per active CRTC when possible. Cursor
  updates may retain either primary source. If any output lacks a usable
  cursor plane, a visible cursor is composited and therefore blocks direct
  scanout.
- Inventory **overlay planes** but keep their allocation policy
  compositor-only. A future overlay planner must prove the complete plane set
  with an atomic `TEST_ONLY` request, including z-order, alpha, color, scaling,
  and explicit synchronization, before any layer can bypass composition.

Plane discovery is separate from allocation. Hotplug re-runs both, and the
presentation-domain signature includes cursor allocation and overlay policy so
stale plane assumptions cannot survive a topology change.

## Alternatives

- **Exit fullscreen or minimize the current window when switching focus.**
  Rejected: fullscreen is client protocol state, not a KMS ownership flag.
  Mutating it adds configure latency, changes application behavior, and makes
  a transient compositor overlay permanently alter the desktop.
- **Keep direct scanout active and place the switcher on an overlay plane.**
  Rejected: the live switcher uses dimming, previews, blur, and liquid glass
  derived from the complete desktop. It is not an independent scanout layer.
- **Allocate every compatible client surface to overlay planes.** Rejected:
  format support alone does not prove stacking, blending, color, scaling, or
  fence compatibility. False-positive allocation produces incorrect pixels;
  compositing is the correct fallback.
- **Always composite.** Rejected: it preserves flexibility but wastes GPU and
  memory bandwidth when one client already provides the exact output image.
- **Treat a fullscreen state bit as direct-scanout authority.** Rejected: a
  fullscreen client can still have popups, subsurfaces, a software cursor, or
  compositor chrome above it, while a borderless full-output video can be a
  valid candidate without requesting fullscreen.

## Consequences

- Fullscreen window switching returns the primary plane to the compositor on
  the first successful switcher frame, while focus and fullscreen protocol
  state remain coherent.
- A failed fallback commit leaves software ownership aligned with the
  framebuffer still displayed by KMS and retries with full damage.
- Cursor motion can remain a cursor-only atomic update during either
  presentation path when hardware cursor planes are available.
- Overlay hardware remains unused for now. This trades potential bandwidth
  savings for deterministic output and leaves a clear capability boundary for
  a later tested plane planner.
- Direct-scanout rejection reasons remain observable and rate-limited, making
  composition costs diagnosable without logging every frame.
