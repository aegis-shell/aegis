# How to Use the Dock and Launcher

Start applications, switch between their windows, and use app-level window
actions from the compositor chrome.

## Identify a Dock Application

1. Move the pointer over an application icon in the Dock.
2. Keep the pointer on that icon for about 300 milliseconds.
3. Read the application name above the animated icon.

The name follows the icon while the Dock magnification settles. Moving to a
different icon restarts the delay, and leaving the Dock hides the name. The
full-screen launcher keeps application names below their cells instead of
using this hover label.

## Open the Launcher

Use any interaction:

- Click the leading `Applications` tile in the Dock.
- Tap `Super` without pressing another key or pointer button.
- Press the default `Super+Return` key binding.

Type to filter applications. Use the arrow keys to move through the grid,
press `Enter` to activate the selected application, or click an application
cell. Scroll or swipe to move between result pages. Press `Escape` to close
the launcher.

See the [Configuration Reference](../reference/config.md#default-key-bindings)
to change the configurable launcher binding. The bare `Super` tap is a
separate built-in gesture.

## Start or Focus an Application

1. Left-click an application in the Dock or launcher.
2. If the application has a running window, wait for ass to focus and raise
   that window.
3. If the application is not running, wait for ass to start it.

Use the application menu when an application has several windows and you
need to choose one explicitly.

## Pin or Unpin a Dock Application

The Dock splits into two sections: pinned applications on the left, and
applications that are running but not pinned on the right of a divider. A
transient tile disappears again when its last window closes.

1. Right-click a Dock tile.
2. Select `Keep in Dock` to pin a transient application, or `Remove from Dock`
   to unpin a pinned one.

The change is written back to the `[dock] pinned` list in the
[configuration file](../reference/config.md#dock). To return to automatic
selection, delete the list and set `autopopulate = true`.

## Use the Application Menu

1. Right-click an application icon or launcher cell.
2. Select a window title to focus or restore that window.
3. Select `Open` or `New Window` to start another instance.
4. Select `Minimize Window` or `Minimize All Windows` to hide windows while
   keeping their clients alive.
5. Select `Close Window` or `Close All Windows` to request a graceful close.

The menu is anchored to the owning application icon or cell rather than the
pointer position. A Dock menu opens above its icon when space permits and
freezes the current Dock magnification so the menu does not move while you
use it. Menus near an output edge move or change side to remain visible.

Minimized windows remain in the menu. Select a minimized window title to
restore and focus it. An application with several windows lists each window
before the application-wide actions.

Press `Escape` or click outside the menu to dismiss it. When the menu is open
inside the launcher, the first `Escape` dismisses the menu and leaves the
launcher open; press `Escape` again to close the launcher.

## Manage a Borderless Application Window

Use compositor gestures when an application does not provide its own title
bar or resize frame. Follow [How to Manage Borderless
Windows](window-management.md) for the move and resize procedures.
