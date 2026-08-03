# Rendering and KMS Plane Reference

This page defines the observable rendering contract for direct DRM/KMS
sessions. Nested sessions always present a composited frame to the outer
compositor and do not own KMS planes.

For the design model, read
[Architecture](../explanation/architecture.md#presentation-plans-and-plane-roles).
For the decision history, read
[ADR-0101](../adr/0101-dual-presentation-paths-and-conservative-kms-plane-allocation.md).

## Presentation Plans

| Plan | Primary-plane source | Output composition | Selection rule |
|------|----------------------|--------------------|----------------|
| Composited | Compositor-owned dma-buf | Flux renders the desktop, clients, effects, shell, overlays, and capture work | Selected whenever any visible scene element or buffer constraint requires composition |
| Direct scanout | Client-owned dma-buf | The compositor output render is skipped | Selected only when one eligible client buffer is the complete physical presentation domain |

Fullscreen and maximized window states do not select either plan. The plan
follows physical coverage and the complete visible scene. A borderless client
can scan out without an XDG fullscreen request, while a fullscreen client is
composited when another visible element is present.

All active outputs currently form one presentation domain and share one
desktop framebuffer. A direct-scanout candidate must therefore cover the
complete physical domain and be accepted by every selected primary plane.

## Direct-Scanout Eligibility

Every condition in this table must hold for the direct plan.

| Area | Required condition |
|------|--------------------|
| Scene | Exactly one client dma-buf surface is visible and it is the single toplevel surface |
| Other buffers | No SHM client surface, popup, subsurface, protocol overlay, or second dma-buf surface is visible |
| Coverage | Buffer dimensions exactly match the physical presentation domain and its origin is `(0, 0)` |
| Geometry | Buffer transform is normal and no source or destination viewport is active |
| Opacity | The format has no alpha channel, or the declared opaque region covers the complete logical surface |
| Shell | No visible shell pixels, overview, window switcher, transition, or live backdrop effect requires composition |
| Capture | No frame capture, screenshot freeze, or Realm capture is pending |
| Session | The session is unlocked and the presentation target is active |
| Cursor | The cursor is hidden or every active output has a compatible hardware cursor plane |
| KMS format | Every active primary plane accepts the client format and modifier |
| Synchronization | An explicit acquire fence is present only when every active primary plane supports `IN_FENCE_FD` |
| Atomic commit | The kernel accepts the complete atomic plane request |

Client damage does not by itself block direct scanout. Damage regions are
forwarded to KMS through `FB_DAMAGE_CLIPS` when the selected primary planes
support the property.

## Composition Rejection Reasons

Direct-scanout rejection telemetry uses the following stable labels.

| Label | Meaning |
|-------|---------|
| `session-locked` | The lock scene owns the output |
| `capture-pending` | Output, screenshot, or Realm capture requires a composited frame |
| `screenshot-freeze` | A screenshot freeze session owns the visible frame |
| `surface-transition` | A client-surface transition is active |
| `overview` | Overview mode is visible |
| `window-switcher` | The live window switcher is opening, visible, or closing |
| `live-backdrop-effect` | Visible chrome samples or filters the composited backdrop |
| `visible-shell-pixels` | Shell chrome contributes pixels to the output |
| `client-overlay` | A protocol overlay or client cursor surface is visible |
| `software-cursor` | A visible cursor must be drawn into the compositor frame |
| `shm-client-surface` | A visible client surface uses SHM rather than dma-buf |
| `no-dmabuf-candidate` | No client dma-buf can supply the primary plane |
| `multiple-dmabuf-surfaces` | More than one client dma-buf surface is visible |
| `not-single-toplevel` | The only dma-buf is not the complete toplevel scene |
| `non-trivial-transform` | The candidate uses a non-normal buffer transform |
| `non-zero-origin` | The candidate does not begin at physical origin `(0, 0)` |
| `viewport` | The candidate uses a source or destination viewport |
| `geometry-mismatch` | The candidate dimensions do not match the physical domain |
| `not-fully-opaque` | Alpha may expose content below the candidate |
| `plane-unsupported` | The primary-plane format or modifier intersection rejects the buffer |
| `kms-rejected` | KMS rejected an otherwise plausible direct atomic commit |

Plausible full-output candidates are reported at info level. Ordinary desktop
states with no candidate are reported at debug level. Reports are rate-limited
and include recent rejection counts.

## KMS Plane Roles

| Plane role | Allocation | Content contract | Direct-scanout effect |
|------------|------------|------------------|-----------------------|
| Primary | Exactly one per active CRTC | Either the compositor framebuffer or the direct client framebuffer in one atomic commit | Owns the selected presentation plan |
| Cursor | At most one compatible ARGB8888 linear plane per active CRTC | Compositor-owned cursor sprite; allocation maximizes coverage across all CRTCs | Allows a visible cursor without reclaiming the primary plane |
| Overlay | Discovered and counted, but not allocated | No desktop, shell, client, or effect content is offloaded | None; overlay offload remains disabled |

A missing cursor plane is not a startup failure. The cursor falls back to
software composition, which blocks direct scanout while the cursor is
visible.

Overlay availability is not proof that a layer can be offloaded. The current
policy requires a future planner to validate z-order, alpha, color, scaling,
format, modifier, damage, and synchronization in one atomic `TEST_ONLY`
request before overlay allocation becomes active.

## Primary-Plane State

Software state describes the content from the last successful presentation;
it does not describe a planned or attempted commit.

| Event | Resulting state |
|-------|-----------------|
| Startup or presentation-target invalidation | Unassigned |
| Successful compositor commit | Composited |
| Successful client direct commit | Direct scanout, owned by that surface |
| Failed direct or compositor commit | Unchanged |
| Output power loss, topology loss, or backend device-epoch loss | Unassigned |

Leaving direct scanout forces full compositor damage and invalidates cached
backdrop inputs. The software state changes to composited only after the
fallback framebuffer has been accepted.

## Window-Switcher Transition

The window-switcher action uses this sequence:

1. Mark the switcher as a composition requirement before planning the frame.
2. Reject direct scanout with `window-switcher`.
3. Render a full compositor frame containing the current client scene and the
   switcher.
4. Reclaim the primary plane after the compositor atomic commit succeeds.
5. Keep the existing client's fullscreen state and Wayland keyboard focus
   unchanged while previewing candidates.
6. Raise and focus the selected window when the switcher closes.
7. Re-enter direct scanout on a later frame only if the resulting scene passes
   every eligibility condition again.

Canceling the switcher retains the original focus. It does not send a
fullscreen configure round trip merely to display compositor chrome.

## Diagnostic Messages

| Log fragment | Interpretation |
|--------------|----------------|
| `plane allocation` | Reports assigned primary and cursor planes, discovered overlays, and the disabled overlay policy |
| `direct scanout active` | A client framebuffer successfully acquired the primary plane |
| `compositor reclaimed the primary plane` | A compositor framebuffer successfully replaced a direct client framebuffer |
| `direct scanout blocked` | A plausible full-output candidate failed an eligibility condition |
| `direct scanout unavailable` | The ordinary scene did not contain a plausible direct candidate |
| `direct scanout failed; compositing instead` | A non-policy backend error rejected the direct attempt and composition was selected |
| `cursor-plane ... rejected` | KMS rejected the cursor portion of the request; the next frame uses a software cursor |

Follow [How to Run aegis on Bare Metal](../how-to/bare-metal-drm.md) to verify
these transitions on real hardware.
