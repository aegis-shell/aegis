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

## Minimize, Restore, or Close a Window

Right-click the application's Dock icon or launcher cell and choose the
required window action. Follow [How to Use the Dock and
Launcher](dock-and-launcher.md#use-the-application-menu) for multi-window and
restore behavior.

## Pick a Window in the Overview

Press `Super+O` (or run `ass-control overview`) to open the overview: every
window on the current workspace appears as a live thumbnail, with a
workspace rail on the left. Click a thumbnail to focus that window and
close the overview; click a rail tile to switch workspaces while the
overview stays open. `Escape`, `Super+O` again, or a click on empty space
dismisses it.

## Control a Window from the Command Line

Query the current ids, then issue a control command:

```bash
ass-control windows
ass-control focus 42
ass-control minimize 42
ass-control close 42
```

See the [Command-Line Reference](../reference/cli.md) for the complete command
table.
