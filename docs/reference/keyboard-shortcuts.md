# System Shortcuts

Aegis handles global shortcuts in the compositor before delivering input to
the focused client. A matched key press and its release are consumed, so an
application does not also receive the shortcut.

## Default Keyboard Shortcuts

| Shortcut | Effect |
|----------|--------|
| `Super` tap | Open or close the application launcher |
| `Super+Return` | Open or close the application launcher |
| `Super+O` | Open or close the window and workspace overview |
| `Super+Q` | Close the focused toplevel |
| `Super+Tab` | Focus the next toplevel and show the live switcher while `Super` remains held |
| `Super+Shift+Tab` | Focus the previous toplevel and show the live switcher while `Super` remains held |
| `Super+Right` | Switch to the next workspace |
| `Super+Left` | Switch to the previous workspace |
| `Super+T` | Toggle tiling on the current workspace |
| `Print` | Open the interactive screenshot region selector |
| `Super+Shift+Q` | Gracefully quit the current Aegis instance |
| `Super+Shift+Return` | Gracefully quit the current Aegis instance |

The quit shortcuts stop only the Aegis process that receives the input. The
normal shutdown path disables its outputs, releases the seat and Direct
Rendering Manager (DRM) device, and returns control to the terminal. This is
the preferred exit path during VT testing.

`Super+Shift+Q` remains available while trusted Aegis chrome owns the
keyboard. No global shortcut runs while the session is locked or while the
focused client has active keyboard-shortcut inhibition.

## Direct-Display VT Shortcuts

On the DRM backend, `Ctrl+Alt+F1` through `Ctrl+Alt+F12` request a switch to
the corresponding virtual terminal. These combinations are compositor-owned
controls and are not configurable `[[keybind]]` entries. The nested backend
does not perform VT switching.

## Pointer Shortcuts

| Shortcut | Effect |
|----------|--------|
| `Super` + left-drag | Move the targeted floating window |
| `Super` + right-drag | Resize the targeted floating window from the nearest edge or corner |

These pointer shortcuts are not configurable `[[keybind]]` entries.

## Configuration

Custom keyboard bindings use `[[keybind]]` entries in `config.toml` and layer
over the built-in defaults. A custom entry with the same combination takes
precedence over its built-in action. Modifier matching is exact. ASCII letter
keysyms are matched without regard to case, so a binding whose key is `q`
also matches the uppercase keysym produced by `Shift+Q`.

See the [Configuration Reference](config.md#key-bindings) for modifier, key,
and action names.
