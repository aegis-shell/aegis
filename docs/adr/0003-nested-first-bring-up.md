# ADR-0003: Nested-first bring-up, DRM/KMS later

- Status: Accepted
- Date: 2026-06-04

## Context

A compositor must present to a display and read input from devices. There
are two presentation targets: a **nested** backend, where ass runs as a
client of an existing Wayland or X11 session and presents into a host
window, and a **DRM/KMS** backend, where ass drives the display hardware
directly on a bare TTY with libinput and libseat.

flux presents through a `VkSurfaceKHR`. A nested Wayland host provides one
directly via `vkCreateWaylandSurfaceKHR`, so flux works unchanged. A
DRM/KMS target has no `VkSurfaceKHR`: it requires GBM-backed images, DRM
page-flipping, and session management, plus a flux path to render into
externally provided images rather than a swapchain.

## Decision

ass brings up the nested backend first and defers DRM/KMS. The nested
backend is the development target; the DRM/KMS backend is added later as a
second implementation of the same backend abstraction.

The nested host window is implemented in the raw style of
[ADR-0002](0002-hand-rolled-wayland-server.md): libwayland-client over FFI
with hand-declared xdg-shell interface tables generated from the protocol
XML, and `ash` to create the `VkSurfaceKHR` on flux's Vulkan instance.

## Alternatives

- **DRM/KMS first.** Rejected for bring-up: it requires flux changes for an
  externally backed render target plus seat and session management before a
  single frame can appear, with harder debugging on a bare TTY.
- **A windowing crate such as winit for the nested host.** Rejected: it
  pulls a heavy dependency and a foreign event model, inconsistent with the
  raw-FFI approach chosen for the server.

## Consequences

- A nested window presents flux frames and flux-ui chrome with no flux
  changes, giving a fast development loop.
- The backend abstraction must be defined so the server, renderer, and
  shell are written once against it and the DRM/KMS backend slots in later.
- DRM/KMS bring-up later requires a flux render-target path independent of
  `VkSurfaceKHR` presentation, tracked as a flux gap.
- flux does not auto-enable `VK_KHR_swapchain`; the nested backend passes it
  explicitly as a required device extension, alongside the `VK_KHR_surface`
  and `VK_KHR_wayland_surface` instance extensions.
