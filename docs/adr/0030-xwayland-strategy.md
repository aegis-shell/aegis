# ADR-0030: XWayland strategy

- Status: Accepted
- Date: 2026-06-23

## Context

ass is Wayland-native. The [vision](../explanation/vision.md#design-principles)
commits the project to never shipping an X11 session and to reaching X11
applications only through XWayland. The [comparative survey](../explanation/comparative-survey.md#what-ass-rejects)
records the precedent: KDE Plasma 6 keeps an X11 session (`kwin_x11`) on a
deprecation track and is removing it; GNOME, sway, river, and niri are
Wayland-native with XWayland for legacy clients. niri routes XWayland
through `xwayland-satellite` so X11 windows appear as native toplevels.

The window manager and the IPC ([ADR-0027](0027-ipc-and-introspection.md))
are built around a single toplevel model. The question is how X11 windows
enter that model: as a special case the chrome and the agent must
distinguish, or as ordinary toplevels whose origin happens to be X11.

## Decision

ass integrates **XWayland as an optional, lazily-started component**, and
**X11 toplevels enter the same window model as Wayland toplevels**. The
chrome, the focus model, the workspace model
([ADR-0025](0025-workspace-model.md)), the layout role
([ADR-0024](0024-layout-model.md)), the decorations, and the IPC all treat
an X11 window the same as a Wayland window. The only consumer that knows a
window is X11 is the server's bridge to the XWayland connection, which the
IPC exposes as an informational field rather than a behavioral one.

XWayland is **lazily started**: the compositor does not launch it at
session start. The first connection attempt to the X11 socket triggers the
launch, and ass parents XWayland's toplevels as it would any client's. This
avoids the cost and the attack surface of an always-on X server on a
Wayland-only desktop, while keeping X11 applications transparent to the
user.

The XWayland integration is owned by a new `ass-xwayland` crate that depends
on `ass-core` and `ass-server` and translates between the X11 window tree
and the existing surface/toplevel model. It is the only crate with an X11
dependency; the rest of the workspace stays X11-free, so the build and the
binary remain clean for users who do not need X11.

Clipboard, drag-and-drop, and primary selection are bridged through the same
crate so that an X11 application can copy from and paste to a Wayland
application without either side knowing the other exists.

## Alternatives

- **No X11 support at all.** Rejected: it fails the "broad application
  compatibility" target in [Vision and Scope](../explanation/vision.md#scope).
  A meaningful set of applications users rely on (games on launchers, some
  professional tools, some emulators) are X11-only or X11-best.
- **Always-on XWayland at session start.** Rejected: it spends resources and
  widens the attack surface for users who never launch an X11 application.
  Lazy start matches the Wayland-native stance without sacrificing
  compatibility.
- **X11 windows as a separate window class.** Rejected: it forks the window
  model, forcing every consumer (renderer, chrome, IPC, agent) to handle two
  shapes, which is the exact failure mode the one-model principle exists to
  prevent.
- **An X11 session alongside the Wayland session (`kwin_x11`).** Rejected
  outright by the vision: ass will never ship an X11 session.
- **External XWayland through `xwayland-satellite` only.** Rejected as the
  primary path: it works well and is a useful reference (and may be an
  option for users who prefer it), but a first-party integration keeps X11
  windows native to the model without depending on an external project's
  release cadence. The external path remains compatible with the IPC.

## Consequences

- A new `ass-xwayland` crate is the only X11-aware part of the workspace;
  feature-gated behind `xwayland`, a build without it stays X11-free.
- X11 toplevels appear in the window list, the dock, the launcher's
  running-app awareness, the overview, and the IPC exactly as Wayland
  toplevels do, which is the one-model principle verified end to end.
- Lazy start requires careful handling of the first-connection race: the
  compositor must start XWayland, wait for readiness, and only then satisfy
  the connecting client, which is a known but non-trivial integration cost.
- Clipboard and selection bridging is real work and is scoped as part of the
  milestone rather than assumed; drag-and-drop between X11 and Wayland is
  the hardest part and is allowed to land incrementally.
- The agent and external tools see X11 windows as ordinary windows with an
  informational origin field, so automation is origin-agnostic by
  construction.
