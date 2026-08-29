# Common Flows

Status: **Partial**.

Common flows combine components into repeatable tasks. The flow owns state
progression and recovery, while security and domain services remain the
source of truth.

## Authentication and consent

Aegis currently has lock authentication, secret prompts, confirmation
prompts, capability grants, and application picking. These surfaces share
modal scaffolding but retain distinct authority and outcomes.

- State who or what requests the action and which resource is affected.
- Focus the first safe, reversible choice. Do not default focus to a
  destructive or durable grant.
- Keep secret text masked, exclude it from semantic publication, and clear it
  after completion or cancellation.
- Distinguish authentication failure from transport or service failure.
- Do not present success until the authoritative owner accepts the request.
- Make cancel and refusal available through both a visible action and
  `Escape` unless the security boundary explicitly forbids dismissal.

## Search and filtering

Prism provides the current application-search reference. A general search
flow follows the same state sequence:

1. Focus starts in the query field when search is the surface's primary job.
2. Results update without moving focus away from the field.
3. Keyboard navigation moves an active result while the query remains
   editable.
4. Empty results distinguish no data from a still-running request or an
   active filter.
5. `Escape` first clears transient navigation or filters, then dismisses the
   search surface according to its context.

Filters remain visible while active, have an accessible name, and provide a
single clear-all path. Result identity, not row position, preserves selection
as data changes.

## Multi-step work

No shared wizard component is adopted. A multi-step flow is appropriate only
when later choices depend on validated earlier choices or when one screen
would create unsafe cognitive load.

- Show the current step, total or named sequence, and completion state.
- Preserve entered data when moving backward.
- Validate at the closest responsible step and keep the failing field in
  view.
- Allow cancellation and state whether partial work is saved.
- Commit once at the authoritative boundary when atomicity matters.
- Restore focus to the first changed or invalid element after navigation.

## Adoption work

Extract a shared flow only after independent surfaces use the same sequence.
The extraction includes state transitions, focus movement, cancellation, and
error recovery; sharing only a panel layout does not make a shared flow.
