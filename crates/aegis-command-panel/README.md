# aegis-command-panel

Modal command panel for the aegis compositor (ADR-0080).

The HUD is display-only: it reserves no space and accepts no
pointer input. The interactions it used to host live here instead — a
full-screen modal overlay in the Sword Art Online menu language: frosted
white floating panels with an amber accent over the standard dark blurred
scrim. The panel opens through the `Super+S` keybinding or a four-finger
touchpad swipe down (both dispatched by the compositor's main loop), and
closes on Escape, a click on the scrim, the same keybinding, or a
four-finger swipe up.

Sections:

- **System** — volume/mute, brightness, Wi-Fi, Bluetooth, Do-Not-Disturb,
  and the tiled-layout toggle, emitted as `SystemAction`s, plus the
  display-only Agent Workspaces status row that aggregates the live Agent
  Interaction Domains (moved here from the HUD, ADR-0083).
- **Tray** — the shared `aegis-tray` StatusNotifierItem snapshot with full
  interaction: left-click `Activate`, right-click host-rendered dbusmenu
  popover (or `SecondaryActivate`).
- **Messages** — the notification history with click-to-dismiss. The queue
  retains one hour of entries; the frameless toast strip presents only a
  three-second window of them (ADR-0083).

## Layout

One centered cluster of three frosted-white surfaces, each with its own
reveal animation:

- **Header band** — the user's identity (ringed avatar, display name,
  `@username · groups` from the local account record) beside a live machine
  monitor: a chassis glyph (laptop/desktop) and CPU/GPU/RAM/NET/DISK/BAT
  gauges fed by `Chrome::update_resource_stats`, with btop-style sparklines
  for CPU, GPU, and RAM.
- **Icon rail** — one circular button per section (ring + amber glyph,
  solid disc when selected) and the close button at the bottom.
- **Content panel** — the section title and the active section's body; all
  three bodies scroll when they overflow.

## Boundaries

The component owns presentation and interaction state only. System snapshots
arrive through the `Chrome` trait each frame; intents leave through
`aegis_shell::ChromeEvents` and the shared tray command channel. It has no
D-Bus, Wayland, or process authority of its own. `aegis-identity` supplies the
same account and ordered photo/VRM contract used by the lock screen;
`aegis-avatar` renders only an explicitly selected VRM with the panel's camera.
This panel owns the flat graphite fallback disc, keyline, and reveal treatment
around that content.
