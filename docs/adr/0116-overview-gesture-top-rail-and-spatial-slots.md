# ADR-0116: Overview gesture, top workspace rail, and spatial slot assignment

- Status: Accepted
- Date: 2026-08-09

## Context

The M9 overview is the modal window/workspace picker: the compositor draws
live thumbnails on a shared grid and the chrome draws frames, labels, and a
workspace rail. Three forces pressed on it:

- Its only built-in entry point was `Super+O`. Users coming from macOS
  reach for the touchpad — four fingers up is the Mission Control muscle
  memory — but the four-finger vertical swipe was the command panel
  ([ADR-0080](0080-hud-status-chips-and-sao-command-panel.md),
  [ADR-0082](0082-configurable-touchpad-swipe-bindings.md)).
- The workspace rail ran down the left edge as numeric tiles. Numbers carry
  no content: the user cannot tell workspaces apart without switching.
  GNOME's Activities top strip and the macOS Spaces bar both show live
  miniatures instead.
- Thumbnails popped onto the grid instantly, filled in z-order, so the
  picker had no spatial relationship to the real desktop. The "cluttered
  yet ordered" feel of Mission Control comes from windows landing near
  their on-screen positions (the classic Exposé fan-out).

## Decision

1. **A new `GestureAction::Overview` opens the picker on a four-finger
   upward swipe and closes it on a downward swipe**, firing at most once
   per gesture. The four-finger vertical default rebinds from
   `CommandPanel` to `Overview`; the command panel keeps `Super+S` and
   remains bindable through `[[gesture]]` configuration. Direction
   semantics stay inside the action, per ADR-0082.
2. **The workspace rail moves from the left edge to the top edge**, and
   each tile renders its workspace's live miniature — every mapped window
   of that workspace, drawn by the compositor into the shared tile
   geometry — instead of a bare number. The chrome draws only a
   translucent tint, the frame, and the caption over the compositor's
   pixels. Tiles appear whenever the output owns more than one workspace,
   unchanged from before.
3. **Thumbnails fly in from their real window geometry.** The chrome's
   reveal progress feeds the thumbnail pass; each cell interpolates from
   the window's actual rect into its grid slot, and the scrim and rail
   tiles fade in with the same factor. Reduced motion snaps instantly, as
   before.
4. **Slot assignment is closest-slot.** Each window claims the free grid
   slot nearest its on-screen center (globally closest pair first,
   deterministic), replacing z-order-sequential fill. The shared-geometry
   contract between the thumbnail pass and the chrome's hit-testing now
   covers assignment: both call `assign_slots` with the same window list
   in the same order.

This record amends
[ADR-0080](0080-hud-status-chips-and-sao-command-panel.md) and
[ADR-0082](0082-configurable-touchpad-swipe-bindings.md), whose default
gesture table assigns the four-finger vertical swipe to the command panel.

## Alternatives

- **Per-direction bindings on one axis** (up → overview, down → command
  panel). ADR-0082 rejected splitting a swipe into per-direction bindings:
  stateful behavior (fire-once latches, held-open sessions) is a property
  of the gesture, not of a direction. A composite action would conflate
  two features with different feature gates.
- **Keep the rail on the left.** The top strip matches the Spaces-bar
  mental model users already know, leaves the grid full height, and suits
  wide outputs better than a vertical column of wide tiles.
- **Full relaxation layout** (KWin "Natural"-style iterative fan-out, the
  closest match to classic Exposé). Rejected: O(n²) overlap passes with
  unbounded iterations, non-local ripple when the window set changes, and
  no stable targets to animate between. Closest-slot keeps most of the
  spatial benefit as a pure, deterministic function.
- **Ordering-based row packing** (GNOME Shell / Plasma 6.1 ExpoLayout).
  Not chosen now: it changes the equal-slot grid contract both passes
  share, for a bigger visual-character change than this milestone needs.
- **Animate the close direction too.** Deferred: the close path currently
  swaps the scene back instantly while the chrome fades, which reads well
  enough; a fly-back can reuse the same interpolation later.

## Consequences

- The command panel loses its default touchpad gesture. This is a
  user-visible behavior change: `docs/reference/config.md` and
  `docs/reference/keyboard-shortcuts.md` document the new default and how
  to restore the panel binding.
- The compositor gains explicit-set preview frame enumeration
  (`client_preview_frames_for` and relatives) so the rail can draw
  workspaces other than the visible one; like other previews, tiles bypass
  physical-output occlusion.
- `aegis_model::overview` grows the top-rail geometry, tile content
  insets, parameterized mini-grids, and `assign_slots`; the thumbnail pass
  and chrome stay in lockstep through that one module.
- The fly-in makes the first frames of the overview a re-layout of the
  live desktop; damage already forces full composites while the reveal
  animation runs.
