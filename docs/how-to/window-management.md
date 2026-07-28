# How to Manage Borderless Windows

Move and resize windows without adding a compositor title bar. These gestures
work with undecorated clients such as `foot` and with applications that draw
their own decorations.

## Move a Window

1. Move the pointer anywhere inside the window content.
2. Hold `Super` before pressing a pointer button.
3. Press and hold the left pointer button.
4. Drag the window to its new position, then release the button.

Moving a tiled, maximized, or fullscreen window changes it to a floating
window before the drag begins. The compositor owns the gesture, so the client
does not need to implement a title-bar drag region.

## Resize from Anywhere

1. Move the pointer inside the window near the edge or corner to resize.
2. Hold `Super`.
3. Press and hold the right pointer button.
4. Drag until the window reaches the required size, then release the button.

The starting pointer position selects the nearest edge or corner. Start near
a corner to resize two axes together. Start near the middle of an edge to
resize one axis.

Tiled, maximized, and fullscreen windows change to floating windows before a
`Super` + right-drag resize begins.

## Resize from an Invisible Border

1. Use a floating window that is neither maximized nor fullscreen.
2. Move the pointer within eight pixels of the inside edge or corner.
3. Wait for the resize cursor to identify the active edge or corner.
4. Left-drag the border, then release the button at the required size.

Use `Super` + right-drag instead when a window is tiled, maximized, or
fullscreen. Layout-owned windows do not expose the invisible border.

## Move or Resize foot

1. Start `foot` normally. No foot-specific title-bar or resize configuration
   is required.
2. Use `Super` + left-drag anywhere in the terminal content to move it.
3. Use `Super` + right-drag anywhere in the terminal content to resize it.
4. After it is floating, use its eight-pixel inside border for direct resize
   when convenient.

The same procedure applies to any Wayland `xdg_toplevel`, regardless of
whether the client provides decorations.

## Show Client-Side Title Bars

Set the decoration policy when an application-drawn title bar is preferred:

```toml
[ui]
window_decorations = "client-side"
```

Save the configuration. Existing decoration-aware Wayland windows reconfigure
without restarting. Restore the default borderless policy with
`window_decorations = "borderless"`.

## Minimize, Restore, or Close a Window

Right-click the application's Dock icon or launcher cell and choose the
required window action. Follow [How to Use the Dock and
Launcher](dock-and-launcher.md#use-the-application-menu) for multi-window and
restore behavior.

## Pick a Window in the Overview

Press `Super+O` (or run `aegis-ctl overview`) to open the overview: every
window on the current workspace appears as a live thumbnail, with a
workspace rail on the left. Click a thumbnail to focus that window and
close the overview; click a rail tile to switch workspaces while the
overview stays open. `Escape`, `Super+O` again, or a click on empty space
dismisses it.

## Switch Windows with Live Previews

1. Hold `Super`.
2. Press `Tab` to select the next window, or `Shift+Tab` to select the
   previous window.
3. Keep holding `Super` while pressing `Tab` again to move through the live
   preview strip.
4. Release `Super` on the required window.

The selected window receives focus on each step. The preview strip closes
when `Super` is released.

## Control a Window from the Command Line

Query the current ids, then issue a control command:

```bash
aegis-ctl windows
aegis-ctl focus 42
aegis-ctl minimize 42
aegis-ctl close 42
```

See the [Command-Line Reference](../reference/cli.md) for the complete command
table.
