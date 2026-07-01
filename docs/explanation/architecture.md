# Architecture

ass is a Wayland compositor for Linux, written in Rust. It composites
client windows and draws its own shell chrome through
[flux](../../optics/flux), a Vulkan-first rendering engine, and
[lens](../../optics/lens), an immediate-mode UI engine that draws through
flux.

This page explains how the components fit together and where the project is
headed. For the product direction, see [Vision and Scope](vision.md); for
the milestone sequence, see [Roadmap](roadmap.md); for the decisions behind
the structure, see the [Architecture Decision Records](../adr/index.md).

## Responsibility Boundary

ass owns the server and platform halves of a compositor; flux and lens
own rendering and UI. The split is fixed in
[ADR-0001](../adr/0001-scope-and-responsibility-boundary.md).

| Concern | Owner |
|---------|-------|
| Wayland server protocol, globals, object lifecycle | ass |
| Input, output, session and seat management | ass |
| Window management, surface and scene model, focus | ass |
| GPU rendering, client buffer import as textures | flux |
| Compositor chrome (panels, decorations, overview) | lens |

flux is a client-side renderer: it presents into a caller-supplied
`VkSurfaceKHR` and has no windowing code. lens consumes input as a data
snapshot and emits draw calls. ass supplies both the surface and the
input.

## Crate Layout

