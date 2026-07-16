# Roadmap

The milestone sequence ass follows from its current state to a desktop a
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
| [M3](#m3-window-management-and-first-chrome) | Window management and first-party chrome | In progress |
| [M4](#m4-drmkms-backend) | DRM/KMS backend with libinput and libseat | Planned |
| [M5](#m5-configuration-and-ipc) | Declarative configuration and versioned IPC | In progress |
| [M6](#m6-workspaces-and-layout) | Dynamic per-output workspaces; floating with tiling policy | Complete (single-output) |
| [M7](#m7-multi-output-and-input-completeness) | Multi-output, mixed DPI, gestures, tablet, color | In progress (model) |
| [M8](#m8-xwayland-and-application-coverage) | XWayland integration and broad application coverage | Planned |
| [M9](#m9-polish-and-completeness) | Animations, overview, notifications, accessibility | Planned |
| [M10](#m10-the-agent-phase) | The agent adaptation layer | In progress (framing) |

## M0: Nested Bring-up

**Outcome.** ass runs as a client of an existing Wayland session and flux
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

## M2: GPU Client Buffers

**Outcome.** `zwp_linux_dmabuf_v1` is implemented with flux dmabuf import,
so GPU clients composite zero-copy. Subsurfaces, `wp_viewport` crop and
scale, `wl_surface.set_buffer_transform`, buffer scale, and per-commit damage
tracking are all in place.

**Status.** Complete. See [ADR-0004](../adr/0004-client-buffers-via-flux-dmabuf-import.md),
[ADR-0011](../adr/0011-subsurface-tree-and-z-split-rendering.md),
[ADR-0014](../adr/0014-buffer-transform-and-viewport-crop.md),
[ADR-0015](../adr/0015-damage-tracking.md), and
[ADR-0020](../adr/0020-buffer-scale-applied-at-composite.md). Remaining:
nested subsurfaces, explicit-sync buffer release.

## M3: Window Management and First Chrome

**Outcome.** Multiple toplevels with focus, interactive move and resize,
minimization, server-side decorations, a window list, a macOS-style dock,
and an application launcher, all behind the `Chrome` trait. Key bindings are
configurable through `$ASS_KEYBINDS`. Real application icons render in the
dock.

**Status.** In progress. Shipped: toplevel metadata and state machine
([ADR-0012](../adr/0012-toplevel-metadata-and-state-machine.md)),
interactive move and resize
([ADR-0013](../adr/0013-interactive-move-and-resize.md)),
shell ↔ server bridge
([ADR-0016](../adr/0016-shell-server-window-management-bridge.md)),
decorations ([ADR-0017](../adr/0017-server-side-decorations-via-overlays.md)),
dock ([ADR-0019](../adr/0019-dock-as-bottom-center-overlay.md)),
the `Chrome` trait ([ADR-0021](../adr/0021-chrome-component-trait.md)),
and the launcher ([ADR-0022](../adr/0022-application-launcher.md)).
Floating-window borders now start edge and corner resize grabs, SVG desktop
icons are rasterized when librsvg is available, and the application catalog
is refreshed while the compositor is running.

**Remaining for M3 close.** Honour
`xdg_toplevel.set_window_geometry` frame insets. This closes the remaining
protocol gap between "demo on the nested backend" and "a complete
floating-only desktop on the nested backend".

## M4: DRM/KMS Backend

**Outcome.** ass drives display hardware directly from a bare TTY through a
DRM/KMS backend, with libinput for input and libseat for session and device
ownership. The nested backend remains for development. Both implement the
`Backend` trait, so the server, renderer, and shell are unchanged.

**Status.** Planned. The backend abstraction is already in place
([`ass-backend`](../../crates/ass-backend)); the nested backend is the only
implementation. The DRM/KMS path needs explicit-sync render targets in flux
(noted as a dependency gap in [Architecture](architecture.md)).

**Verification.** ass starts from a TTY on a single monitor, lights the
display, and runs M3's chrome against real clients without a host session.

## M5: Configuration and IPC

**Outcome.** The placeholder `$ASS_KEYBINDS` environment variable is
replaced by a single declarative TOML file with a versioned schema and full
live reload. A versioned IPC over a unix socket exposes the same model the
shell reads, so external programs can query and mutate windows, workspaces,
outputs, and inputs.

**Status.** In progress, nearly complete. The configuration system shipped
(ADR-0026): one TOML file at `$XDG_CONFIG_HOME/ass/config.toml`, schema
version 1, mtime live reload, structured diagnostics, with `$ASS_KEYBINDS`
retained as a deprecated transitional override. The IPC shipped its full
seed surface (ADR-0027): versioned length-framed JSON over
`$XDG_RUNTIME_DIR/ass.sock`, capability-gated handshake, `query`
(`GetWindows`), `control`/`session` commands (`Focus`/`Close`/`Move`/`Cycle`/`Quit`)
applied on the main loop, and a `WindowsChanged` event stream. See
[ADR-0026](../adr/0026-configuration-system.md) and
[ADR-0027](../adr/0027-ipc-and-introspection.md).

**Remaining for M5 close.** Richer config sections as M6 lands; workspace
and output commands/events once those models exist. The seam itself is
complete.

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
`ass-core::workspace` (`WorkspaceModel`, `Workspace`, `Output`,
`WorkspaceId`/`OutputId`): dynamic per-output workspaces, the trailing-empty
invariant, empty-workspace reaping, toplevel place/remove/move, switch and
switch-to, and output-removal relocation — fully unit-tested in isolation.
The model is wired into the server: a toplevel maps onto the focused
output's current workspace, rendering and chrome see only the visible set,
switching (`Super+Left`/`Super+Right`) drops keyboard focus from a now-hidden
window, and removal reaps the emptied workspace. The IPC exposes
`GetWorkspaces`, `SwitchWorkspace`/`SwitchWorkspaceTo`, and a
`WorkspaceChanged` event (ADR-0027). A top-center workspace indicator
(`WorkspaceBar` chrome component) shows one numbered tile per workspace,
highlights the current, and switches on click. The tiling policy is
implemented end to end: a pure `ass-core::layout` module (`LayoutRole`,
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
remaining items are polish and M7-dependent: server-side output hotplug
(actual unplug/replug needs the multi-output backend, ADR-0028); per-workspace
tiling policy config; floating-role exceptions; chrome-aware tiling margins
(the dock now reserves the bottom edge; other chrome can follow).

**Verification.** Each output has its own workspace set with an empty
workspace always available. A window can be tiled or floated independently.
Unplugging a monitor relocates its workspaces and restores them on replug.
The chrome shows the workspace state and the IPC exposes it.

## M7: Multi-Output and Input Completeness

**Outcome.** Per-output independent geometry with mixed DPI and fractional
scale through `wp_fractional_scale_v1`. Touchpad gestures, tablet support
with per-output mapping, and basic color management land with the libinput
backend.

**Status.** In progress (model groundwork). The per-output geometry model
landed in `ass-core::output` (`OutputMode`, `Scale`, `OutputGeometry`) with
the logical-size derivation (physical mode, axis-swap for 90°/270°
transforms, divide by integer or fractional scale) — pure and unit-tested.
See [ADR-0028](../adr/0028-output-and-monitor-model.md).

**Remaining for M7.** Server/backend wiring: track real output geometry
from the backend, expose it to the workspace model and the tiling work-area,
and advertise `wp_fractional_scale_v1` + `wp_viewporter`. Touchpad gestures,
tablet mapping, and color management arrive with the libinput/DRM-KMS
backend (M4). Needs real multi-monitor or DRM hardware to verify.

**Verification.** Two outputs at different scales render correctly and the
compositor's own chrome stays pixel-perfect. A touchpad three-finger swipe
switches workspaces. A tablet stylus maps to a chosen output.

## M8: XWayland and Application Coverage

**Outcome.** XWayland is integrated as an optional, lazily-started backend.
X11 windows enter the same window model as Wayland toplevels, so focus,
workspaces, decorations, and the IPC treat them uniformly.

**Status.** Planned. See [ADR-0030](../adr/0030-xwayland-strategy.md).

**Verification.** A representative set of X11 applications (an IDE, a
browser, an emulator) run under ass with correct focus, decoration,
clipboard, drag-and-drop, and workspace placement.

## M9: Polish and Completeness

**Outcome.** The remaining surfaces that make a desktop feel finished: a
declarative animation layer owned by lens with reduced-motion
([ADR-0029](../adr/0029-animation-and-effect-policy.md)), a unified overview
that combines window and workspace picking (the GNOME Activities / niri
Overview model), notifications served by the `Chrome` trait, screen lock and
idle, a screenshot and screencast path through `xdg-desktop-portal`, and
screen-reader and basic accessibility hooks.

**Status.** Planned. The animation ADR is recorded; the rest are described
at the level of intent and will be expanded when the milestone opens.

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
named scopes, deterministic floating-window geometry, and bounded target-local
input are implemented. The blueprint is in
[The Agent Phase](agent-phase.md); decisions are recorded in
[ADR-0031](../adr/0031-agent-as-scoped-ipc-client.md) through
[ADR-0036](../adr/0036-scoped-semantic-automation.md). Pixel capture and the
remaining desktop-dependent semantic surface stay open.

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
