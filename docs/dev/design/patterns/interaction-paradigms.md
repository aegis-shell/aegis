# Interaction Paradigms

Status: **Partial**.

Interaction paradigms define consistent behavior across pointer, keyboard,
touchpad, and assistive input. They preserve the same domain action even when
the gesture or command differs.

## Drag and drop

- Start a drag only after a movement threshold so click remains reliable.
- Keep the dragged object's source, current target, and allowed operation
  visible. Use copy, move, link, and forbidden cursor semantics consistently.
- Update the target in the same frame as pointer movement and keep automatic
  scrolling bounded.
- Accept drop only on an eligible target. A rejected drop returns the object
  to a stable state and announces the reason when useful.
- Provide a keyboard or command alternative for consequential transfers.
- Treat moving a window into an Interaction Domain as an authority-changing
  operation, not ordinary cosmetic rearrangement.

## Keyboard shortcuts

The canonical user-visible bindings live in the
[Keyboard Shortcuts Reference](../../../reference/keyboard-shortcuts.md).
Components use semantic actions rather than checking raw key symbols when an
action already exists.

- Keep navigation, activation, cancellation, and context-menu access
  available without a pointer.
- Do not shadow global compositor bindings inside transient chrome.
- Show a shortcut beside a menu item only when it is active in that context.
- Respect input methods and do not bind unmodified printable keys in text
  entry contexts.
- Route focus predictably after a shortcut opens, completes, or dismisses a
  surface.

## Context menus

Context menus contain actions for one explicit target or current selection.
They use the shared menu material and metrics, retain the target while open,
and do not move normal keyboard focus merely to render.

- Open from secondary click or the platform context-menu key.
- Anchor near the pointer or focused target and keep the menu within the
  usable output.
- Order common, target-specific actions before destructive actions; separate
  groups visually and semantically.
- Disable an unavailable action only when seeing it explains the model;
  otherwise omit it.
- Close after a command, `Escape`, target destruction, or an outside action.

## Adoption work

- Centralize drag thresholds, auto-scroll policy, and keyboard alternatives.
- Add focus-routing tests for every global shortcut that opens chrome.
- Consolidate context-menu opening and dismissal across Dock, launcher, and
  window surfaces.
