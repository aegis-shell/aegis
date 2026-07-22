# ADR-0048: Compositor-owned Agent operation feedback (amends ADR-0040)

- Status: Accepted
- Date: 2026-07-22

## Context

ADR-0040 gives each Agent Realm an independent logical seat. The physical
user can observe retained read-only window mirrors, Realm state in the status
bar and Control Center, and applied/refused decisions in the mutation journal.
Those surfaces do not show where an Agent pointer moved or clicked. Reusing
the physical XDG cursor would conflate two independent seats, make input
ownership ambiguous, and let an Agent operation change the user's cursor.

Operation feedback also crosses a trust boundary. A model or client-provided
overlay could claim that an action happened before the compositor applied it,
obscure a different target, or enter the Agent's directed capture and become a
self-referential visual control signal. Keyboard details can contain secrets
and must not be copied into presentation state.

## Decision

Render Agent operation feedback as non-interactive, compositor-owned Shell
chrome that is separate from the physical user's XDG cursor.

The main loop publishes feedback only after `forward_agent_input_to` succeeds.
Each event carries a monotonic presentation sequence, Realm and target-window
identity, the exact applied global pointer position when one exists, and a
privacy-preserving operation class. Keyboard feedback says only that keyboard
input occurred; it never carries the key code or text.

On a visible read-only human mirror, Shell draws a circular crosshair rather
than an arrow cursor. Movement has a short trail, clicks have a pulse, and a
label names the Realm and operation. Realm identity uses a stable color plus
shape and text, so color is never the sole signal. If the target is not visible
or an operation has no pointer position, Shell shows a compact background-
operation pill below the status bar. Paused Realm feedback is dimmed, revocation
removes it, and recent activity fades after six seconds. The persistent status
bar indicator remains the source for longer-lived Realm state.

The feedback component never captures pointer or keyboard input and never
moves, hides, or restyles the user's XDG cursor. It renders after client
content but below notifications and modal trusted chrome. The secure lock path
hides Shell chrome. Directed Realm capture continues to render Realm client
surfaces directly, so Agent feedback is excluded and cannot enter the Agent's
own observation loop.

## Alternatives

- **Reuse the physical XDG cursor.** Rejected because the human and Agent seats
  are independent authorities and must remain visually distinguishable.
- **Let the Agent draw its own cursor overlay.** Rejected because untrusted
  pixels cannot prove that the compositor applied an operation.
- **Include feedback in directed Realm captures.** Rejected because the Agent
  would observe its own presentation signal and could steer from stale or
  recursive feedback.
- **Show only the status bar Realm indicator and journal.** Rejected because
  they prove lifecycle and outcomes but do not locate pointer behavior for
  real-time human supervision.
- **Display key names or typed text.** Rejected because trusted chrome and
  desktop screenshots would become a credential and content side channel.

## Consequences

- The user can distinguish Agent movement and clicks from their own cursor and
  can still supervise keyboard or off-screen activity at a coarser level.
- Visual feedback is evidence of a successfully applied compositor operation;
  queued or refused commands never produce it.
- Exact pointer projection is available only while the human has a visible
  read-only mirror. Otherwise the UI intentionally reports background activity
  without inventing a screen position.
- Shell keeps a small amount of ephemeral per-Realm presentation state and
  ticks frames during its bounded fade interval.
- Visual acceptance can use a real, non-clicking pointer probe. The Neenee
  smoke command requires an explicit window id, verifies the applied journal
  entry, retains the human observer mirror, and restores authority during
  cleanup.
