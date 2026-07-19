# Architecture

ass is a Wayland compositor for Linux, written in Rust. It composites
client windows and draws its own shell chrome through
[flux](../../../optics/libs/flux), a Vulkan-first rendering engine, and
[lens](../../../optics/libs/lens), an immediate-mode UI engine that draws through
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
| **Model** | `ass-core` | Backend- and renderer-agnostic model: geometry, surface graph, outputs, Realms, seats, and interaction authority |
| | `ass-protocols` | Wayland extension interface tables, generated once and shared |
| **Server / window management** | `ass-server` | Hand-rolled Wayland server: globals, protocol object lifecycle, per-Realm seats and outputs, focus, authority transfer, tiling, and workspaces |
| | `ass-backend` | Presentation and input targets: nested (development) and DRM/KMS + libinput + libseat (bare TTY) |
| | `ass-render` | Compositing: client buffers to flux textures, scene to the output via flux |
| **Shell / interaction** | `ass-shell` | Compositor chrome host and `Chrome` contract on lens, plus shared components: launcher, workspace bar, overview, decorations, toast |
| | `ass-dock` | Bottom-center dock chrome component: pinned and running apps, magnification, pin actions |
| | `ass-control-center` | Compositor-owned Control Center chrome component: system status pages and typed system/Realm intents |
| | `ass-wallpaper` | Background layer: multi-format image and short-video wallpaper |
| | `ass-config` | Declarative configuration: versioned TOML schema, loader, live reload |
| **Convenience channels** | `ass-apps` | freedesktop.org desktop-entry enumeration and icon-theme lookup |
| | `ass-launch` | Ordinary app detachment and fail-closed Realm namespace/cgroup launch |
| | `ass-ipc` | Versioned scoped IPC, sealed capture transport, and introspection over a Unix socket |
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
- **"Change the chrome / interactions"** (dock, launcher, bars) → `ass-shell`
  for the host and contract; the dock and the Control Center live in the
  `ass-dock` and `ass-control-center` component crates ([ADR-0044](../adr/0044-dock-and-control-center-crates.md)).
- **"Add an external control path"** (CLI, scripts, the agent) →
  `ass-ipc` + `ass-ctl`.
- **"Start or discover apps"** → `ass-apps` (discovery) + `ass-launch`
  (spawn). `ass-launch` is intentionally narrow: process detachment and
  environment, not window management.

## Backend Abstraction

A backend owns the presentation target and the raw input stream. The
nested backend runs ass as a client of an existing Wayland session and
presents into a host window; the DRM/KMS backend drives the display
hardware directly with libinput input and libseat session ownership. Both
implement one `Backend` trait so the server, renderer, and shell are written
once.

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
   or redraw signals, and holds the `VkSurfaceKHR`. The loop blocks on
   these events when idle and waits with a ~60 fps deadline while
   animating; the presentation engine (FIFO swapchain acquire nested,
   the KMS page-flip wait on DRM) sets the real cadence
   ([ADR-0038](../adr/0038-frame-pacing.md)).
2. The server dispatches its event loop; clients commit surfaces and attach
   buffers (`wl_shm` or dmabuf), updating the surface tree in `ass-core`.
   shm contents are snapshotted at commit time, copying only the damaged
   rows when the frame's size and damage allow
   ([ADR-0039](../adr/0039-damage-driven-shm-refresh.md)).
3. The renderer turns each mapped surface into a flux texture — dmabuf by
   zero-copy import, `wl_shm` by CPU upload — refreshing only the damage
   bounding box for same-size commits, and composites them in
   z-order into the frame, then overlays the lens chrome. When a
   wallpaper is loaded (see [ADR-0018](../adr/0018-wallpaper-crate.md)),
   `ass-wallpaper` draws it as the bottom-most layer before the renderer
   runs.
4. The frame is submitted and presented to the host surface.
5. Input is routed through the physical Realm's seat. Agent input uses an
   independent Realm seat and never shares focus, modifiers, grabs,
   selection, drag-and-drop, or text-input state with the physical stream.
