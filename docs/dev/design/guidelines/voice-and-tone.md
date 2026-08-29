# Voice and Tone

Status: **Partial**.

Aegis speaks directly, calmly, and precisely. Product copy names the object,
state, and available action without personality filler or blame. Security and
system messages become firmer as consequence increases, not more dramatic.

## Voice principles

| Principle | Application |
|----------|-------------|
| Direct | Lead with the state or action; remove introductions such as “It looks like.” |
| Specific | Name the affected application, setting, device, or resource when safe. |
| Calm | Describe risk and failure without alarmist punctuation or jokes. |
| Actionable | Give the next safe step when the user can recover. |
| Honest | Distinguish unavailable, disconnected, refused, and failed states. |

## Capitalization and labels

English UI uses sentence case except proper names and standardized protocol
or platform names. Buttons and menu items begin with a concise verb when they
perform an action. Settings labels name the property; switches do not include
“enable” when the label and state already communicate it.

- Use “Cancel” for abandoning an in-progress operation without commitment.
- Use “Close” for dismissing a completed or informational surface.
- Name destructive actions precisely: “Remove account,” not “OK.”
- Use an ellipsis only when an action opens another step before it commits.
- Avoid slashes, ampersands, and sentence fragments that cannot translate
  cleanly.

## Error messages

An actionable error has up to three parts:

1. What failed: “Display settings were not applied.”
2. Why, when the reason is known and safe: “The output disconnected.”
3. What to do next: “Reconnect it and try again.”

Do not expose raw protocol messages, stack traces, credentials, filesystem
paths, or authority internals in primary UI copy. A diagnostic detail view
may provide a stable error identifier and sanitized technical context.

## Security and consent

Consent copy identifies the requester, requested capability, affected
resource, and duration. Avoid vague verbs such as “access” when the operation
is specifically view, control, capture, or store. A durable grant must never
look equivalent to a one-time grant.

## Localization contract

Product code uses strongly typed message identifiers for shared shell copy.
Do not build sentences by concatenating translated fragments. Parameters,
plural forms, and reordering need message-level formatting before they are
introduced. Source copy leaves enough context for translators to identify
the surface, grammatical role, and consequence.

## Adoption work

- Inventory remaining hard-coded user-visible strings outside the shared
  catalog.
- Add standard copy patterns for empty, loading, permission, validation, and
  destructive states.
- Add terminology and tone review to the component example workflow.

See [Internationalization](internationalization.md) and
[State and Feedback](../patterns/state-and-feedback.md).
