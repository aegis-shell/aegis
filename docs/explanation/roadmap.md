# Roadmap

The milestone sequence aegis follows from its current state to a desktop a
human uses daily, and onward to the agent phase described in
[Vision and Scope](vision.md). Each milestone is verifiable before the next
begins. This page replaces the inline roadmap that used to live in
[Architecture](architecture.md); the table there now points here.

Milestones are ordered by dependency, not by calendar. Dates are not
committed; the verification criteria are.

## Status Legend

- **Complete** — shipped and exercised in the nested backend.
- **In progress** — partially landed; the remaining work is named.
- **Planned** — designed (linked ADR) but not started.
- **Future** — described at the level of intent; ADRs land when the
  milestone opens.

## Milestones

| Milestone | Outcome | Status |
|-----------|---------|--------|
| [M0](#m0-nested-bring-up) | Nested window: flux presents cleared frames with lens chrome | Complete |
| [M1](#m1-core-wayland-server) | Core globals, `wl_shm` client surface composited, input routed | Complete |
| [M2](#m2-gpu-client-buffers) | `zwp_linux_dmabuf_v1` with flux dmabuf import | Complete |
| [M3](#m3-window-management-and-first-chrome) | Window management and first-party chrome | Complete |
| [M4](#m4-drmkms-backend) | DRM/KMS backend with libinput and libseat | In progress (hardware verification) |
| [M5](#m5-configuration-and-ipc) | Declarative configuration and versioned IPC | Complete |
| [M6](#m6-workspaces-and-layout) | Dynamic per-output workspaces; floating with tiling policy | Complete (single-output) |
| [M7](#m7-multi-output-and-input-completeness) | Multi-output, mixed DPI, gestures, tablet, color | In progress |
| [M8](#m8-xwayland-and-application-coverage) | XWayland integration and broad application coverage | Descoped |
| [M9](#m9-polish-and-completeness) | Animations, overview, notifications, accessibility | In progress |
| [M10](#m10-the-agent-phase) | The agent adaptation layer | In progress (framing) |

## M0: Nested Bring-up

**Outcome.** aegis runs as a client of an existing Wayland session and flux
presents cleared frames into the host window, with lens chrome visible.

**Status.** Complete. See [ADR-0003](../adr/0003-nested-first-bring-up.md).

## M1: Core Wayland Server

**Outcome.** The hand-rolled Wayland server advertises the core globals, a
real `wl_shm` client surface is composited, and pointer and keyboard input
is routed to the focused client with xkbcommon keymaps and modifier state.

**Status.** Complete. See
[ADR-0002](../adr/0002-hand-rolled-wayland-server.md),
[ADR-0009](../adr/0009-input-pipeline-and-pointer-focus.md), and
[ADR-0010](../adr/0010-keyboard-pipeline-and-xkbcommon-ownership.md).

Popup grabs are scoped to their seat and preserve pointer events for the
owning client while dismissing outside clicks. Keyboard focus follows the
nearest xdg role rather than a child surface, so browser UI implemented as
either an `xdg_popup` or a `wl_subsurface` remains interactive without
displacing its owning toplevel.

## M2: GPU Client Buffers

**Outcome.** `zwp_linux_dmabuf_v1` is implemented with flux dmabuf import,
so GPU clients composite zero-copy. Subsurfaces, `wp_viewport` crop and
scale, `wl_surface.set_buffer_transform`, buffer scale, and per-commit damage
tracking are all in place.

**Status.** Complete. See [ADR-0004](../adr/0004-client-buffers-via-flux-dmabuf-import.md),
[ADR-0011](../adr/0011-subsurface-tree-and-z-split-rendering.md),
[ADR-0014](../adr/0014-buffer-transform-and-viewport-crop.md),
[ADR-0015](../adr/0015-damage-tracking.md), and
[ADR-0020](../adr/0020-buffer-scale-applied-at-composite.md). Explicit-sync
buffer release landed with `zwp_linux_explicit_synchronization_v1` (acquire
fences through Flux import and KMS `IN_FENCE_FD`; deferred `wl_buffer.release`).
Nested subsurfaces are walked recursively for both rendering and input: a
subsurface's own subsurfaces anchor in its buffer space, stack relative to
it, and receive pointer input directly when topmost.

## M3: Window Management and First Chrome

**Outcome.** Multiple toplevels with focus, interactive move and resize,
minimization, borderless compositor-owned controls, a window list, a
macOS-style dock, and an application launcher. Shell surfaces remain behind
the `Chrome` trait. Key bindings are configurable through the versioned TOML
configuration.
Real application icons render in the dock.

**Status.** Complete. Shipped: toplevel metadata and state machine
([ADR-0012](../adr/0012-toplevel-metadata-and-state-machine.md)),
interactive move and resize
([ADR-0013](../adr/0013-interactive-move-and-resize.md)),
shell ↔ server bridge
([ADR-0016](../adr/0016-shell-server-window-management-bridge.md)),
decoration policy
([ADR-0063](../adr/0063-compositor-owned-borderless-decoration-policy.md)),
dock ([ADR-0019](../adr/0019-dock-as-bottom-center-overlay.md)),
the `Chrome` trait ([ADR-0021](../adr/0021-chrome-component-trait.md)),
and the launcher ([ADR-0022](../adr/0022-application-launcher.md)).
Floating-window borders start edge and corner resize grabs, SVG desktop
icons are rasterized when librsvg is available, the application catalog is
refreshed while the compositor is running, and
`xdg_toplevel.set_window_geometry` frame insets are honored end to end —
client-decorated windows place, hit-test, and receive input by their visible
window rect, excluding shadow margins. Initial configuration restores
remembered geometry only for main windows, state-only configures preserve a
mapped window's size, and transient dialogs are centered without overwriting
their application's saved main-window geometry.

## M4: DRM/KMS Backend

**Outcome.** aegis drives display hardware directly from a bare TTY through a
DRM/KMS backend, with libinput for input and libseat for session and device
ownership. The nested backend remains for development. Both implement the
`Backend` trait, so the server, renderer, and shell are unchanged.

**Status.** In progress — code complete, pending hardware verification. The
backend abstraction ships in
([`aegis-backend`](../../crates/aegis-backend)) with two implementations behind
the `Backend` trait: nested (development) and DRM/KMS. The DRM backend does
atomic modesetting with a TEST_ONLY preflight, scans out Flux offscreen
dma-bufs (GBM-less) through a two-slot page-flip ring with explicit-sync
`IN_FENCE_FD` when the plane set allows, takes input from libinput
(pointer, keyboard, touch, touchpad gestures, tablet tools), owns the session
through libseat with VT switch suspend/resume, and handles udev hotplug with
per-connector workspace restore (ADR-0025) and surface recreation when the
modifier set changes. `AEGIS_BACKEND=auto|drm|nested` selects the target at
startup.

**Verification.** aegis starts from a TTY on a single monitor, lights the
display, and runs M3's chrome against real clients without a host session.
The known-risk paths to exercise first are live event dispatch while an
atomic batch waits for every CRTC, a VT round-trip with a flip in flight, and
monitor unplug/replug during presentation. Mixed-refresh monitors currently
share one presentation domain and therefore retire at its slowest CRTC; the
constraint and state lifecycle are recorded in
[ADR-0077](../adr/0077-presentation-domain-redraw-state-machine.md).

## M5: Configuration and IPC

**Outcome.** A single declarative TOML file provides a versioned schema and
full live reload. Versioned IPC over a unix socket exposes the same model the
shell reads, so external programs can query and mutate windows, workspaces,
outputs, and inputs.

**Status.** Complete. The configuration system shipped
(ADR-0026): one TOML file at `$XDG_CONFIG_HOME/aegis/config.toml`, schema
version 1, mtime live reload, and structured diagnostics. The IPC shipped its
full
seed surface (ADR-0027): versioned length-framed JSON over
`$XDG_RUNTIME_DIR/aegis.sock`, capability-gated handshake, `query`
(`GetWindows`, `GetWorkspaces`, `GetOutputs`, `GetNotifications`),
`control`/`session` commands (`Focus`/`Close`/`Move`/`Cycle`/
`SwitchWorkspace[To]`/`MoveToWorkspace`/`ToggleTiling`/`Quit`) applied on the
main loop, and `WindowsChanged`/`WorkspaceChanged`/`Notified` event streams.
Protocol version 4 extends the same boundary with revisioned persistent
settings snapshots, confirmed display/touchpad transactions, and settings
journal entries for the standalone modular System Settings application.
Layout, dock, UI policy, window rules, and agent scopes all load from the
same file and apply live. See
[ADR-0026](../adr/0026-configuration-system.md) and
[ADR-0027](../adr/0027-ipc-and-introspection.md).

**Verification.** Editing the config file changes behavior without a
restart and reports schema errors to the user. An external tool enumerates
windows and focuses one through the IPC. The existing keybinding parser and
matcher are reused unchanged as one consumer of the config.

**Why here.** Workspaces and tiling (M6) are configured through this file
and exercisable through this IPC; landing them first avoids a throwaway
configuration path.

## M6: Workspaces and Layout

**Outcome.** Dynamic per-output workspaces, with floating as the universal
base and an optional, policy-driven tiling layer applied on top. Window
rules from the configuration file drive placement and layout policy.

**Status.** In progress. The pure workspace/output model landed in
`aegis-core::workspace` (`WorkspaceModel`, `Workspace`, `Output`,
`WorkspaceId`/`OutputId`): dynamic per-output workspaces, the trailing-empty
invariant, empty-workspace reaping, toplevel place/remove/move, switch and
switch-to, and output-removal relocation — fully unit-tested in isolation.
The model is wired into the server: a toplevel maps onto the focused
output's current workspace, rendering and chrome see only the visible set,
switching (`Super+Left`/`Super+Right`) drops keyboard focus from a now-hidden
window, and removal reaps the emptied workspace. The IPC exposes
`GetWorkspaces`, `SwitchWorkspace`/`SwitchWorkspaceTo`, and a
`WorkspaceChanged` event (ADR-0027). A top-center workspace indicator
(HUD chrome component, hosted by the `aegis-hud` crate)
shows one numbered tile per workspace,
highlights the current, and switches on click. The tiling policy is
implemented end to end: a pure `aegis-core::layout` module (`LayoutRole`,
`LayoutParams`, the `Layout` trait, a `MasterStack` policy), a `layout_role`
field on `Window`, and server application — `Super+T` or the IPC
`ToggleTiling` command flips the current workspace to tiled, the master-stack
policy runs over the workspace's windows each frame, and clients are
reconfigured only when their target rect moves (steady state sends no
configures). Replug-restore (ADR-0025) is implemented in the model: each
workspace remembers its birth connector, survives an output unplug relocated
to a survivor, and returns home when the same connector is re-added —
unit-tested. See [ADR-0024](../adr/0024-layout-model.md) and
[ADR-0025](../adr/0025-workspace-model.md).

**M6 status: functionally complete for the single-output compositor.** The
previously remaining items have landed: server-side output hotplug ships with
the DRM/KMS backend (M4), including replug-restore against real connectors;
the per-workspace tiling default is configurable (`[layout] default_tiled`);
transient dialogs are floating-role exceptions, both at map time and during
the tiling sweep. What remains is polish: chrome-aware tiling margins for
chrome beyond the dock (which already reserves the bottom edge).

**Verification.** Each output has its own workspace set with an empty
workspace always available. A window can be tiled or floated independently.
Unplugging a monitor relocates its workspaces and restores them on replug.
The chrome shows the workspace state and the IPC exposes it.

## M7: Multi-Output and Input Completeness

**Outcome.** Per-output independent geometry with mixed DPI and fractional
scale through `wp_fractional_scale_v1`. Touchpad gestures, tablet support
with per-output mapping, and basic color management land with the libinput
backend.

**Status.** In progress. The per-output geometry model landed in
`aegis-core::output` (`OutputMode`, `Scale`, `OutputGeometry`) — see
[ADR-0028](../adr/0028-output-and-monitor-model.md) — and is now wired end to
end: backends report real connectors and geometry, the server advertises
per-connector `wl_output` (v4, with name/description) and `zxdg_output_v1`,
and the workspace model and tiling work-area track live output geometry.
`wp_fractional_scale_v1` and `wp_viewporter` are advertised. Touchpad
gestures (`zwp_pointer_gestures_v1`) and tablet tools
(`zwp_tablet_manager_v2`, full axis routing with pointer-emulation fallback)
land with the libinput backend.

**Remaining for M7.** Color management (needs color-space support in the
flux engine first). Multi-monitor rendering correctness and gesture/tablet
feel need real hardware to verify. The per-output display policy landed as
`[[output]]` config entries: scale, live DRM mode selection, position, and
primary are in effect and editable through System Settings; the output
transform is parsed but deferred until the renderer applies it.
System Settings now runs as an ordinary Wayland application; display and
touchpad pages share a module contract, while unfinished settings domains
remain visible but explicitly unavailable.

**Verification.** Two outputs at different scales render correctly and the
compositor's own chrome stays pixel-perfect. A touchpad three-finger swipe
switches workspaces. A tablet stylus maps to a chosen output.

## M8: XWayland and Application Coverage

**Status.** Descoped — XWayland is not in the support scope. X11
applications are out of scope for the supported configuration; the
integration strategy remains recorded in
[ADR-0030](../adr/0030-xwayland-strategy.md) should the decision ever be
revisited.

## M9: Polish and Completeness

**Outcome.** The remaining surfaces that make a desktop feel finished: a
declarative animation layer owned by lens with reduced-motion
([ADR-0029](../adr/0029-animation-and-effect-policy.md)), a unified overview
that combines window and workspace picking (the GNOME Activities / niri
Overview model), notifications served by the `Chrome` trait, screen lock and
idle, a screenshot and screencast path through `xdg-desktop-portal`, and
screen-reader and basic accessibility hooks.

**Status.** In progress. Landed: notifications served by the `Chrome` trait
(queue, IPC, toast and HUD chrome), screen lock
(`ext-session-lock-v1`, fail-closed with secure-frame confirmation) and idle
(`ext-idle-notify-v1` + `zwp-idle-inhibit-v1`), the reduced-motion half
of the animation policy ([ADR-0029](../adr/0029-animation-and-effect-policy.md)) —
one `[ui] reduced_motion` switch resolves every chrome and lens transition
in a single frame, live on reload — and the declarative transition mechanism
itself: non-interactive window geometry changes (tiling, IPC geometry)
record previous and target rectangles, publish them in the snapshot, and
interpolate at draw time, with subsurface trees glued to their root. The
unified overview (window grid + workspace rail, live thumbnails, click to
focus, `Super+O` or `aegis overview`) is in daily use, and a screenshot
path (`aegis display capture`, scoped `CaptureOutput` pixel capture per
[ADR-0041](../adr/0041-sealed-file-descriptor-pixel-transport.md)) covers the
single-frame half of the capture story. The independently developed and
packaged `xdg-desktop-portal-aegis` backend now serves Settings v1,
Screenshot v2, ScreenCast v3, idle-only Inhibit, Secret v1 with an at-rest
vault plus a transitional
`org.freedesktop.secrets` compatibility layer
([ADR-0085](../adr/0085-portal-secret-absorption-and-secret-service-compat.md)),
Lockdown, FileChooser v3, AppChooser v2, Email v2, Notification v2,
Account v1, DynamicLauncher v1, and Wallpaper v1. Compositor-owned operations
use scoped IPC
([ADR-0075](../adr/0075-independent-portal-package-and-backend-contract.md),
[ADR-0099](../adr/0099-resource-authority-and-out-of-process-file-chooser.md)).
Screenshot region selection, color picking, and monitor/window ScreenCast
selection use one compositor-owned interactive picker. Application and
secret-prompt dialogs remain native compositor chrome; FileChooser runs in a
portal-owned, one-shot GTK4 process and uses xdg-foreign-v2 only for transient
parenting. ScreenCast republishes the scoped output-frame stream
([ADR-0052](../adr/0052-scoped-output-frame-streaming.md)) as a PipeWire
producer. The backend does not advertise Background or persistent ScreenCast
grants until Aegis has the required policy UI, application tracking, and
PermissionStore integration; the routing default is now `aegis;gtk`, so
Aegis answers first everywhere and GTK only covers the few unsupported
interfaces (Access, Print, Location, Background). Still
planned: window open/close transitions, the workspace-switch slide,
zero-copy ScreenCast export, and screen-reader accessibility hooks.

**Verification.** Reduced-motion is respected end to end. The overview
lists every window across workspaces and switches to a chosen one.
Notifications appear and dismiss. The session locks and restores.

## M10: The Agent Phase

**Outcome.** The introspection and IPC work done for M5 is extended into an
automation contract: stable identifiers for every window, workspace, and
output; a journaled mutation log the agent can replay; and a capability
model that bounds what the agent may do. The agent is an IPC client with a
defined scope, never a special client of the compositor.

**Status.** In progress. Durable window ids, the mutation journal, fail-closed
named scopes and leases, deterministic floating-window geometry, bounded
target-local input, sealed-descriptor pixel capture, independent Interaction Domain seats
and virtual outputs, transferable interaction authority, damage-driven
observation, and cgroup-owned application sandboxes are implemented
([ADR-0040](../adr/0040-realms-seats-and-transferable-interaction-authority.md)
through
[ADR-0042](../adr/0042-mount-scoped-realm-portals-and-cgroup-sandboxes.md)).
The blueprint is in
[The Agent Phase](agent-phase.md); decisions are recorded in
[ADR-0031](../adr/0031-agent-as-scoped-ipc-client.md) and its follow-ons.
The remaining
desktop-dependent semantic surface (window-content capture per window,
semantic element trees) stays open.

The `aegis-mcp` integration now closes the client-side Interaction Domain loop:
fuji discovers scoped tools through MCP, while the bridge manages one
recoverable Agent Interaction Domain across application launch, authority transfer,
directed capture, bounded input, and revocation. The renamed fuji agent is
self-contained in this workspace; neither Praxion nor Neenee is required.
Voice activation and shell-native conversation chrome remain follow-up product
surfaces ([ADR-0047](../adr/0047-neenee-agent-realm-platform-bridge.md),
[ADR-0050](../adr/0050-fuji-agent-product-and-bridge-rename.md),
[ADR-0087](../adr/0087-aegis-mcp-standalone-platform-bridge-crate.md)).

## Sequencing Rationale

The order is forced by a few hard dependencies:

- M4 (DRM/KMS) is independent of M5/M6 and could be parallelized, but it is
  sequenced first because hardware brings up a class of bugs the nested
  backend hides.
- M5 (configuration and IPC) precedes M6 (workspaces and tiling) so the
  layout policy has a real home and is exercisable from outside.
- M6 precedes M7 because per-output workspaces assume a real output model,
  and exercising mixed DPI without workspaces hides the cases that matter.
- M8 (XWayland) is late because it depends on a stable window model and a
  working IPC to be exercisable meaningfully.
- M9 (polish) is last in the desktop phase because animations, overview,
  and notifications are the parts most visible to a user and least tolerant
  of an unstable core beneath them.
- M10 (agent) waits for the desktop phase so the contract is built on a
  model that has already been exercised by humans.

## See Also

- [Vision and Scope](vision.md) — the product direction the milestones
  deliver.
- [Architecture](architecture.md) — the component boundaries the milestones
  fill in.
- [Comparative Survey](comparative-survey.md) — where each milestone's ideas
  come from.
