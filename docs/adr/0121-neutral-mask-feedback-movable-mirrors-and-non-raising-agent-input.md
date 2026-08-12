# ADR-0121: Neutral mask feedback, movable mirrors, and non-raising Agent input (amends [0048](0048-compositor-owned-agent-operation-feedback.md), [0091](0091-agent-controlled-window-physical-mirrors-and-guard.md))

- Status: Accepted
- Date: 2026-08-11

## Context

ADR-0048 renders applied Agent operations as a colored circular crosshair
with a movement trail and click pulse, and ADR-0091 gives each read-only
mirror a guard whose border uses the same per-Interaction-Domain identity
hues. In practice the blue/purple accent colors read as arbitrary chrome
rather than as information, and the crosshair competes with the window
content the user is trying to watch. The feedback also lives at the pointer
only; nothing marks the operated window as a whole.

Two interaction properties compound the problem. Every applied Agent input
batch passed through `change_keyboard_focus`, whose unconditional raise moved
the target window above the physical user's windows on each batch — an
Agent-operated window could not stay covered and appeared to keep grabbing
the desktop. And a read-only mirror could not be moved at all: the server
rejected every human move path, so a mirror blocking the user's view had to
be tolerated until control returned.

## Decision

Agent operation feedback is a **semi-transparent mask plus text**, with
sprite glyphs below both:

- Over the visible read-only mirror, Shell draws a neutral scrim mask over
  the window rectangle and a label naming the Interaction Domain and the
  applied operation class. The mask is translucent enough that the user
  still reads the window content. Paused domains dim as before; activity
  still holds for four seconds and fades out by six.
- The applied pointer position is an ordinary arrow-cursor sprite (the
  bundled Bibata `left_ptr`), and clicks and scrolls show a simplified
  mouse sprite with the pressed button or wheel highlighted. Both sprites
  render below the mask and label. Colors come from shared design tokens;
  per-domain hues are gone from the guard border as well.
- The projection is structured around an `OperationRegion` — window-scoped
  today, resolving to the mirror's display-clipped rectangle. A future
  workspace-scoped domain (an entire workspace handed to an Interaction
  Domain) resolves to a whole-output rectangle at the same seam; that
  extension is kept open deliberately and is not enabled.

Targeted Agent input **no longer raises its window**. `change_keyboard_focus`
skips the stacking raise while `synthetic_target` is set; keyboard focus
remains scoped to the Agent's independent seat, so a batch changes neither
the human's focus nor the human's stacking order, and other windows may
cover the Agent-operated window between and during batches.

Read-only mirrors are **movable by the physical user**. Press-and-drag on a
mirror's guard is the single gesture that escapes the input barrier: the
guard chrome requests the move, and the server starts an interactive-move
grab for the observed window without touching keyboard focus or stacking.
Moving is presentation state only — focus, resize, minimize, close, and
client input remain barred, and the mirror keeps its read-only semantics.
Agent pointer coordinates are already target-local
(`SyntheticInputAction`), so an applied operation lands correctly however
the user repositions the mirror mid-operation; no coordinate translation
depends on where the window used to be.

## Alternatives

- **Keep per-domain hues for multi-domain distinction.** Rejected: the mask
  plus label already names the domain, and simultaneous Agent domains are
  rare enough that identity colors cost more visual noise than they buy.
- **Draw the sprites above the mask.** Rejected: the mask is the "this area
  is externally operated" signal; letting the sprite punch through it would
  recreate the old marker-above-content reading.
- **Raise on the first batch of an operation only.** Rejected as
  unpredictable: whether the window jumps forward would depend on invisible
  batch boundaries. Never raising is the consistent rule; the mask and the
  persistent guard keep the window identifiable wherever it sits.
- **Let the human move a mirror through the normal `start_interactive_move`
  gate.** Rejected: that gate requires human control authority and focuses
  the window, which would defocus the user's own window for a pure
  presentation adjustment. The dedicated mirror-move path checks human
  observation and skips focus entirely.
- **Translate Agent coordinates on mirror moves.** Unnecessary: preparation
  converts target-local to global against the current window origin per
  batch, so correctness follows from the existing coordinate model.

## Consequences

- The user can watch an Agent operate without color noise, cover the
  operated window with their own windows, and drag the mirror aside —
  supervision no longer competes with using the desktop.
- The crosshair, movement trail, click pulse ring, and identity palette are
  removed; the visual vocabulary is mask + text + cursor/mouse sprites.
- Agent input batches are stacking-neutral; tests or tools that relied on
  an Agent batch bringing its window to the front must observe stacking
  directly instead.
- Mirror moves are compositor-owned gestures initiated by trusted chrome;
  they bypass the command/journal path exactly like title-bar drags and
  grant the human no new authority over the window's content.
- A workspace-scoped `OperationRegion` remains a documented extension
  point; enabling it is a separate decision.
