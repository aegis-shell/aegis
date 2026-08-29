# State and Feedback

Status: **Partial**.

State feedback tells the user what the system is doing, what data is
available, and how to recover. It stays near the action or content it
describes unless the event remains relevant after that surface disappears.

## State model

| State | Presentation | Required behavior |
|------|--------------|-------------------|
| Loading | Preserve the destination structure; show progress near the affected region | Prevent duplicate work and keep cancellation available when supported. |
| Empty | Explain that the request succeeded but produced no content | Offer the most likely next action, filter reset, or creation path. |
| Partial or stale | Keep usable data visible with a clear freshness warning | Identify what is unavailable and allow retry without discarding good state. |
| Error | Place a concise message beside the failed operation or region | Preserve input, state the recovery, and avoid implying success. |
| Success | Update the authoritative content; add transient confirmation only when the result is otherwise invisible | Do not make the user dismiss routine success. |

Loading, empty, and error are states of a component or flow. They are not
separate pages by default, and switching among them should preserve the
surrounding focus and layout where practical.

## Toasts and notifications

A **toast** is transient in-session feedback for a recent action. A
**notification** is durable enough to revisit, may arrive in the background,
and belongs to the notification model.

- Use inline feedback when the source remains visible.
- Use a toast when the result would otherwise be invisible after the source
  closes or focus moves.
- Use a notification for asynchronous, time-relevant events that the user may
  need later.
- Do not emit both a toast and notification for the same event unless each
  carries a distinct role.
- Coalesce repeated events and preserve the newest authoritative state.
- Critical security prompts are modal decisions, not notifications styled as
  alerts.

## Timing and dismissal

Transient feedback pauses its timeout while hovered or keyboard-focused and
never disappears before assistive technology can announce it. Important
recovery instructions remain inline or durable. Reduced motion removes
entrance and exit animation but not the message or its dwell time.

## Error contract

An error message names the failed object or action, gives the usable reason
when safe, and provides a next step. Log identifiers and protocol text remain
diagnostic detail; user copy does not expose secrets or internal topology.
See [Voice and Tone](../guidelines/voice-and-tone.md).

## Adoption work

- Define shared progress, inline error, and empty-state compositions.
- Centralize toast priority, coalescing, focus, and dwell-time policy.
- Add live-region or equivalent semantic announcements for state changes.
- Test optimistic settings updates against refusal and revision conflicts.
