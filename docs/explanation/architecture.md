# Architecture

aegis is a Wayland compositor for Linux, written in Rust. It composites
client windows and draws its own shell chrome through
[flux](https://github.com/ming2k/optics/tree/main/libs/flux), a Vulkan-first
rendering engine, and
[lens](https://github.com/ming2k/optics/tree/main/libs/lens), an
immediate-mode UI engine that draws through flux.

This page explains how the components fit together and where the project is
headed. For the product direction, see [Vision and Scope](vision.md); for
the milestone sequence, see [Roadmap](roadmap.md); for the decisions behind
the structure, see the [Architecture Decision Records](../adr/index.md).

## Responsibility Boundary

aegis owns the server and platform halves of a compositor; flux and lens
own rendering and UI. The split is fixed in
[ADR-0001](../adr/0001-scope-and-responsibility-boundary.md).

| Concern | Owner |
|---------|-------|
| Wayland server protocol, globals, object lifecycle | aegis |
| Input, output, session and seat management | aegis |
| Window management, surface and scene model, focus | aegis |
| GPU rendering, client buffer import as textures | flux |
| Compositor chrome (panels, overview, notifications) | lens |

flux is a client-side renderer: it presents into a caller-supplied
`VkSurfaceKHR` and has no windowing code. lens consumes input as a data
snapshot and emits draw calls. aegis supplies both the surface and the
input.

## Crate Layout

aegis is a Cargo workspace under `crates/`. The split keeps the server,
backend, renderer, and shell behind clear seams so the
[AI-adaptation phase](#roadmap) can grow a semantic model from
`aegis-core`. The crates group by responsibility:

| Role | Crate | Responsibility |
|------|-------|----------------|
| **Model** | `aegis-core` | Backend- and renderer-agnostic model: geometry, surface graph, outputs, Realms, seats, and interaction authority |
| | `aegis-protocols` | Wayland extension interface tables, generated once and shared |
| **Server / window management** | `aegis-compositor` | Hand-rolled Wayland server: globals, protocol object lifecycle, per-Realm seats and outputs, focus, authority transfer, tiling, and workspaces |
| | `aegis-backend` | Presentation and input targets: nested (development) and DRM/KMS + libinput + libseat (bare TTY) |
| | `aegis-render` | Compositing: client buffers to flux textures, scene to the output via flux |
| **Shell / interaction** | `aegis-shell` | Compositor chrome host and `Chrome` contract on lens, plus shared components: launcher, overview, screenshot selector, toast |
| | `aegis-design` | Product design tokens, themes, and data-only surface materials shared by chrome components |
| | `aegis-dock` | Bottom-center dock chrome component: pinned and running apps, magnification, pin actions |
| | `aegis-ai-workspaces` | Compositor-owned Agent Realm lifecycle and authority management |
| | `aegis-settings` | Standalone modular System Settings application |
| | `aegis-hud` | Display-only HUD status chips: system status, workspace dots, clock, notification count, and the StatusNotifierItem tray row |
| | `aegis-command-panel` | Full-screen modal command panel: quick settings, tray activation, and notification dismissal |
| | `aegis-wallpaper` | Background layer: multi-format image and short-video wallpaper |
| | `aegis-avatar` | User-avatar loading and rendering: still images and VRM models |
| | `aegis-config` | Declarative configuration: versioned TOML schema, loader, live reload |
| **Session services** | `aegis-lock` | Multi-output session-lock presentation and PAM authentication |
| | `aegis-idle` | Ordered inactivity policy, lock-before-sleep coordination, and display-power requests |
| **Convenience channels** | `aegis-desktop-entries` | freedesktop.org desktop-entry enumeration and icon-theme lookup |
| | `aegis-launcher` | Ordinary app detachment and fail-closed Realm namespace/cgroup launch |
| | `aegis-ipc` | Versioned scoped IPC, sealed capture transport, and introspection over a Unix socket |
| | `aegis-ctl` | Command-line driver for the aegis IPC (the reference external tool) |
| **AI integration** | `aegis-mcp` | The platform's MCP bridge: scoped desktop tools and one bridge-managed Agent Realm for any agent (ADR-0087) |
| | `aegis-fuji` | fuji, the in-tree agent product: providers, agent loop, tools, MCP client, sessions, skills, permissions |
| **Binary** | `aegis` | The binary: wires the parts together and runs the event loop |

flux and lens are consumed through separately versioned Rust binding
workspaces in the Optics monorepo, following the openssl-sys / rusqlite
convention: `flux-rs` (`flux` / `flux-sys`) and `lens-rs`
(`lens` / `lens-sys`). Native libraries cross the repository boundary
through `pkg-config`; binding sources come from a locked release by default.
See
[ADR-0071](../adr/0071-worktree-isolated-cross-repository-development.md).

### Naming note: where the "user-facing" logic lives

The crate names are *mechanism-oriented*, which can make the product roles
hard to read at a glance. For the most common "I want to change what the
user sees or can do" tasks:

- **"Manage windows"** (focus, close, move, tile, workspace) → `aegis-compositor`.
- **"Change the chrome / interactions"** (dock, launcher, HUD, panel) → `aegis-shell`
  for the host and contract; the HUD and command panel live in the
  `aegis-hud` and `aegis-command-panel` component crates. The command panel
  owns live-system controls, Agent Workspaces has an independent
  compositor-owned component, and persistent settings run as the standalone
  `aegis-settings` application
  ([ADR-0060](../adr/0060-statusbar-system-controls-and-live-system-ipc.md),
  [ADR-0069](../adr/0069-documentation-owned-installation-and-throwaway-development-staging.md),
  [ADR-0045](../adr/0045-statusbar-crate-and-sni-tray.md)).
- **"Add an external control path"** (CLI or scripts) → `aegis-ipc` +
  `aegis-ctl`; agents consume that same IPC through the `aegis-mcp` bridge
  without entering the compositor process
  ([ADR-0047](../adr/0047-neenee-agent-realm-platform-bridge.md),
  [ADR-0050](../adr/0050-fuji-agent-product-and-bridge-rename.md),
  [ADR-0087](../adr/0087-aegis-mcp-standalone-platform-bridge-crate.md)). The
  compositor-owned Agent Workspaces surface reports generic Realm authority;
  it does not infer fuji process state
  ([ADR-0074](../adr/0074-generic-agent-workspaces-status-surface.md)).
- **"Start or discover apps"** → `aegis-desktop-entries` (discovery) + `aegis-launcher`
  (spawn). `aegis-launcher` is intentionally narrow: process detachment and
  environment, not window management.
- **"Lock or handle inactivity"** → `aegis-lock` owns presentation and
  authentication, while `aegis-idle` owns staged policy. The compositor
  retains protocol, input, inhibitor, output-power, and fail-closed authority
  ([ADR-0078](../adr/0078-out-of-process-idle-and-session-lock.md)).

## Settings Boundary

Persistent settings use the same state-in, intent-out direction as the rest
of the compositor without placing their presentation inside its process. The
System Settings reads a coherent settings snapshot over the scoped IPC and
returns typed edits with the revision it observed. The compositor remains the
authority that validates, persists, applies, journals, and publishes the next
revision.

A module owns one visible settings domain and its draft editor state. It does
not own the configuration file or the host service. This distinction keeps
the module catalog broad without pretending all settings belong to the
compositor: account modules use system account and authorization services.
The power module persists Aegis inactivity policy, while the supervised
policy client coordinates the host's backlight and logind services.
Compositor-owned display/input policy uses the Aegis settings IPC.

Volume, brightness, radios, Do Not Disturb, and current-workspace layout are
immediate service or session controls rather than persistent settings. The
command panel presents them, external clients use the live-system IPC, and
both paths converge on one runtime handler. Realm lifecycle is authority
management rather than configuration and retains the independent AI
Workspaces surface. The standalone System Settings app remains the canonical
persistent-settings UI. See
[ADR-0060](../adr/0060-statusbar-system-controls-and-live-system-ipc.md) and
the [System Settings Reference](../reference/settings.md).

## Session Lock and Inactivity

Session security crosses three lifetimes. The compositor has the longest
lifetime and owns the state that must fail closed: protocol acceptance,
exclusive input routing, idle inhibition, physical output power, and the
opaque scene shown when a confirmed locker disappears. The lock client has a
short authentication lifetime and owns only what the user sees and enters.
The idle coordinator has a replaceable policy lifetime and can restart when
settings change.

This separation keeps authentication and host power services out of the
compositor without delegating the security boundary. The idle coordinator
may request a lock, but it cannot claim that the lock is secure. It waits for
the lock client's readiness signal, which is emitted only after compositor
confirmation. Display power-off, suspend, and release of the logind delay
inhibitor occur after that boundary.

Activity reverses presentation policy in the opposite order: outputs wake
behind the secure frame, the backlight is restored, and authentication
remains necessary. A policy failure can wake a screen but cannot unlock it.
A lock-presentation failure can remove the client but cannot reveal normal
desktop content.

Direct sessions own the physical devices and system sleep transition. Nested
sessions retain the complete locking model but leave brightness, output
power, and suspend to the outer desktop. See
[ADR-0078](../adr/0078-out-of-process-idle-and-session-lock.md) and
[How to Configure Locking and Idle](../how-to/lock-and-idle.md).

## Backend Abstraction

A backend owns the presentation target and the raw input stream. The
nested backend runs aegis as a client of an existing Wayland session and
presents into a host window; the DRM/KMS backend drives the display
hardware directly with libinput input and libseat session ownership. Both
implement one `Backend` trait so the server, renderer, and shell are written
once.

Rendering and event dispatch have separate ownership. A submitted DRM frame
belongs to one **presentation domain** until every CRTC in its atomic batch
reports a page flip. Client requests, input, hotplug, and session events
continue during that interval, while visible changes coalesce into one next
redraw. The current domain spans all active outputs because they share one
desktop framebuffer and atomic commit. This preserves the real backend
boundary instead of pretending the outputs can retire independently. See
[ADR-0077](../adr/0077-presentation-domain-redraw-state-machine.md).

The nested backend, and the server itself, use raw libwayland over FFI
rather than a higher-level framework
([ADR-0002](../adr/0002-hand-rolled-wayland-server.md),
[ADR-0003](../adr/0003-nested-first-bring-up.md)). The nested host window
drives libwayland-client with xdg-shell interface tables generated from the
protocol definition, and `ash` creates the `VkSurfaceKHR` on flux's Vulkan
instance.

## Per-Frame Data Flow

Each frame follows this sequence:

1. The backend dispatches host, input, session, hotplug, and client-wakeup
   events. Dispatch remains live when a previous DRM frame is waiting for
   vblank.
2. The server accepts surface commits and attached `wl_shm` or dma-buf
   buffers. Input is routed through the owning Realm seat. Events received
   during an in-flight presentation preserve their edge information while
   coalescing into one next redraw.
3. A queued redraw opens one synchronous render transaction. The renderer
   imports or refreshes only changed client content, composites the mapped
   surface trees, and draws wallpaper and shell chrome. A no-damage result
   skips both rendering and presentation.
4. A successful nested submission returns pacing to the outer compositor. A
   successful DRM submission transfers the complete atomic batch to KMS and
   waits asynchronously for every CRTC page flip. Pending client frame
   callbacks complete on successful submission; callback-only work uses an
   estimated refresh boundary without creating an empty atomic commit.
5. VT loss or output recreation cancels the old presentation epoch. Resume
   rebuilds the backend resources and presents a full frame before incremental
   damage resumes.
6. Client buffers release once the GPU or display engine no longer needs
   them: against an explicit completion fence on DRM, or after enough later
   nested frames to retire every Flux slot.

The lifecycle and no-damage callback rules are recorded in
[ADR-0077](../adr/0077-presentation-domain-redraw-state-machine.md).
Incremental `wl_shm` refresh is recorded in
[ADR-0039](../adr/0039-damage-driven-shm-refresh.md), and reusable dma-buf
synchronization is recorded in
[ADR-0076](../adr/0076-linux-dmabuf-device-feedback-and-reusable-buffer-sync.md).

Client GPU buffers reach flux through a dma-buf import path added to flux
([ADR-0004](../adr/0004-client-buffers-via-flux-dmabuf-import.md)). The
client's graphics API is not the deciding boundary: OpenGL and Vulkan clients
can both export dma-bufs. linux-dmabuf v4 feedback identifies the DRM device
used by Flux's Vulkan physical device, so Mesa allocates on the same GPU;
version 3 remains the fallback when that identity is unavailable. See
[ADR-0076](../adr/0076-linux-dmabuf-device-feedback-and-reusable-buffer-sync.md).

## Clipboard Policy

Each seat has one explicit clipboard. Client selections and
compositor-owned payloads use the standard Wayland data-device path; an
interactive screenshot may publish PNG and file-URI representations to the
physical seat without affecting an agent Realm.

aegis deliberately does not advertise the X11-style Primary Selection. In this
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
| Reusable-buffer acquire synchronization and release | flux and aegis | Aegis transports each commit's acquire fence; Flux waits it per frame on cached imports; direct scanout uses KMS fences ([ADR-0076](../adr/0076-linux-dmabuf-device-feedback-and-reusable-buffer-sync.md)) |
| Wayland server, DRM/KMS, libinput, seat and session | aegis | Implemented in aegis ([ADR-0002](../adr/0002-hand-rolled-wayland-server.md)) |

flux does not auto-enable `VK_KHR_swapchain`; the nested backend requests it
explicitly, with the `VK_KHR_surface` and `VK_KHR_wayland_surface` instance
extensions.

## Roadmap

The full milestone sequence — from the completed nested bring-up through the
DRM/KMS backend, configuration and IPC, workspaces and layout, multi-output,
polish, and the agent phase — lives in
[Roadmap](roadmap.md). XWayland is descoped from the supported
configuration. The product direction behind it is
[Vision and Scope](vision.md), and the systems aegis borrows from are surveyed
in [Comparative Survey](comparative-survey.md).

The summary table has been retired: it duplicated the
[Roadmap](roadmap.md), which is the single living status page (per-milestone
outcomes, shipped state, and verification criteria). M0–M3 are complete; M4
(DRM/KMS) is code-complete pending hardware verification; M5/M6 are
complete; M7–M10 are in progress as recorded there, and XWayland is
descoped.
