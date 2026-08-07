# How to Use the HUD and the Command Panel

Aegis presents system status as a minimal HUD and keeps every related
interaction in one modal panel, the command panel (ADR-0080, ADR-0081,
ADR-0083, ADR-0114).

## Read the HUD Chips

Two frosted chips float over the desktop along the top edge:

- **Left** — network, Bluetooth, and battery status, the StatusNotifierItem
  tray row (excess items collapse into a `+N` indicator), then the clock
  and the notification count.
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
panel opens centered over a dark blurred scrim as a cluster of dark-glass
surfaces: a header band across the top, the main panel at the bottom-left,
and a side column at the right holding the notifications list above the
tray grid. The HUD and Dock hide while the panel owns the screen and
return after it has fully closed.

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

## Read the Header Band

The header band shows your avatar, display name, and account groups on
the left, and a live machine summary on the right: CPU load with a
recent-history sparkline, GPU, memory, network rates, disk, and
battery. Gauge rows appear only for hardware the machine actually has
— a row without real data is simply absent. The gauges refresh every
two seconds.

## Switch Tabs

Click a tab in the main panel's flat tab bar — **System** plus one tab
per available settings module. The active tab reads in the cyan accent.
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

Select a settings module tab — **Display**, **Touchpad**, **Appearance**,
or **Power Management**. Display, appearance, and power edits stage
locally and commit when you select their **Apply** button; touchpad
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
