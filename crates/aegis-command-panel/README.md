# aegis-command-panel

Modal command panel for the aegis compositor (ADR-0080).

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

One centered cluster of dark glass surfaces, each with its own reveal
animation:

- **Header band** — the user's persona (ringed avatar, display name,
  `@username · groups` from the local account record) beside a live machine
  monitor: a chassis glyph (laptop/desktop) and CPU/GPU/RAM/NET/DISK/BAT
  gauges fed by `ChromeUpdate::ResourceStats`, with btop-style sparklines
  for CPU, GPU, and RAM.
- **Main panel** — a flat tab bar (System plus one tab per available
  settings module, close button at the right end) over the active tab's
  body; tab bodies scroll when they overflow.
- **Side column** — the notifications panel (flexible height) over the
  fixed-height tray panel, both always visible regardless of the active
  tab.

## Boundaries

The component owns presentation and interaction state only. System snapshots
arrive through the `Chrome` trait each frame; intents leave through
`aegis_shell::ChromeEvents` and the shared tray command channel. It has no
D-Bus, Wayland, or process authority of its own. The feature-gated
`aegis_shell::persona` module supplies the same profile and ordered photo/VRM
contract used by the lock screen, including the internal VRM rendering
backend. This panel owns the camera composition, flat graphite fallback disc,
keyline, motion trigger, and reveal treatment around that content.
