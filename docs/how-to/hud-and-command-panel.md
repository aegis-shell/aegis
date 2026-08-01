# How to Use the HUD and the Command Panel

Aegis presents system status as a minimal HUD and keeps every related
interaction in one modal panel, the command panel (ADR-0080, ADR-0081,
ADR-0083).

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
**Messages** list (and counts toward the HUD bell) for up to an hour, where
you can dismiss it. Do Not Disturb suppresses popups while the list keeps
accumulating.

## Open the Command Panel

Press `Super+S`, or swipe down on the touchpad with four fingers. The
panel opens centered over a dark blurred scrim as a cluster of three
surfaces: a header band across the top, an icon rail down the left
edge, and a content panel for the selected section.

Close it with `Super+S` again, `Escape`, a click on the scrim, the
circular close button at the bottom of the icon rail, or a
four-finger swipe up.

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

## Switch Sections

Click a section button in the left icon rail — **System**, **Tray**,
or **Messages**. The selected section reads as a solid amber disc.
Long sections scroll inside the content panel.

## Adjust Quick Settings

Open the panel and select **System** in the icon rail. Controls group
under muted headers: Sound, Brightness, Connectivity, Desktop, Agent
Workspaces, and Session. The section carries the volume slider and
mute toggle, the brightness slider, Wi-Fi and Bluetooth toggles, Do
Not Disturb, and the tiled-layout toggle, plus a display-only **Agent
Workspaces** status row aggregating the live agent sessions. Controls
a host cannot serve (no backlight, no audio device) read as
unavailable.

## Activate a Tray Item

Open the panel and select **Tray**. Items lay out in a grid whose
column count adapts to the panel width; the grid scrolls when it
overflows. Left-click an item to activate it; right-click to open its
context menu. Items that expose a dbusmenu `Menu` object path render
the menu in place with submenu navigation; all others fall back to the
specification's `SecondaryActivate`.

## Dismiss Notifications

Open the panel and select **Messages**. Notifications list as cards
with the summary and body, newest first; click a card to dismiss it.
The list scrolls and has no row cap: entries stay for up to an hour
after they were posted — long after their three-second popup has
disappeared. Do Not Disturb (in **System**) suppresses popups while
keeping the list.

Four-finger swipes are compositor-owned and never reach applications. See
[System Shortcuts](../reference/keyboard-shortcuts.md) for the complete
shortcut and gesture reference.
