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
`aegis-model`. The crates group by responsibility:

| Role | Crate | Responsibility |
|------|-------|----------------|
| **Model** | `aegis-model` | Backend-, protocol-, and renderer-agnostic state plus deterministic model rules |
| | `aegis-security` | Transport-neutral Actor authority plus bounded, integrity-checked audit persistence |
| | `aegis-semantic` | Bounded accessibility-tree validation, window-local identity namespacing, provider ownership, and semantic action routing |
| | `aegis-wayland-protocols` | Wayland extension interface tables, generated once and shared |
| **Server / window management** | `aegis-compositor` | Hand-rolled Wayland server: globals, protocol object lifecycle, per-Interaction Domain seats and outputs, focus, authority transfer, tiling, and workspaces |
| | `aegis-backend` | Presentation and input targets: nested (development) and DRM/KMS + libinput + libseat (bare TTY) |
| | `aegis-render` | Compositing: client buffers to flux textures, scene to the output via flux |
| **Shell / interaction** | `aegis-shell` | Compositor chrome host and `Chrome` contract on lens, shared components, and the feature-gated `persona` profile/portrait domain |
| | `aegis-design` | Product design tokens, themes, and data-only surface materials shared by chrome components |
| | `aegis-dock` | Bottom-center dock chrome component: pinned and running apps, magnification, pin actions |
| | `aegis-settings` | Settings module library: module contract, registry, and built-in pages hosted by the command panel |
| | `aegis-hud` | Display-only HUD status chips: system status, workspace dots, clock, notification count, and the StatusNotifierItem tray row |
| | `aegis-command-panel` | Full-screen modal command panel: quick settings, hosted settings modules, tray activation, and notification dismissal |
| | `aegis-wallpaper` | Background layer: image, video, 3D, and multi-plane parallax wallpaper |
| | `aegis-config` | Declarative configuration: versioned TOML schema, loader, live reload |
| **Session services** | `aegis-lock` | Multi-output session-lock presentation and PAM authentication |
| | `aegis-idle` | Ordered inactivity policy, lock-before-sleep coordination, and display-power requests |
| | `aegis-atspi` | Supervised out-of-process AT-SPI tree and semantic-action adapter with a session-only system principal |
| **Convenience channels** | `aegis-desktop-entries` | freedesktop.org desktop-entry enumeration and icon-theme lookup |
| | `aegis-launcher` | Ordinary app detachment and fail-closed Interaction Domain namespace/cgroup launch |
| | `aegis-ipc` | Native capability broker contract: versioned identity, scopes, leases, Interaction Domain authority, sealed capture transport, and introspection over a Unix socket |
| | `aegis-commands` | Domain-oriented parser and IPC dispatcher behind native `aegis` management commands |
| **AI integration** | `aegis-mcp` | Stateless MCP `2026-07-28` adapter over the native broker, with one subject-bound Agent Interaction Domain per connector instance (ADR-0090) |

| **Binary** | `aegis` | The binary: wires the parts together and runs the event loop |

