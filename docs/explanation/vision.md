# Vision and Scope

tessera is a Wayland compositor and graphical shell for Linux, written in Rust
on [flux](https://github.com/ming2k/optics/tree/main/libs/flux) and
[lens](https://github.com/ming2k/optics/tree/main/libs/lens). This page
states what the project is for, what it is not for, and the design principles
that keep it small as it grows toward the feature richness of a full desktop
shell. The concrete milestone sequence is [Roadmap](roadmap.md); the
decisions behind the principles are
[Architecture Decision Records](../adr/index.md); the systems tessera borrows
from are surveyed in [Comparative Survey](comparative-survey.md).

## The Two Phases

tessera has two sequential goals. The first is a good desktop for human users.
The second adapts the same compositor so an AI agent can understand and
operate the machine through it.

The second phase is not a bolted-on assistant. It is the reason the
window-management model, the introspection surface, and the IPC seam are
designed the way they are from the start. A compositor built only for human
input accumulates implicit state — focus heuristics, chrome-internal
shortcuts, unlogged mutations — that an agent cannot reach. tessera keeps that
state explicit and queryable, so the same model the chrome renders and the
human drives is the model the agent reads and acts on.

## Product Target

The near-term target is feature parity with a mainstream Linux shell at the
level that affects daily use: a coherent panel and dock, a launcher and
overview, dynamic per-output workspaces, floating with optional tiling,
multi-monitor with mixed DPI, configuration with live reload, animations
that do not fight the user, and broad Wayland application compatibility.
The reference point is GNOME Shell's reach; the constraint is that
tessera reaches it with a fraction of the code and a single coherent model.

"More simple and more pure" is the differentiator. Simple means a small
core that owns its responsibilities directly rather than wrapping a toolkit.
Pure means each concern has one home: the compositor owns the Wayland server
and window model, flux owns rendering, lens owns the immediate-mode UI, and
the chrome is a pluggable component. Nothing is duplicated across seams.

## Design Principles

The principles are durable. Specific mechanisms may change, but every
addition is checked against them.

**One model, many readers.** The window, workspace, output, and input state
lives once, in `tessera-model`. The renderer, the chrome, the IPC, and the later
agent layer all read the same snapshot. State is never reconstructed for a
second consumer, because two copies of the truth always diverge. This is
already the basis of the `Chrome` trait
([ADR-0021](../adr/0021-chrome-component-trait.md)) and will be the basis of
the introspection API ([ADR-0027](../adr/0027-ipc-and-introspection.md)).

**The chrome is a component, not a privilege.** First-party chrome (dock,
launcher, overview, window list) is registered into the shell at startup and
can be replaced or omitted. Window-decoration ownership remains compositor
policy rather than a second chrome implementation. This keeps the core small
and lets the agent or an alternative shell replace the human-facing chrome
without forking the compositor.

**Configuration is data, not code.** One declarative file, one versioned
schema, full live reload. tessera does not embed a scripting language. Power and
automation go through a versioned IPC, the same path the agent uses. This is
the explicit rejection of GNOME's fragmented stores, sway's mixed-syntax
file, and river's unstructured startup script
([ADR-0026](../adr/0026-configuration-system.md)).

**Rendering and UI are delegated.** flux renders; lens draws the chrome. tessera
does not grow a scene graph, a text shaper, or an animation framework in
tree. When a capability is missing, it is added where it belongs
([ADR-0001](../adr/0001-scope-and-responsibility-boundary.md)).

**Wayland-only.** tessera will never ship an X11 session, and XWayland is
descoped from the supported configuration: X11 applications are simply
unsupported. The integration strategy remains recorded should it ever be
revisited ([ADR-0030](../adr/0030-xwayland-strategy.md)).

**Borrow deliberately.** Every borrowed idea is named against its source and
checked against the principles. The [Comparative Survey](comparative-survey.md)
records what tessera takes and what it leaves.

## Scope

The following are in scope for the desktop phase:

- The Wayland server, the seat, the output and input pipelines, and the
  window manager, owned directly by tessera
  ([ADR-0002](../adr/0002-hand-rolled-wayland-server.md)).
- A DRM/KMS backend for bare-TTY operation alongside the nested backend
  ([ADR-0003](../adr/0003-nested-first-bring-up.md)).
- A coherent first-party shell: panel, dock, launcher, overview,
  notifications, and wallpaper as `Chrome` components, with borderless
  window controls owned by the compositor.
- Dynamic per-output workspaces
  ([ADR-0025](../adr/0025-workspace-model.md)) and a floating-first layout
  with optional tiling ([ADR-0024](../adr/0024-layout-model.md)).
- A single declarative configuration file with live reload
  ([ADR-0026](../adr/0026-configuration-system.md)).
- A versioned IPC and introspection surface
  ([ADR-0027](../adr/0027-ipc-and-introspection.md)).
- Multi-output with mixed DPI and fractional scale
  ([ADR-0028](../adr/0028-output-and-monitor-model.md)).
- A declarative animation layer owned by lens, with reduced-motion
  ([ADR-0029](../adr/0029-animation-and-effect-policy.md)).

## Non-Goals

To stay small, tessera explicitly does not aim to provide the following:

- **X11 applications.** tessera is Wayland-only; XWayland
  ([ADR-0030](../adr/0030-xwayland-strategy.md)) is descoped.
- **A bundled application suite.** No file manager, terminal emulator, text
  editor, image viewer, or web browser. The launcher discovers what the host
  installed.
- **An in-process extension runtime.** No JavaScript, QML, or Lua inside the
  compositor. All automation is out-of-process over IPC.
- **A second renderer.** flux is the renderer. There is no fallback software
  path beyond what flux itself provides.
- **A settings GUI in the compositor.** Configuration is a file. A separate
  GUI may exist later as a normal Wayland client, but it is not part of the
  compositor.
- **Mobile, tablet, and TV shells as built-in variants.** The `Chrome` trait
  makes such shells possible as separate compositions; tessera ships only the
  desktop composition.

## The Agent Phase

The agent phase begins once the desktop phase is stable. Its premise is that
the work done for the desktop — one explicit model, a versioned IPC, chrome
as a component — is most of the work an agent needs. The remaining work is
an automation contract layered on the IPC: stable identifiers for windows
and workspaces, a journal of mutations the agent can replay, and a
capability model so the agent can act only where permitted. The agent is
never a special client of the compositor; it is an IPC client with a
defined scope.

The intent above is expanded into a concrete blueprint in
[The Agent Phase](agent-phase.md). The framing decisions are recorded in
[ADR-0031](../adr/0031-agent-as-scoped-ipc-client.md) and its follow-ons.

## See Also

- [Architecture](architecture.md) — the component boundaries the principles
  assume.
- [Roadmap](roadmap.md) — the milestone sequence.
- [Comparative Survey](comparative-survey.md) — the systems tessera borrows from
  and rejects.
