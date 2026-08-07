# ADR-0114: Panel-hosted settings and the HUD command panel

- Status: Accepted
- Date: 2026-08-07

## Context

[ADR-0049](0049-standalone-modular-control-center.md) moved persistent
settings out of the compositor process into a standalone application,
arguing for crash containment, launcher identity, and deep links.
[ADR-0056](0056-system-settings-identity-and-boundary.md) gave that
application the System Settings identity and forbade compositor chrome from
hosting persistent settings modules.
[ADR-0060](0060-statusbar-system-controls-and-live-system-ipc.md) then split
the surfaces: compositor chrome owns immediate live-system controls, while
the standalone application remains the canonical persistent-settings editor.
[ADR-0080](0080-hud-status-chips-and-sao-command-panel.md) concentrated the
chrome interactions in one modal command panel with an icon rail and
System/Tray/Messages sections, rendered in the frosted-white SAO palette.

The standalone application never delivered what the process boundary was
meant to protect. No third-party process-protocol module extension point
materialized, so every shipped module is reviewed first-party code. Its
module host duplicated the panel's presentation work — navigation,
snapshot subscription, apply lifecycle — and its IPC worker duplicated
plumbing the compositor already owns. In exchange the product carried two
settings surfaces, a separate launcher identity, and an extra window for a
surface users treat as part of the shell.

The `SettingsModule` contract kept modules process-agnostic throughout: a
module owns presentation and draft state, receives the authoritative
snapshot, and returns typed intents. Re-hosting the same modules inside the
command panel is therefore mechanical and changes no commit semantics.

The panel's layout and palette also drifted from the product direction.
The icon rail and section switching hid the notification list and the tray
behind navigation even though the panel is modal and has room to show
both, and the frosted-white SAO island read as a foreign light element
inside the dark VR/AR personal-info-HUD language the shell is adopting.

## Decision

1. **Settings modules host in the command panel.** The standalone
   `aegis-settings` Wayland application is removed: the binary, the
   `io.github.ming2k.aegis.Settings` desktop entry and icon, the launcher
   identity, and the settings IPC worker. The `aegis-settings` crate
   survives as a lib-only settings module library: the `SettingsModule`
   contract, the `ModuleRegistry`, and the built-in modules (display,
   touchpad, appearance, and power available; mouse, keyboard, users, and
   window-rules registered with stable metadata but unavailable until
   their backends exist).

2. **The panel presents settings as flat tabs.** The main panel's tab bar
   holds the pre-existing System quick-controls tab plus one tab per
   available settings module, with the close button at its right end. The
   icon rail and the `Section { System, Tray, Messages }` switching are
   gone. Below the full-width header band, the main panel sits at the
   bottom-left and an always-visible side column sits at the right: the
   flat notification list (all retained notifications tiled,
   click-to-dismiss) over the always-visible tray icon grid (left-click
   activation, right-click dbusmenu popover anchored to the live cell
   rect, as before).

3. **Module intents travel a new in-process chrome channel.** Modules
   render in-process through the same `SettingsModule` contract and emit
   `SettingsAction`s through
   `ChromeEvents.settings_actions: Vec<(Option<u64>, SettingsAction)>` in
   `aegis-shell`. The compositor main loop drains the channel into the
   same `commit_settings` path the IPC settings API uses — the same
   revision check, session-lock guard, and mutation journaling — and
   pushes the authoritative snapshot chrome-ward as
   `ChromeUpdate::Settings(&SettingsSnapshot)` on startup and after every
   commit. The revisioned IPC settings API remains available to external
   clients unchanged.

4. **The panel adopts the HUD visual language.** The frosted-white SAO
   palette is replaced for the command panel by a dark-glass, cyan-accent
   VR/AR palette: the `Hud` tokens in `aegis-design`, with `themes::hud`
   and `materials::hud_panel` factories and corner-bracket accents. The
   `Sao` tokens remain in `aegis-design` for any other consumers.

This record supersedes the standalone-application boundary decided in
ADR-0049 and ADR-0056 (both already superseded as records; their lineage
carried the boundary forward) and partially supersedes ADR-0060: its
live-system IPC protocol and its immediate-versus-persistent authority
split remain in effect, but its "standalone System Settings is the
canonical persistent-settings editor" boundary does not. It amends
ADR-0080's panel layout and palette and ADR-0083's references to the
Messages section. Those records keep their original text.

## Alternatives

- **Keep the standalone System Settings application alongside the panel
  tabs.** Rejected because two settings surfaces duplicate the module
  host, the navigation, and the apply lifecycle, and every future module
  would have to justify which surface it belongs to.
- **Run one helper process per module and proxy the snapshots.** Rejected
  because the shipped modules are all first-party and hold no I/O
  authority; a per-module process protocol would add an IPC contract,
  lifecycle management, and failure modes to protect against faults the
  module contract already confines to presentation logic.
- **Route module intents over the compositor's own IPC socket.** Rejected
  for the reason ADR-0060 rejected status-bar loopback: both endpoints
  live in one process, and serializing a loopback request adds failure
  modes without strengthening the authority boundary. The drained chrome
  channel enters the identical commit path, so validation and journaling
  stay shared.
- **Keep the SAO palette for the panel.** Rejected because the frosted
  white island contradicts the dark HUD language the rest of the shell
  presents; the panel is the most visible chrome surface and sets the
  product's visual tone.

## Consequences

- The crash-containment argument of ADR-0049 is deliberately traded away:
  a fault in a settings module's rendering or draft handling can now take
  down the compositor. The team accepts this because every module is
  statically registered, reviewed first-party code whose fault surface is
  presentation logic over an authoritative snapshot — the same risk class
  the command panel already carries for quick settings, tray, and
  notifications — and because the process boundary never produced the
  third-party extension point that would have given isolation real
  content.
- Settings lose their launcher identity, desktop entry, window grouping,
  and `--module` deep links. The panel opens with `Super+S` or a
  four-finger swipe, and external clients keep the revisioned settings
  IPC as the UI-independent edit path.
- One host owns the settings apply lifecycle: revision checks, the
  session-lock guard, persistence, and journal coverage are identical for
  panel edits and IPC edits by construction.
- Packaging no longer installs `bin/aegis-settings`, the
  `io.github.ming2k.aegis.Settings.desktop` entry, or its icon;
  development staging and tutorial workflows no longer stage them.
- Unavailable modules (mouse, keyboard, users, window-rules) keep their
  registered routes and metadata but render no tab until their
  authoritative backend exists; an available module's tab renders the
  module's page in the main panel.
- The compositor now depends on the `aegis-settings` library crate; the
  earlier rule forbidding a compositor-to-settings dependency is
  inverted. Modules still must not read `config.toml`, probe hardware, or
  call system services directly.
