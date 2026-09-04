# How to Use the HUD and the Command Panel

Tessera presents system status as a minimal HUD and keeps every related
interaction in one modal panel, the command panel (ADR-0080, ADR-0081,
ADR-0083, ADR-0114). The panel is the display-and-control surface for
desktop-computer behavior (ADR-0115): the state and controls of the
daily-use domains — sound, displays, network and Bluetooth, power, the
session, notifications, the tray, your account persona, machine resources,
and desktop preferences. It is not a console for the compositor itself.

## Read the HUD Chips

Two frosted chips float over the desktop along the top edge:

- **Left** — network status (the associated Wi-Fi network's name when the
  live link is wireless), Bluetooth with its on/off word, the speaker's
  level (or "Muted"), and battery, the StatusNotifierItem tray row (excess
  items collapse into a `+N` indicator), then the clock and the
  notification count.
- **Center** — one dot per workspace; the active workspace's dot is larger
  and brighter.

The chips reserve no space: windows tile and maximize underneath them, and
clicks fall straight through to the windows below. Moving the cursor near a
chip fades it out; moving away fades it back in. A fullscreen window hides
the HUD entirely.

Set `enabled = false` under `[hud]` in `config.toml` to turn the HUD off at
the next compositor launch.

## Read Notification Popups

New notifications appear as a frameless strip at the top-right: plain
floating text, newest on top, no background panel. A popup is display-only
— it never captures clicks — and disappears on its own after three seconds.
The notification itself is not gone: it stays in the command panel's
notifications list (and counts toward the HUD bell) for up to an hour,
where you can dismiss it. Do Not Disturb suppresses popups while the list
keeps accumulating.

## Open the Command Panel

Press `Super+S`, or swipe down on the touchpad with four fingers. The
panel opens as a boundless floating HUD layout over an opaque background.
The background and elevated surfaces follow the desktop's light or dark
appearance, while system blue marks active controls. The main control panel
sits in the center, with a compact profile chip at the top-left,
notifications at the top-right, and the machine telemetry and tray column at
the right. The HUD and Dock hide while the panel owns the screen and return
after it has fully closed.

Close it with `Super+S` again, `Escape`, a click on the scrim, the close
button at the right end of the tab bar, or a four-finger swipe up.

Bind a different key with a `[[keybind]]` entry whose action is
`command_panel`:

```toml
[[keybind]]
mods = ["super"]
key = "d"
action = "command_panel"
```

## Read the Profile and Machine Monitors

The **profile chip (top-left)** shows your avatar (with live 3D VRM rendering
and VRMA animation playback when configured), display name, and account groups.

The **main control panel (center)** holds System quick controls (Sound, Brightness,
Connectivity, Desktop) and modular settings tabs.

The **machine monitor (right-center)** shows a live hardware summary: chassis
pictogram, CPU load with a recent-history sparkline, GPU, memory, network
rates, disk, and battery, pinned above the StatusNotifierItem tray.

The **notifications panel (top-right)** presents retained notification history
with click-to-dismiss.

## Switch Tabs

Click a tab in the main panel's flat tab bar — **System** plus one tab
per available settings module. The active tab uses the system-blue accent.
Long tab bodies scroll inside the main panel.

## Adjust Quick Settings

Open the panel and select the **System** tab. Controls group under muted
headers: Sound, Brightness, Connectivity, Desktop, Agent Workspaces, and
Session. The tab carries the volume slider and mute toggle, the
brightness slider, Wi-Fi and Bluetooth toggles, Do Not Disturb, and the
tiled-layout toggle, plus a display-only **Agent Workspaces** status row
aggregating the live agent sessions. Controls a host cannot serve (no
backlight, no audio device) read as unavailable.

## Edit Persistent Settings

Select a settings module tab — **Display**, **Input**, **Appearance**,
**Dock**, or **Power Management**. Display, appearance, and power edits stage
locally and commit when you select their **Apply** button; input and dock
edits apply immediately. Committed changes persist to `config.toml`
through the compositor's revisioned settings transaction; see the
[Settings Reference](../reference/settings.md) for module routes, apply
policies, and backend availability.

## Activate a Tray Item

The tray grid is always visible at the bottom of the side column. Items
lay out in a grid whose column count adapts to the panel width.
Left-click an item to activate it; right-click to open its context menu.
Items that expose a dbusmenu `Menu` object path render the menu in place
with submenu navigation; all others fall back to the specification's
`SecondaryActivate`.

## Dismiss Notifications

The notifications list fills the rest of the side column and needs no
tab or section switch. Notifications list as cards with the summary and
body, newest first; click a card to dismiss it. The list scrolls and has
no row cap: entries stay for up to an hour after they were posted — long
after their three-second popup has disappeared. Do Not Disturb (on the
**System** tab) suppresses popups while keeping the list.

Four-finger swipes are compositor-owned and never reach applications. See
[System Shortcuts](../reference/keyboard-shortcuts.md) for the complete
shortcut and gesture reference.