ass is a Cargo workspace under `crates/`. The split keeps the server,
backend, renderer, and shell behind clear seams so the
[AI-adaptation phase](#roadmap) can grow a semantic model from
`ass-core`. The crates group into four roles:

| Role | Crate | Responsibility |
|------|-------|----------------|
| **Model** | `ass-core` | Backend- and renderer-agnostic model: geometry, surface tree, outputs, focus |
| | `ass-protocols` | Wayland protocol interface tables (xdg-shell), generated once and shared |
| **Server / window management** | `ass-server` | Hand-rolled Wayland server: socket, globals, protocol object lifecycle, focus, move/close, tiling, workspaces, xdg-output |
| | `ass-backend` | Presentation and input targets: the nested backend now, DRM/KMS + libinput + seat later |
| | `ass-render` | Compositing: client buffers to flux textures, scene to the output via flux |
| **Shell / interaction** | `ass-shell` | Compositor chrome host + components on lens: dock, launcher, workspace bar, decorations, toast |
| | `ass-wallpaper` | Background layer: multi-format image and short-video wallpaper |
| | `ass-config` | Declarative configuration: versioned TOML schema, loader, live reload |
| **Convenience channels** | `ass-apps` | freedesktop.org desktop-entry enumeration and icon-theme lookup |
| | `ass-launch` | Detached, XDG-environment-aware launching of desktop applications |
| | `ass-ipc` | Versioned IPC and introspection surface for ass over a unix socket |
| | `ass-ctl` | Command-line driver for the ass IPC (the reference external tool) |
| **Binary** | `ass` | The binary: wires the parts together and runs the event loop |

flux and lens are consumed through Rust bindings kept in separate
repositories from their C libraries, following the openssl-sys /
rusqlite convention: `flux-rs` (`flux` / `flux-sys`) and `lens-rs`
(`lens` / `lens-sys`). See [ADR-0023](../adr/0023-split-flux-lens-stack.md).

### Naming note: where the "user-facing" logic lives

The crate names are *mechanism-oriented*, which can make the product roles
hard to read at a glance. For the most common "I want to change what the
user sees or can do" tasks:

- **"Manage windows"** (focus, close, move, tile, workspace) → `ass-server`.
- **"Change the chrome / interactions"** (dock, launcher, bars) → `ass-shell`.
- **"Add an external control path"** (CLI, scripts, the agent) →
  `ass-ipc` + `ass-ctl`.
- **"Start or discover apps"** → `ass-apps` (discovery) + `ass-launch`
  (spawn). `ass-launch` is intentionally narrow: process detachment and
  environment, not window management.

## Backend Abstraction

A backend owns the presentation target and the raw input stream. The
nested backend runs ass as a client of an existing Wayland session and
presents into a host window; the planned DRM/KMS backend drives the display
hardware directly. Both implement one `Backend` trait so the server,
renderer, and shell are written once.

The nested backend, and the server itself, use raw libwayland over FFI
rather than a higher-level framework
([ADR-0002](../adr/0002-hand-rolled-wayland-server.md),
[ADR-0003](../adr/0003-nested-first-bring-up.md)). The nested host window
drives libwayland-client with xdg-shell interface tables generated from the
protocol definition, and `ash` creates the `VkSurfaceKHR` on flux's Vulkan
instance.

## Per-Frame Data Flow

In nested operation, each frame runs the following sequence:

1. The backend pumps host-window events, producing input events and resize
   or redraw signals, and holds the `VkSurfaceKHR`.
2. The server dispatches its event loop; clients commit surfaces and attach
   buffers (`wl_shm` or dmabuf), updating the surface tree in `ass-core`.
3. The renderer turns each mapped surface into a flux texture — dmabuf by
   zero-copy import, `wl_shm` by CPU upload — and composites them in
   z-order into the frame, then overlays the lens chrome. When a
   wallpaper is loaded (see [ADR-0018](../adr/0018-wallpaper-crate.md)),
   `ass-wallpaper` draws it as the bottom-most layer before the renderer
   runs.
4. The frame is submitted and presented to the host surface.
5. Input is routed: the backend's input goes to the focused client through
   `wl_seat`, with a copy to the chrome when the pointer is over it.
6. Client buffers are released once the GPU no longer needs them.

Client GPU buffers reach flux through a dmabuf import path added to flux
([ADR-0004](../adr/0004-client-buffers-via-flux-dmabuf-import.md)).

## Dependency Gaps

Building the compositor surfaced capabilities missing from the
dependencies. Each is placed by responsibility per
[ADR-0001](../adr/0001-scope-and-responsibility-boundary.md).

| Gap | Owner | Resolution |
|-----|-------|------------|
| Import client dmabuf as a texture | flux | dmabuf import API ([ADR-0004](../adr/0004-client-buffers-via-flux-dmabuf-import.md)) |
| Render target not tied to `VkSurfaceKHR` presentation (for DRM/KMS) | flux | External-image render path, future work |
| Rust bindings to flux and lens | bindings | `flux-rs` / `lens-rs` crates ([ADR-0023](../adr/0023-split-flux-lens-stack.md)) |
| Explicit synchronization for buffer release | flux and ass | Timeline semaphores plus the Wayland explicit sync protocol, future work |
| Wayland server, DRM/KMS, libinput, seat and session | ass | Implemented in ass ([ADR-0002](../adr/0002-hand-rolled-wayland-server.md)) |

flux does not auto-enable `VK_KHR_swapchain`; the nested backend requests it
explicitly, with the `VK_KHR_surface` and `VK_KHR_wayland_surface` instance
extensions.

## Roadmap

The full milestone sequence — from the completed nested bring-up through the
DRM/KMS backend, configuration and IPC, workspaces and layout, multi-output,
XWayland, polish, and the agent phase — lives in
[Roadmap](roadmap.md). The product direction behind it is
[Vision and Scope](vision.md), and the systems ass borrows from are surveyed
in [Comparative Survey](comparative-survey.md).

The summary table is kept here as a quick status reference; the verification
criteria and sequencing rationale are on the roadmap page.

| Milestone | Outcome |
|-----------|---------|
| M0 | Nested window: flux presents cleared frames with lens chrome. Complete. |
| M1 | Wayland server with the core globals; a real `wl_shm` client surface composited; input routed to the focused client. Complete: pointer and keyboard forward end-to-end with xkbcommon keymap and modifier state, click-to-focus, and shell input mirroring. |
| M2 | `zwp_linux_dmabuf_v1` with flux dmabuf import; GPU clients composited zero-copy. Implemented: per-surface position tracking, subsurface tree (direct children, above/below z-split), `wp_viewport` source crop and destination scale, `wl_surface.set_buffer_transform` via CPU staging (8 cases), and additional fourccs (ARGB/ABGR + X-variants). Buffer scale (`set_buffer_scale`) is stored but not yet applied at composite; nested subsurfaces and damage tracking remain. |
| M3 | Window management and richer chrome: multiple toplevels, focus, move and resize, decorations, overview. Implemented: toplevel metadata (title, app_id, parent, size hints), maximized/fullscreen/activated state with configure events, interactive move and resize with serial validation and size-hint clamping, a chrome window-list panel with click-to-focus and close, per-window server-side decorations (title bar + close gadget) drawn via flux-ui overlays with click-to-move, and a macOS-style bottom-center dock of per-window tiles. Border-drag resize and `xdg_toplevel.set_window_geometry` frame insets remain. |
| M4 | DRM/KMS backend with libinput and libseat for bare-TTY operation. |
| M5 and beyond | Configuration and IPC, workspaces and layout, multi-output, XWayland, polish, and the agent-adaptation layer. See [Roadmap](roadmap.md). |
