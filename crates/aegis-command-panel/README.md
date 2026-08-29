# aegis-command-panel

Modal command panel for the aegis compositor (ADR-0080).

## Scope

The panel is the display-and-control surface for desktop-computer behavior
(ADR-0115): the domains daily desktop use involves — sound, displays,
network and Bluetooth, power, the session, notifications, the tray, the
user's persona, and desktop preferences. The scope test
is the user's computer, not this compositor: a surface belongs when its
subject exists on any desktop computer regardless of the compositor
implementation. Compositor-mechanism surfaces — window-tree internals,
protocol or IPC state, introspection, developer tooling — stay out and
remain CLI/MCP territory. Domains follow the user's model rather than the
implementing component, so window tiling and Agent Workspaces are in scope
while Interaction Domain lifecycle management is not. Machine-resource
monitoring is deliberately out of scope: an always-on utilization probe is
a standing performance cost that contradicts the panel's event-driven
open/close lifecycle, so the probe thread was retired with its surface.

The HUD is display-only: it reserves no space and accepts no
pointer input. The interactions it used to host live here instead — a
full-screen modal overlay in a personal-info HUD language over an opaque,
scheme-adaptive canvas. Solid elevated surfaces use grouped grays, quiet
separators, and system-blue interaction states, with bold sans-serif display
typography. The panel does not request backdrop blur or liquid glass. It opens
through the `Super+S` keybinding or a four-finger touchpad swipe down (both
dispatched by the compositor's main loop), and closes on Escape, a click on
the background, the same keybinding, or a four-finger swipe up.

Surfaces:

- **Quick Controls tab (first)** — the daily toggles: volume (+ mute),
  brightness, always-on (the session idle inhibitor), and do-not-disturb,
  emitted as `SystemAction`s. The remaining quick settings (Wi-Fi and
  Bluetooth radios, tiled layout, Agent Workspaces status, Lock Now) are
  held out of the panel for now; they return through the settings module
  tabs.
- **Settings module tabs** — one tab per available `aegis-settings` module
  (display, input, appearance, power), rendered from the registry inside
  the main panel; their `SettingsAction`s leave through
  `ChromeEvents::settings_actions` with the current snapshot revision.
- **Notifications** — the notification history as a frameless stream of
  items, newest first, with click-to-dismiss and a tail that fades out
  toward the bottom of the region. The queue retains one hour of entries;
  the frameless toast strip presents only a three-second window of them
  (ADR-0083).
- **Clock (top-center)** — locale wall-clock time at hero scale plus the
  weekday and date, frameless.
- **Tray (left-middle)** — the shared `aegis-tray` StatusNotifierItem
  snapshot as a vertical icon column of compact 22px glyphs: hover raises a
  rounded accent plate *behind* the icon (the glyph itself never dims or
  disappears — only its backing becomes prominent), left-click `Activate`,
  right-click host-rendered dbusmenu popover (or `SecondaryActivate`), and
  vertical scrolling when icons overflow the column.
- **Work Mode segmented control (right-bottom)** — beside the power knob,
  filling the corner band: a frameless segmented control over the session
  power modes (`Balanced`, `Stay Awake`, `Dashboard Lock`) with a
  spring-driven sliding indicator for a light elastic bounce on switches
  and per-segment hover tooltips describing each mode's idle behavior.
- **Power knob & session cluster (right-bottom)** — the corner's right
  half: a circular power knob — one click requests power-off through the
  system-level confirmation chrome; dragging right pulls out a capsule
  whose far button is reboot, dragging left arms suspend — and a separate
  lock button.

## Layout

A boundless, floating HUD layout over the solid grouped background, each
surface with its own reveal animation:

- **Main Control Panel (Center)** — the primary focal point: a solid capsule
  nav rail (Quick Controls plus one capsule per available settings module)
  beside the active tab's elevated surface; tab bodies scroll when they
  overflow.
- **Identity block (top-left)** — frameless user persona: a 56px avatar orb
  (live 3D VRM rendering and VRMA animation playback) inside a gapped cyan
  line ring, the display name at title scale, `@username · groups`, and the
  machine's hostname from the local account/kernel record.
- **Notifications stream (top-right)** — each notification as its own
  recessed card: a distinct background, hairline border, and 10px gaps so
  individuals read clearly; the tail fades out toward the bottom. The
  queue retains one hour of entries; the frameless toast strip presents
  only a three-second window of them (ADR-0083).
- **Clock (top-center)** — the locale wall-clock time at hero scale with
  the weekday and date beneath, drawn frameless between the identity block
  and the notification stream.
- **Tray column (left-middle)** — StatusNotifierItem icons as a vertical
  column: hover raises a rounded-square shadow plate behind the icon,
  left-click activates, right-click opens the dbusmenu popover to the
  icon's right, and the column scrolls when icons overflow (the scrollbar
  fades in on wheel movement and decays out when idle).
- **Work Mode segmented control & power cluster (right-bottom)** — one
  horizontal band pinned into the corner: a frameless segmented control
  over the three session power modes, its active segment marked by a
  sliding pill indicator driven by an under-damped spring so switches land
  with a light elastic bounce (hover a segment for its idle-behavior
  tooltip); beside it, a circular power knob — one click requests
  power-off through the system-level confirmation (the same consent chrome
  portals use); dragging right pulls out a capsule whose far button is
  reboot, dragging left arms suspend — and a separate lock button.

## Boundaries

The component owns presentation and interaction state only. System snapshots
arrive through the `Chrome` trait each frame; intents leave through
`aegis_shell::ChromeEvents` and the shared tray command channel. It has no
D-Bus, Wayland, or process authority of its own. The feature-gated
`aegis_shell::persona` module supplies the same profile and ordered photo/VRM
contract used by the lock screen, including the internal VRM rendering
backend. This panel owns the camera composition, flat graphite fallback disc,
keyline, motion trigger, and reveal treatment around that content.