The optional xdg-desktop-portal backend is developed in the independent
[xdg-desktop-portal-aegis repository](https://github.com/aegis-shell/xdg-desktop-portal-aegis).
It remains an explicitly version-pinned Aegis companion because its scoped
IPC mechanisms move with the compositor, while its D-Bus, PipeWire,
encrypted-secret, and PAM dependencies evolve outside the core source
workspace. See
[ADR-0095](../adr/0095-independent-portal-repository-and-component-workspace.md).

flux and lens are consumed through separately versioned Rust binding
workspaces in the Optics monorepo, following the openssl-sys / rusqlite
convention: `flux-rs` (`flux` / `flux-sys`) and `lens-rs`
(`lens` / `lens-sys`). Native libraries cross the repository boundary
through `pkg-config`; binding sources come from a locked release by default.
See
[ADR-0071](../adr/0071-worktree-isolated-cross-repository-development.md).

## Actor Boundary

Agent support is an authority projection across existing compositor domains,
not an Agent runtime inside the shell:

```text
Wayland compositor
└── native capability broker
    ├── human Actor context
    └── Agent Actor context
        ├── identity: authenticated principal + bounded Actor session
        ├── capability: independent operations + exact resource grants
        ├── view/input: Interaction Domain output + independent seat + authority
        ├── observation: filtered semantic snapshot + single-use token
        └── storage/network: isolated sandbox + grant-consuming brokers
```

`aegis-model` owns durable semantic and Interaction Domain models;
`aegis-security` owns Actor sessions, capabilities, resource grants,
observation leases, transaction preconditions, and integrity-checked audit
persistence; `aegis-semantic` validates
untrusted application accessibility trees; `aegis-ipc` carries those
contracts; and `aegis-compositor` derives the view and routes each independent
seat. The binary runtime assembles and revokes the live facets, while the
security audit module persists privacy-minimized decisions. `aegis-atspi`
performs D-Bus/toolkit work outside the compositor and rechecks target state
immediately before dispatch. It binds AT-SPI applications to Wayland windows
by equal kernel/D-Bus process identity plus an exact title; ordinary observers
never receive process ids. Reasoning, planning, and long-term memory remain in
out-of-tree agent runtimes.

Observation and action are separate capability families. A semantic Interaction Domain
observation is bound to the Actor, authority revision, and complete target
state. The action consumes that observation once; state change aborts the
batch before dispatch. Framebuffer capture is an independently authorized
fallback and never substitutes for a semantic precondition. See
[ADR-0102](../adr/0102-actor-scoped-semantic-observation-and-transactional-actions.md)
and [ADR-0104](../adr/0104-actor-sessions-resource-grants-and-accessibility-adapter.md).

## Naming Note: Where the "User-Facing" Logic Lives

The crate names are *mechanism-oriented*, which can make the product roles
hard to read at a glance. For the most common "I want to change what the
user sees or can do" tasks:

- **"Manage windows"** (focus, close, move, tile, workspace) → `aegis-compositor`.
- **"Change the chrome / interactions"** (dock, launcher, HUD, panel) → `aegis-shell`
  for the host and contract; the HUD and command panel live in the
  `aegis-hud` and `aegis-command-panel` component crates. The command panel
  owns live-system controls, the display-only Agent Workspaces status row,
  and the persistent settings pages hosted from the `aegis-settings`
  module library
  ([ADR-0114](../adr/0114-panel-hosted-settings-and-hud-command-panel.md),
  [ADR-0060](../adr/0060-statusbar-system-controls-and-live-system-ipc.md),
  [ADR-0045](../adr/0045-statusbar-crate-and-sni-tray.md)).
- **"Add an external control path"** (CLI or scripts) → `aegis-ipc` +
  native `aegis` resource commands; agents consume that same IPC through the
  `aegis-mcp` bridge
  without entering the compositor process
  ([ADR-0047](../adr/0047-neenee-agent-realm-platform-bridge.md),
  [ADR-0087](../adr/0087-aegis-mcp-standalone-platform-bridge-crate.md)). The
  command panel's Agent Workspaces status row reports generic Interaction Domain
  authority; it does not infer any external agent's process state
  ([ADR-0074](../adr/0074-generic-agent-workspaces-status-surface.md)).
- **"Start or discover apps"** → `aegis-desktop-entries` (discovery) + `aegis-launcher`
  (spawn). `aegis-launcher` is intentionally narrow: process detachment and
  environment, not window management.
- **"Lock or handle inactivity"** → `aegis-lock` owns presentation and
  authentication, while `aegis-idle` owns staged policy. The compositor
  retains protocol, input, inhibitor, output-power, and fail-closed authority
  ([ADR-0078](../adr/0078-out-of-process-idle-and-session-lock.md)).

## Settings Boundary

Persistent settings follow the same state-in, intent-out direction as the
rest of the compositor. The command panel hosts the settings pages
in-process: each module reads a coherent revisioned snapshot pushed through
the chrome update channel and returns typed edits with the revision it
observed, and the main loop commits them through the same path that serves
the settings IPC. The compositor remains the authority that validates,
persists, applies, journals, and publishes the next revision.

A module owns one visible settings domain and its draft editor state. It does
not own the configuration file or the host service. This distinction keeps
the module catalog broad without pretending all settings belong to the
compositor: account modules use system account and authorization services.
The power module persists Aegis inactivity policy, while the supervised
policy client coordinates the host's backlight and logind services.
Compositor-owned display/input policy uses the same commit path whether the
edit arrives from the panel or from an external IPC client.

Volume, brightness, radios, Do Not Disturb, and current-workspace layout are
immediate service or session controls rather than persistent settings. The
command panel presents them on its System tab, external clients use the
live-system IPC, and both paths converge on one runtime handler. Interaction
Domain lifecycle is authority management rather than configuration and is
managed through the `aegis interaction-domain *` CLI and the
`interaction_domain_*` MCP tools. The command panel is the canonical
persistent-settings UI. See
[ADR-0114](../adr/0114-panel-hosted-settings-and-hud-command-panel.md),
[ADR-0060](../adr/0060-statusbar-system-controls-and-live-system-ipc.md), and
the [Settings Reference](../reference/settings.md).

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

In a direct session, display ownership determines renderer ownership. The
KMS primary node granted by libseat identifies the physical GPU, and Flux
must select a Vulkan device whose primary or render-node identity belongs to
that GPU. A mismatch is a startup error rather than an implicit cross-GPU
copy path. This keeps dma-buf modifiers, synchronization, power policy, and
scanout under one explicit device boundary. See
[ADR-0100](../adr/0100-strict-kms-vulkan-device-affinity.md).

Rendering and event dispatch have separate ownership. A submitted DRM frame
belongs to one **presentation domain** until every CRTC in its atomic batch
reports a page flip. Client requests, input, hotplug, and session events
continue during that interval, while visible changes coalesce into one next
redraw. The current domain spans all active outputs because they share one
desktop framebuffer and atomic commit. This preserves the real backend
boundary instead of pretending the outputs can retire independently. See
[ADR-0077](../adr/0077-presentation-domain-redraw-state-machine.md).

### Presentation Plans and Plane Roles

Direct sessions have two presentation paths. The normal path composites the
desktop, live glass, shell chrome, protocol overlays, and capture work into a
framebuffer for the primary plane. The fast path assigns one opaque,
full-output client buffer directly to that plane when the visible scene needs
nothing else. Eligibility follows the pixels and synchronization contract,
not merely a window's fullscreen state.

Every active output has one assigned primary plane and may have an independent
cursor plane. Overlay planes are discovered but remain compositor-owned policy:
arbitrary desktop layers and backdrop-dependent effects are not offloaded
without a complete atomic capability proof. Opening the window switcher makes
composition mandatory, so the fullscreen buffer yields the primary plane while
its protocol state remains intact. See
[ADR-0101](../adr/0101-dual-presentation-paths-and-conservative-kms-plane-allocation.md).
The exact eligibility conditions, rejection labels, and diagnostic messages
are listed in the
[Rendering and KMS Plane Reference](../reference/rendering.md).

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
   buffers. Input is routed through the owning Interaction Domain seat. Events received
   during an in-flight presentation preserve their edge information while
   coalescing into one next redraw.
3. A queued redraw plans primary-plane ownership from the complete visible
   scene. One eligible client buffer takes the direct path. Every other scene
   opens a synchronous render transaction, imports or refreshes changed client
   content, composites the mapped surface trees, and draws wallpaper and shell
   chrome. A no-damage result skips both rendering and presentation.
4. A successful nested submission returns pacing to the outer compositor. A
   successful DRM submission transfers either primary-plane source, plus the
   cursor plane when available, as one atomic batch and waits asynchronously
   for every CRTC page flip. Pending client frame callbacks complete on
   successful submission; callback-only work uses an estimated refresh
   boundary without creating an empty atomic commit.
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
physical seat without affecting an agent Interaction Domain.

aegis deliberately does not advertise the X11-style Primary Selection. In this
interaction model, publishing text merely because it was highlighted is an
implicit global side effect and a duplicate clipboard channel. Capability
absence is reported honestly through the Wayland registry rather than through
an empty protocol object. See
[ADR-0043](../adr/0043-explicit-clipboard-only.md).

## Interaction Domain Authority

One compositor owns one surface graph. An **Interaction Domain** selects which interaction
groups it controls, which groups it observes, which seat state can send
input, and which physical or virtual output presents the result. Moving a
live window between Interaction Domains changes authority and scene selection; it does not
recreate or reparent the `wl_surface`.

The human desktop is Interaction Domain `1`. An agent Interaction Domain has an independent seat and
directed virtual output. A window launched directly in an Agent Interaction Domain is
presented to the human Interaction Domain as a read-only observer mirror by default. A
transferred window keeps the same physical mirror unless the transfer
explicitly removes it. Visibility does not grant control: the human seat
remains excluded from client input and every window-control command path.

The physical mirror carries a compositor-owned controlled-window guard. Its
subdued presentation, authority label, and `not-allowed` pointer communicate
that the window is visible for supervision rather than available for use.
The server-side Interaction Domain model remains the authority boundary; the guard is a
trusted explanation of that state and an additional physical pointer barrier.
Successful Agent operations use a separate ephemeral marker above the guard,
so persistent ownership and recent activity remain distinct signals.

Clients without proven native multi-seat behavior move as a complete
interaction group, so a normal single-instance application needs no app-side
changes. Removing the human observer hides the complete group from the
physical scene and physical window snapshot rather than leaving inert chrome.

Applications started inside an Interaction Domain additionally receive a mount-scoped
Wayland portal and namespace/cgroup sandbox. That process boundary is
separate from transferring an already-running surface: compositor authority
can move immediately, while Linux namespaces cannot be applied
retroactively. See
[ADR-0103](../adr/0103-actor-authority-and-interaction-domain-architecture.md),
[ADR-0040](../adr/0040-realms-seats-and-transferable-interaction-authority.md),
[ADR-0042](../adr/0042-mount-scoped-realm-portals-and-cgroup-sandboxes.md),
[ADR-0048](../adr/0048-compositor-owned-agent-operation-feedback.md), and
[ADR-0091](../adr/0091-agent-controlled-window-physical-mirrors-and-guard.md).

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
