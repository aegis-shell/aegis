# ADR-0076: Linux-dmabuf device feedback and reusable-buffer synchronization

- Status: Accepted
- Date: 2026-07-29

## Context

[ADR-0004](0004-client-buffers-via-flux-dmabuf-import.md) established
zero-copy import of client GPU buffers, but advertising only
`zwp_linux_dmabuf_v1` version 3 exposed formats and modifiers without
identifying the compositor's GPU. Mesa's Wayland EGL path could therefore
fail to obtain a DRM device for OpenGL clients and fall back to `llvmpipe`.
The compositor still rendered through Vulkan, which made compositor frame
timing alone misleading: a client could be software-rendered before aegis
ever sampled its buffer.

OpenGL versus Vulkan is not the interoperability boundary. Either client API
can allocate a dma-buf that Flux imports through Vulkan. The required
boundary is a shared DRM device identity, compatible format and modifier
sets, and correct synchronization between the producer, compositor, and KMS.

Wayland clients also reuse a small swapchain, normally cycling through two to
four `wl_buffer` objects. Re-importing a Vulkan image on every commit honors a
new acquire fence but discards that reuse and adds avoidable allocation and
driver work. Direct scanout adds a second constraint: a buffer may bypass
Flux only when KMS accepts its format and modifier, and it cannot be released
until the display engine finishes using it.

## Decision

aegis advertises linux-dmabuf version 4 when it can identify the DRM device
used by the Flux Vulkan physical device. It queries
`VK_EXT_physical_device_drm`, prefers the render node, and uses the DRM/KMS
primary node only as a direct-backend fallback. If no reliable device is
available, aegis retains the version 3 compatibility path.

Version 4 feedback uses a sealed native format-table file descriptor built
from the formats and modifiers Flux reports as importable. Default and
per-surface feedback identify the selected device and publish a render
tranche. A version 4 client receives feedback objects rather than the
deprecated version 3 `format` and `modifier` events.

The server assigns a stable monotonic identity to every accepted
`wl_buffer`. The renderer keeps a bounded cache of up to eight imported
dma-buf images per surface, keyed by surface and buffer identity. When a
client recommits an existing buffer with a new acquire fence, aegis asks Flux
to wait that fence for the current canvas frame instead of rebuilding the
Vulkan image. Flux owns Vulkan semaphore import and queue-wait mechanics;
aegis owns Wayland buffer identity, fence transport, cache policy, and
release timing.

Direct scanout is allowed only for the intersection of client, Flux, and KMS
format/modifier support. aegis uses the actual KMS out-fence as the
completion fence and releases the client buffer only after that fence
retires.

## Alternatives

- **Require clients to use Vulkan or change their renderer.** Rejected:
  OpenGL clients interoperate through dma-buf, and changing application
  settings would conceal incorrect compositor device feedback.
- **Keep version 3 format/modifier advertisement.** Rejected: it cannot tell
  Mesa which DRM device matches the compositor and permitted a silent
  software-rendering fallback.
- **Re-import every committed buffer.** Rejected: it is correct but defeats
  normal client swapchain reuse and adds allocation work to the frame path.
- **Move Wayland feedback and KMS policy into Flux.** Rejected:
  [ADR-0001](0001-scope-and-responsibility-boundary.md) assigns platform
  protocol and display ownership to aegis; Flux owns renderer capabilities
  and GPU synchronization primitives.

## Consequences

- Mesa OpenGL and Vulkan clients can select the same GPU as Flux. Older
  clients retain the version 3 fallback.
- Performance investigations must confirm the client's reported renderer
  before profiling compositor pacing. `llvmpipe` is a client allocation or
  device-feedback failure, not evidence that OpenGL itself is unsuitable.
- The Aegis renderer depends on the corresponding Flux per-frame dma-buf
  acquire-wait API. Local cross-repository work follows
  [ADR-0071](0071-worktree-isolated-cross-repository-development.md); a
  canonical Aegis dependency update follows an Optics release.
- Default and per-surface feedback currently publish the same render
  tranche. Future scanout-specific tranches can refine that policy without
  changing the ownership boundary.
- Buffer release remains tied to presentation completion as established by
  [ADR-0038](0038-frame-pacing.md), now using the real KMS completion fence
  on direct display.
