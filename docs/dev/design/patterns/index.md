# Patterns

Patterns define behavior that spans multiple components or a complete user
task. A pattern owns sequencing, state transitions, dismissal, and recovery;
it does not redefine the visual foundations of its components.

## Inventory

| ID | Pattern group | Status | Scope |
|----|---------------|--------|-------|
| 3.1 | [State and Feedback](state-and-feedback.md) | Partial | Loading, empty, error, toast, and notification states |
| 3.2 | [Common Flows](common-flows.md) | Partial | Authentication, search and filtering, and multi-step work |
| 3.3 | [Interaction Paradigms](interaction-paradigms.md) | Partial | Drag and drop, keyboard shortcuts, and context menus |

## Pattern contract

An adopted pattern documents its entry conditions, intermediate states,
completion and cancellation, focus destination, error recovery, and reduced
motion behavior. Security-sensitive patterns also identify the authoritative
state owner and avoid optimistic success presentation before the operation is
accepted.

Return to the [Design Language](../index.md).
