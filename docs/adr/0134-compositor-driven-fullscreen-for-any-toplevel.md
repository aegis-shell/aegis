# ADR-0134: Compositor-driven fullscreen for any toplevel

- Status: Accepted
- Date: 2026-08-22

## Context

`xdg_toplevel.set_fullscreen` is a client request: only the application can
ask for fullscreen. Applications that never issue it — a game that ships a
windowed mode only, a legacy tool pinned to its 800×600 window — can never
reach Aegis's fullscreen presentation: the whole-output configure, the
chrome stand-down, the paused wallpaper, and the direct-scanout eligibility
of [ADR-0101](0101-dual-presentation-paths-and-conservative-kms-plane-allocation.md) are all
keyed on the same state bit. The user's exit is to resize a window to cover
the output, which still reserves the chrome work area, still animates the
wallpaper behind it, and still lets the dock's hot edge and the HUD draw
over the game.

Aegis already has the inverse asymmetry solved: compositor-side maximize
(`SetMaximized`) flips the same state bit the client's
`set_maximized` request sets. Fullscreen had no compositor-side entry.

The design question is whether a compositor-initiated fullscreen violates
xdg-shell's intent. It does not: the protocol deliberately splits state
ownership into *requests* (client asks) and *configure* (compositor
answers, and may answer unprompted). Sending a client a configure carrying
`XDG_TOPLEVEL_STATE_FULLSCREEN` with the output's size is the mechanism the
protocol provides for exactly this; the client adapts to the state it is
told, same as it adapts to `activated` or `maximized` configures the
compositor initiates on focus changes and dock-menu maximize. Sway's
`fullscreen toggle`, KWin's fullscreen shortcut, and GNOME Shell's
window-menu fullscreen all take this path.

## Decision

1. **Compositor-driven fullscreen reuses the xdg-shell fullscreen state,
   not a new internal flag.** `Server::set_toplevel_fullscreen` flips
   `WindowState::fullscreen` and drives the shared
   `reconfigure_with_state` geometry path: the whole output (not the
   chrome-inset work area), the floating rect saved exactly once on entry
   and restored on exit. The client observes
   `XDG_TOPLEVEL_STATE_FULLSCREEN` in a normal configure. Because
   `SpaceUse` ([ADR-0064](0064-output-space-use-and-chrome-policy.md)) derives
   from the same bit, the dock lockout, HUD dormancy, wallpaper pause, and
   presentation-side suppression apply with no new chrome-side code.

2. **`Super+F11` is the default binding; bare `F11` stays unbound.** The
   action is `fullscreen` in `[[keybind]]`. Leaving bare `F11` to the
   client preserves the near-universal in-app fullscreen shortcut; the
   compositor binding is the escape hatch for apps that do not have one.
   The toggle targets the focused toplevel, which is what a game in play
   always is. The action is not in the keyboard-capture allowlist: modal
   chrome owns the keyboard, and un-fullscreening an obscured window is
   not a compositor-level control.

3. **One new IPC command, protocol 30.** `SetFullscreen { id, fullscreen }`
   under the `control` capability and the `SetWindowGeometry` operation
   class, window-scoped like its siblings, with a `TransactOp` mirror so
   batches can include it. `aegis window fullscreen <id> <on|off>` wraps
   it. The keybinding dispatches through the same journaled command path
   as IPC and chrome ([ADR-0033](0033-mutation-journal.md)).

4. **Fullscreen stays session state.** Nothing is persisted to the window
   state store or configuration; `saved_floating_rect` restores geometry on
   exit, and a window that both maximized and un-fullscreened lands in the
   maximized rect, matching what a client observes when it unsets
   fullscreen after maximizing.

## Alternatives

- **A separate "presentation mode" flag** that covers the output but keeps
  the xdg state clean. Rejected: two states meaning nearly the same thing
  would need every chrome, scanout, and damage consumer to check both, and
  clients would never learn why the output-sized configure arrived.

- **Only an IPC/CLI entry, no default keybind** (the always-on-top shape of
  [ADR-0084](0084-session-scoped-always-on-top-window-band.md)). Rejected
  here because the primary consumer is a game in play: focus is already
  captured by the game, and reaching a terminal to fullscreen it is the
  exact friction this removes. The dock app-menu route stays out: its
  lifecycle rows deliberately hide for fullscreen windows, and a game has
  no reason to open the dock mid-session.

- **Layer-shell-like "cover the chrome" for windows.** Not applicable:
  Aegis ships no wlr-layer-shell ([ADR-0080](0080-hud-status-chips-and-sao-command-panel.md)),
  and the chrome already derives its stand-down from window state.

## Consequences

Any window can now be played free of dock, HUD, and wallpaper influence,
which is the point. Applications receive a fullscreen configure they did
not request; well-behaved xdg-shell clients handle that the same way they
handle unsolicited maximize, but a client that assumes fullscreen is only
ever self-inflicted may render at the wrong size until its next commit —
the same risk compositor-side maximize already carries.

`set_toplevel_maximized` refuses while fullscreen (fullscreen wins the
`SpaceUse` precedence); the inverse is deliberately asymmetric —
un-fullscreening a maximized window restores its maximized rect rather
than refusing — so a user cannot strand a window in a state it cannot
leave through the same binding.
