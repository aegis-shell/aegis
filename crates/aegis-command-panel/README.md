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
full-screen modal overlay in a VR/AR personal-info HUD language over one
blurred translucent background: the compositor frosts the whole output and
a scheme-adaptive painted wash (ink in dark mode, pearl in light mode)
blends bright and dark into the blur, with cyan-accented liquid-glass
surfaces floating on top and bold sans-serif display typography. The panel
opens through the `Super+S` keybinding or a four-finger touchpad swipe down
(both dispatched by the compositor's main loop), and closes on Escape, a
click on the scrim, the same keybinding, or a four-finger swipe up.

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
- **Notifications** — the notification history with click-to-dismiss,
  pinned to the top-right. The queue retains one hour of entries; the
  frameless toast strip presents only a three-second window of them
  (ADR-0083).
- **Network monitor (right-middle)** — the live interface and Wi-Fi name
  (`wlan0 · Homelab-5G`), two framed line charts plotting upload and
  download throughput from `ChromeUpdate::ResourceStats`, and the current
  rates as text under each chart.
- **Work Mode switcher (right-bottom)** — quick 1-click segmented toggle
  between session power modes (`Balanced`, `Stay Awake`, `Dashboard Lock`),
  with active mode highlighting and live behavioral status hints.
- **Power & Session panel (right-bottom)** — immediate system lifecycle and
  session actions: Lock Now, Suspend, Restart, and Power Off (Shutdown).
- **Tray** — the shared `aegis-tray` StatusNotifierItem snapshot with full
  interaction: left-click `Activate`, right-click host-rendered dbusmenu
  popover (or `SecondaryActivate`).

## Layout

A boundless, floating HUD layout over the shared blurred background, each
surface with its own reveal animation:

- **Main Control Panel (Center)** — the primary focal point: a liquid-glass
  capsule nav rail (Quick Controls plus one capsule per available settings
  module) beside the active tab's frosted glass view; tab bodies scroll
  when they overflow.
- **Identity block (top-left)** — frameless user persona: a 56px avatar orb
  (live 3D VRM rendering and VRMA animation playback) inside a gapped cyan
  line ring, the display name at title scale, `@username · groups`, and the
  machine's hostname from the local account/kernel record.
- **Notifications panel (top-right)** — the notification history with
  click-to-dismiss. The queue retains one hour of entries; the frameless
  toast strip presents only a three-second window of them (ADR-0083).
- **Network monitor (right-middle)** — the network surface below the
  notification stream, sharing the column's right edge.
- **Work Mode & Power Session panels (right-bottom)** — two floating liquid-glass
  panels stacked beneath the network monitor, providing instant glanceability
  and quick control over session idle policies and power lifecycle actions.

## Boundaries

The component owns presentation and interaction state only. System snapshots
arrive through the `Chrome` trait each frame; intents leave through
`aegis_shell::ChromeEvents` and the shared tray command channel. It has no
D-Bus, Wayland, or process authority of its own. The feature-gated
`aegis_shell::persona` module supplies the same profile and ordered photo/VRM
contract used by the lock screen, including the internal VRM rendering
backend. This panel owns the camera composition, flat graphite fallback disc,
keyline, motion trigger, and reveal treatment around that content.
