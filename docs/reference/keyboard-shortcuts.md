# System Shortcuts

Aegis handles global shortcuts in the compositor before delivering input to
the focused client. A matched key press and its release are consumed, so an
application does not also receive the shortcut.

## Default Keyboard Shortcuts

| Shortcut | Effect |
|----------|--------|
| `Super+A` | Open or close the full application launcher |
| `Super+Space` | Open or close Prism application search |
| `Super+O` | Open or close the window and workspace overview |
| `Super+S` | Open or close the command panel (quick settings, settings modules, tray, notifications) |
| `Super+Q` | Close the focused toplevel |
| `Super+Tab` | Focus the next toplevel and show the live switcher while `Super` remains held |
| `Super+Shift+Tab` | Focus the previous toplevel and show the live switcher while `Super` remains held |
| `Super+Right` | Switch to the next workspace |
| `Super+Left` | Switch to the previous workspace |
| `Super+T` | Toggle tiling on the current workspace |
| `Super+L` | Lock the session |
| `Print` | Open the interactive screenshot region selector |
| `Super+Ctrl+Q` | Gracefully quit the current Aegis instance |
| `Super+Shift+Return` | Gracefully quit the current Aegis instance |

The quit shortcuts stop only the Aegis process that receives the input. The
normal shutdown path disables its outputs, releases the seat and Direct
Rendering Manager (DRM) device, and returns control to the terminal. This is
the preferred exit path during VT testing.

The launcher, Prism, and command-panel toggles, `Super+L`, `Print`, and the quit
shortcuts
remain available while trusted Aegis chrome owns the keyboard. No global
shortcut runs while the session is locked or while the focused client has
active keyboard-shortcut inhibition. The development-only
`[dev] allow_quit_while_locked` option lifts the lock-screen exclusion for
the quit shortcuts alone; see the
[Configuration Reference](config.md#development-options).

`Super+L` starts the first-party `ext-session-lock-v1` client. The compositor
then routes input only to its lock surfaces and retains an opaque fail-closed
frame if the client exits unexpectedly. See
[How to Configure Locking and Idle](../how-to/lock-and-idle.md).

## Direct-Display VT Shortcuts

On the DRM backend, `Ctrl+Alt+F1` through `Ctrl+Alt+F12` request a switch to
the corresponding virtual terminal. These combinations are compositor-owned
controls and are not configurable `[[keybind]]` entries. The nested backend
does not perform VT switching.

## Touchpad Gestures

| Gesture | Effect |
|---------|--------|
| Three-finger swipe left | Switch to the next workspace |
| Three-finger swipe right | Switch to the previous workspace |
| Three-finger swipe up | Focus the next toplevel on the current workspace, showing the live switcher until the gesture ends |
| Three-finger swipe down | Focus the previous toplevel on the current workspace, showing the live switcher until the gesture ends |
| Four-finger swipe down | Open the command panel |
| Four-finger swipe up (panel open) | Close the command panel |

Three- and four-finger swipes are compositor-owned (ADR-0080, ADR-0082):
they are claimed by Aegis and never forwarded to client
`zwp_pointer_gestures_v1` objects. A three-finger swipe latches its axis
once it travels 30 px, then fires one step per 120 px of travel. Swipes
with any other finger count forward to clients unchanged.

The table shows the built-in defaults. Each finger count and axis pair can
be rebound or disabled with a `[[gesture]]` entry in `config.toml`; see
the [Configuration Reference](config.md#touchpad-gestures).

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
