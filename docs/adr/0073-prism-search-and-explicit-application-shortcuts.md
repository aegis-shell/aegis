# ADR-0073: Prism search and explicit application shortcuts

- Status: Accepted
- Date: 2026-07-29

## Context

[ADR-0022](0022-application-launcher.md) assigns a bare `Super` tap to the
full application launcher. That gesture consumes a high-value modifier by
itself and differs from the explicit key combinations used by the rest of
the global keymap.

The full launcher serves application browsing, but opening a full-screen
grid is heavier than necessary when the application name is already known.
A compact search surface needs the same catalog, icon, running-window, and
start-or-focus behavior without becoming a second discovery or process-launch
path.

[ADR-0021](0021-chrome-component-trait.md) and
[ADR-0044](0044-dock-and-control-center-crates.md) provide a component
boundary for chrome with independent presentation and interaction state.

## Decision

### 1. Use explicit shortcuts for both application surfaces

`Super+A` opens or closes the full Applications launcher. `Super+Space`
opens or closes Prism. Both are ordinary configurable key bindings and remain
available while trusted chrome owns the keyboard.

A bare `Super` tap has no built-in action. `Super+Return` is no longer a
default launcher binding. The generic modifier-tap state machine remains
available in `aegis-core`, but the compositor runtime does not feed it or
assign it a default action.

### 2. Add Prism as an in-process chrome component crate

`aegis-prism` is a standalone crate on the `aegis-shell` `Chrome` contract.
It owns the compact search panel, result-window navigation, pointer hit
testing, input capture, animation, and backdrop request. The `aegis` binary
registers the component as part of the shell composition.

Prism remains in the compositor process. Its keyboard capture, modal
interaction, borrowed GPU icon handles, and shared backdrop pass are
compositor-internal contracts with no external protocol boundary.

### 3. Reuse the catalog and activation path

Prism consumes the same `AppCatalog` and `IconSet` snapshots as the Dock and
full launcher. It uses the pure `aegis-core` launcher state machine for
querying, selection, and running-window matching.

Activating a Prism result emits the existing typed chrome intent: focus a
matching running window, open a compositor-owned built-in, or ask the
composition root to start an external desktop entry. Prism does not enumerate
applications, mutate Wayland state, or spawn processes.

### 4. Keep the two catalog surfaces mutually exclusive

Opening Prism closes the full launcher, and opening the full launcher closes
Prism. Switching surfaces resolves the previous surface immediately so two
modal catalog components cannot capture the same input or overlap during an
exit animation.

## Alternatives

- **Keep the bare `Super` gesture.** Rejected because a modifier-only gesture
  is easier to trigger accidentally and is not represented in the
  configurable keymap.
- **Turn the full launcher into a compact mode.** Rejected because browsing
  and known-item search have distinct layout and interaction lifecycles.
  Keeping two components avoids mode-specific branches in the full-screen
  grid.
- **Implement Prism as an external Wayland client.** Rejected because the
  required trusted input capture, borrowed icon textures, backdrop blur, and
  typed activation intents are already available through the in-process
  chrome contract and have no equivalent external protocol.
- **Give Prism its own application discovery or launch stack.** Rejected
  because duplicate catalog and process paths would drift from launcher and
  Dock behavior.

## Consequences

- Users have one explicit shortcut for browsing applications and one for
  compact search.
- The keymap gains the `prism` action with `toggleprism` and `spotlight`
  aliases.
- `aegis-prism` becomes a workspace member and depends on `aegis-core`,
  `aegis-design`, `aegis-shell`, and lens. `aegis-shell` does not depend on
  the component crate; the binary remains the composition root.
- Launcher and Prism share catalog refreshes, icon ownership, running-window
  focus behavior, reduced-motion policy, and the existing application launch
  drain.
- Historical descriptions of the original bare-`Super` decision in
  ADR-0022 remain intact; this record amends that shortcut decision.
