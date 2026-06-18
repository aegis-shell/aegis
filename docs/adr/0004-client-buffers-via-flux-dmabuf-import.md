# ADR-0004: Client buffers via flux dmabuf import

- Status: Accepted
- Date: 2026-06-04

## Context

A compositor turns each client's committed buffer into something it can
sample while compositing. Clients submit buffers in two main forms:
`wl_shm` (CPU memory) and `linux-dmabuf` (GPU memory shared by file
descriptor). GPU clients use dmabuf, and sampling those without a copy is
the path that matters for performance.

flux creates textures (`flux_image`) only from CPU data, through
`flux_image_create` with initial data or `flux_image_update_region`. It has
no path to wrap an external `VkImage` or import a dmabuf. Importing client
GPU buffers as textures is a rendering and texture concern, which
[ADR-0001](0001-scope-and-responsibility-boundary.md) places in flux.

## Decision

flux gains a dmabuf import API that turns a client dmabuf into a
`flux_image` usable by the canvas, implemented with
`VK_EXT_external_memory_dma_buf` and `VK_EXT_image_drm_format_modifier`.
ass composites client GPU buffers through this import path with no copy.
The capability is added to flux from the start rather than deferred behind a
copy-based interim.

`wl_shm` buffers continue to use flux's existing CPU upload path.

## Alternatives

- **shm-only first, dmabuf later.** Rejected: GPU clients are a primary
  case, and a copy-based interim would be thrown away.
- **Composite client buffers in ass at the raw-Vulkan seam, bypassing the
  canvas.** Rejected as the primary path: it duplicates compositing logic
  in ass and bypasses flux's canvas, contrary to
  [ADR-0001](0001-scope-and-responsibility-boundary.md). The raw seam
  remains available through the bindings for cases flux does not cover.

## Consequences

- flux exposes a new dmabuf import entry point and the format/modifier query
  needed to advertise `zwp_linux_dmabuf_v1` formats to clients.
- The flux device must enable the external-memory and modifier device
  extensions at creation; ass requests them.
- Buffer-release timing requires GPU/CPU synchronization between ass and
  clients; explicit synchronization (dmabuf fences, the Wayland explicit
  sync protocol) is follow-up work that may require flux to expose timeline
  semaphores.
