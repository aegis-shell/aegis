# aegis-command-panel

Modal command panel for the aegis compositor (ADR-0080).

## Scope

The panel is the display-and-control surface for desktop-computer behavior
(ADR-0115): the domains daily desktop use involves — sound, displays,
network and Bluetooth, power, the session, notifications, the tray, the
user's persona, machine resources, and desktop preferences. The scope test
is the user's computer, not this compositor: a surface belongs when its
subject exists on any desktop computer regardless of the compositor
implementation. Compositor-mechanism surfaces — window-tree internals,
protocol or IPC state, introspection, developer tooling — stay out and
remain CLI/MCP territory. Domains follow the user's model rather than the
implementing component, so window tiling and Agent Workspaces are in scope
while Interaction Domain lifecycle management is not.

The HUD is display-only: it reserves no space and accepts no
pointer input. The interactions it used to host live here instead — a
full-screen modal overlay in a VR/AR personal-info HUD language: deep
blue-black translucent "dark glass" floating panels with a cyan accent,
thin hairlines, and corner brackets over the standard dark blurred
scrim. The panel opens through the `Super+S` keybinding or a four-finger
touchpad swipe down (both dispatched by the compositor's main loop), and
closes on Escape, a click on the scrim, the same keybinding, or a
four-finger swipe up.

Surfaces:

- **System tab** — volume/mute, brightness, Wi-Fi, Bluetooth, Do-Not-Disturb,
  and the tiled-layout toggle, emitted as `SystemAction`s, plus the
  display-only Agent Workspaces status row that aggregates the live Agent
  Interaction Domains (moved here from the HUD, ADR-0083).
- **Settings module tabs** — one tab per available `aegis-settings` module
  (display, touchpad, appearance, power), rendered from the registry inside
  the main panel; their `SettingsAction`s leave through
  `ChromeEvents::settings_actions` with the current snapshot revision.
- **Notifications** — the notification history with click-to-dismiss,
  pinned to the top of the side column. The queue retains one hour of
  entries; the frameless toast strip presents only a three-second window of
  them (ADR-0083).
- **Tray** — the shared `aegis-tray` StatusNotifierItem snapshot at the
  bottom of the side column with full interaction: left-click `Activate`,
  right-click host-rendered dbusmenu popover (or `SecondaryActivate`).

## Layout

A boundless, floating HUD layout of dark glass surfaces, each with its own reveal
animation:

- **Main Control Panel (Center)** — the primary focal point: a flat tab bar (System
  plus one tab per available settings module, close button at the right end) over
  the active tab's body; tab bodies scroll when they overflow.
- **Profile chip (top-left)** — compact user persona (ringed 48px avatar orb with live
  3D VRM rendering and VRMA animation playback, display name, and
  `@username · groups` from the local account record).
- **Notifications panel (top-right)** — the notification history with click-to-dismiss.
  The queue retains one hour of entries; the frameless toast strip presents only a
  three-second window of them (ADR-0083).
- **Machine telemetry & Tray (right-center)** — the machine monitor (chassis glyph plus
  CPU/GPU/RAM/NET/DISK/BAT gauges fed by `ChromeUpdate::ResourceStats`, with
  btop-style sparklines for CPU, GPU, and RAM) over the fixed-height tray panel.

## Boundaries

The component owns presentation and interaction state only. System snapshots
arrive through the `Chrome` trait each frame; intents leave through
`aegis_shell::ChromeEvents` and the shared tray command channel. It has no
D-Bus, Wayland, or process authority of its own. The feature-gated
`aegis_shell::persona` module supplies the same profile and ordered photo/VRM
contract used by the lock screen, including the internal VRM rendering
backend. This panel owns the camera composition, flat graphite fallback disc,
keyline, motion trigger, and reveal treatment around that content.
