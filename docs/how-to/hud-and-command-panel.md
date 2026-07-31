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

Press `Super+S`, or swipe down on the touchpad with four fingers. The panel
opens over a dark blurred scrim.

Close it with `Super+S` again, `Escape`, a click on the scrim, or a
four-finger swipe up.

Bind a different key with a `[[keybind]]` entry whose action is
`command_panel`:

```toml
[[keybind]]
mods = ["super"]
key = "d"
action = "command_panel"
```

## Adjust Quick Settings

Open the panel and select **System** in the left menu. The section carries
the volume slider and mute toggle, the brightness slider, Wi-Fi and
Bluetooth toggles, Do Not Disturb, and the tiled-layout toggle, plus a
display-only **Agent Workspaces** status row aggregating the live agent
sessions. Controls a host cannot serve (no backlight, no audio device)
read as unavailable.

## Activate a Tray Item

Open the panel and select **Tray**. Left-click an item to activate it;
right-click to open its context menu. Items that expose a dbusmenu `Menu`
object path render the menu in place with submenu navigation; all others
fall back to the specification's `SecondaryActivate`.

## Dismiss Notifications

Open the panel and select **Messages**. The newest notifications list
first; click a row to dismiss it. Entries stay in the list for up to an
hour after they were posted — long after their three-second popup has
disappeared. Do Not Disturb (in **System**) suppresses popups while
keeping the list.

Four-finger swipes are compositor-owned and never reach applications. See
[System Shortcuts](../reference/keyboard-shortcuts.md) for the complete
shortcut and gesture reference.