6. Client buffers are released once the GPU no longer needs them — against
   the completion fence on DRM, or a few frames late on nested
   ([ADR-0038](../adr/0038-frame-pacing.md)).

Client GPU buffers reach flux through a dmabuf import path added to flux
([ADR-0004](../adr/0004-client-buffers-via-flux-dmabuf-import.md)).

## Clipboard Policy

Each seat has one explicit clipboard. Client selections and
compositor-owned payloads use the standard Wayland data-device path; an
interactive screenshot may publish PNG and file-URI representations to the
physical seat without affecting an agent Realm.

ass deliberately does not advertise the X11-style Primary Selection. In this
interaction model, publishing text merely because it was highlighted is an
implicit global side effect and a duplicate clipboard channel. Capability
absence is reported honestly through the Wayland registry rather than through
an empty protocol object. See
[ADR-0043](../adr/0043-explicit-clipboard-only.md).

## Realm Authority

One compositor owns one surface graph. A **Realm** selects which interaction
groups it controls, which groups it observes, which seat state can send
input, and which physical or virtual output presents the result. Moving a
live window between Realms changes authority and scene selection; it does not
recreate or reparent the `wl_surface`.

The human desktop is Realm `1`. An agent Realm has an independent seat and
directed virtual output. A physical read-only mirror is rendered but excluded
from hit-testing and all window-control command paths. Clients without proven
native multi-seat behavior move as a complete interaction group, so a normal
single-instance application needs no app-side changes.

Applications started inside a Realm additionally receive a mount-scoped
Wayland portal and namespace/cgroup sandbox. That process boundary is
separate from transferring an already-running surface: compositor authority
can move immediately, while Linux namespaces cannot be applied
retroactively. See
[ADR-0040](../adr/0040-realms-seats-and-transferable-interaction-authority.md)
and
[ADR-0042](../adr/0042-mount-scoped-realm-portals-and-cgroup-sandboxes.md).

## Dependency Gaps

Building the compositor surfaced capabilities missing from the
dependencies. Each is placed by responsibility per
[ADR-0001](../adr/0001-scope-and-responsibility-boundary.md).

| Gap | Owner | Resolution |
|-----|-------|------------|
| Import client dmabuf as a texture | flux | dmabuf import API ([ADR-0004](../adr/0004-client-buffers-via-flux-dmabuf-import.md)) |
| Render target not tied to `VkSurfaceKHR` presentation (for DRM/KMS) | flux | Offscreen dma-buf render path (`flux::Surface::offscreen_dmabuf` + export) |
| Rust bindings to flux and lens | bindings | `flux-rs` / `lens-rs` crates ([ADR-0023](../adr/0023-split-flux-lens-stack.md)) |
| Explicit synchronization for buffer release | flux and ass | `zwp_linux_explicit_synchronization_v1` with acquire fences through flux import and KMS `IN_FENCE_FD` |
| Wayland server, DRM/KMS, libinput, seat and session | ass | Implemented in ass ([ADR-0002](../adr/0002-hand-rolled-wayland-server.md)) |

flux does not auto-enable `VK_KHR_swapchain`; the nested backend requests it
explicitly, with the `VK_KHR_surface` and `VK_KHR_wayland_surface` instance
extensions.

## Roadmap

The full milestone sequence — from the completed nested bring-up through the
DRM/KMS backend, configuration and IPC, workspaces and layout, multi-output,
polish, and the agent phase — lives in
[Roadmap](roadmap.md). XWayland is descoped from the supported
configuration. The product direction behind it is
[Vision and Scope](vision.md), and the systems ass borrows from are surveyed
in [Comparative Survey](comparative-survey.md).

The summary table has been retired: it duplicated the
[Roadmap](roadmap.md), which is the single living status page (per-milestone
outcomes, shipped state, and verification criteria). M0–M3 are complete; M4
(DRM/KMS) is code-complete pending hardware verification; M5/M6 are
complete; M7–M10 are in progress as recorded there, and XWayland is
descoped.
