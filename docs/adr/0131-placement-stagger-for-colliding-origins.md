# ADR-0131: Session-scoped placement stagger for colliding origins

- Status: Accepted
- Date: 2026-08-19

## Context

Aegis places a newly mapped root toplevel through a priority chain: an
explicit `[[window_rule]]` position, the remembered per-application position
from `window_state.json`, the session's last geometry for that application,
and finally a fallback diagonal cascade (`60 + i*32`). Because
`remember_window_positions` defaults to on, almost every application has a
remembered position, and a second window of the same application — or any
two windows resolved to the same origin — received **exactly** that origin:
`position = rect.origin` with no displacement. The new window stacks
pixel-for-pixel on the live one and fully occludes it. The user cannot see
that a new window opened at all; the only cue is the changed title.

Two earlier decisions frame this one. The M2-era cascade is documented as a
placeholder that "M3's window manager will own" (ADR-0012), but no placement
policy ADR superseded it — M3 shipped with the cascade as the last-resort
branch and the overlap hole above it. And the remembered-geometry store
persists whatever rectangle a window rests in at destroy time or at the end
of an interactive move/resize. A naive fix that simply adds an offset at map
would therefore be *persisted*: the next session's first window would start
at the offset origin, get offset again on the next duplicate map, and be
persisted again — the classic cascade drift seen in traditional stacking
compositors.

This follows the same boundary ADR-0032 draws for identifiers: session-local
facts do not belong in durable stores. The stagger is placement policy at
map time, not geometry the user chose, so the store must never see it.

## Decision

When a newly mapped root toplevel's resolved origin **exactly equals** a
live mapped root toplevel's origin, the compositor shifts it diagonally to
the first free origin along the same diagonal as the fallback cascade:
`+32/+32` logical pixels per step, at most 8 steps, each candidate clamped
into the output with the same 100 px title-bar inset the remembered-position
path uses. If every candidate collides, the window keeps the colliding
origin (a bounded scan must never fail a map). Transients are exempt: a
dialog centered on its parent is the intended presentation, and its
parent-centering pass owns its position. The nudge applies uniformly to all
three origin-resolving branches — rule, remembered, and session geometry —
and to the fallback cascade, since a rule position being stack-occluded by a
second matching window is the same failure.

The stagger is **session-scoped and never persisted**. The surface records
the fold-back pair `(base, nudged)`. Both persistence sites — toplevel
destroy and the end of an interactive move/resize — fold the origin back to
`base` while the window still rests exactly at `nudged`, whether the
rectangle is read from the live position or from a `saved_floating_rect`
captured there. The offset therefore never reaches
`last_app_geometries` or `window_state.json`, and consecutive sessions
reproduce identical placement: the same collision, the same first free
origin, zero drift.

A user- or agent-driven reposition (interactive move, interactive resize,
`set-geometry`) consumes the record, because from that moment the user owns
the position and persistence should remember what they chose. Policy moves
(tiling, maximize/fullscreen, minimize) leave the record in place; the
fold-back's exact-origin guard keeps a stale record inert.

The whole mechanism lives in `aegis-compositor` next to the placement code:
no shared model type, IPC field, or store field changes. The store's version
stays 2.

## Alternatives

- **Persist the staggered origin (naive offset).** Rejected: it is the
  cascade-drift bug. Each session inherits the previous session's offset and
  adds another, so windows march down the screen over days of use.
- **Rect-overlap avoidance (smart placement).** Rejected as the first step:
  remembered placement is per-application, so the realistic collision is an
  exact stack of same-app or same-rule windows, which the origin test
  resolves exactly. A full overlap solver is O(n²) with placement that
  depends on unrelated windows, which ADR-0116 already rejected for the
  overview layout for the same reasons (non-local ripple, no stable
  targets). It can be added later behind the same fold-back mechanism if
  real usage asks for it.
- **Fix the collision by re-centering the duplicate window.** Rejected:
  centering is the transient presentation, so a centered duplicate main
  window reads as a dialog; and repeated maps would oscillate between two
  presentations instead of settling into a stable stack.
- **Offset only the remembered-position branch.** Rejected: the rule
  position and the session-geometry branch have the identical failure mode,
  and branch-specific behavior makes the policy harder to reason about than
  one uniform rule.

## Consequences

- Opening a duplicate window no longer hides the window it duplicates; the
  diagonal stack matches what the fallback cascade already taught users.
- Remembered geometry stays meaningful: what persists is always either the
  user's chosen position or the position the policy resolved before any
  session-local stagger.
- Two windows of the same application still overlap heavily after the
  stagger (by construction — only the origin moves 32+32 per step); the
  mechanism guarantees visibility of both title areas, not a non-overlapping
  layout.
- The nudge is per-window state, so closing the older window leaves the
  younger window's nudge armed; its own destroy folds back to its base. No
  cross-window bookkeeping exists to get wrong.
- Collision is tested against live windows on every workspace, so a window
  launch-placed onto a second workspace (ADR-0118) still staggers if the
  application's other window occupies the remembered origin, even though the
  two are never visible together. Accepted for simplicity: the fold-back
  keeps it invisible to the store, and the cost is one 32-pixel offset on a
  workspace the user is not looking at.
- `SurfaceRec` grows one session-scoped field; the store schema, the IPC
  surface, and `aegis-model` are untouched.
- Follow-up: a window rule field to pin a placement that opts out of the
  stagger (for kiosk-style pinned positions) would compose cleanly with the
  fold-back, since a pinned rule position is `base` by definition.
