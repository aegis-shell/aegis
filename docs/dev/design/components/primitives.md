# Primitive Components

Status: **Partial**.

Primitive components are the smallest interactive or presentational units in
the product system. Generic mechanics belong in Optics `lens`; Aegis adds
semantic themes, product roles, and composition only when multiple product
surfaces need the same contract.

## Inventory

| Primitive | Current state | Shared owner |
|-----------|---------------|--------------|
| Button | Generic widget exists; dialog action-button composition is shared | `lens`, then `aegis-ui::dialog` |
| Text input | Generic widget and bounded product buffers exist; product variants are not cataloged | `lens`, then the owning flow |
| Checkbox and switch | Generic widgets are used in settings; state matrix is not yet documented centrally | `lens` |
| Slider | Shared theme roles exist; labeling and validation remain flow-owned | `lens` and `aegis-design` |
| Tooltip | Glass material role exists; trigger, delay, and dismissal are not centralized | Owning chrome component |
| Icon view | Application icon loading and vector drawing exist; no single product wrapper exists | Asset resolver and `lens` |

This inventory does not require Aegis wrappers around every `lens` widget.
A wrapper is justified only by repeated product anatomy or behavior.

## Required state contract

Interactive primitives expose every applicable state without changing their
identity or layout unexpectedly:

| State | Requirement |
|------|-------------|
| Rest | Clear affordance and readable label or accessible name |
| Hover | Immediate, restrained feedback; never the only discoverability cue |
| Pressed | Same-frame acknowledgement tied to the active pointer or key |
| Focused | Persistent keyboard focus indication distinct from selection |
| Disabled | Inert behavior plus a perceivable unavailable state and reason when useful |
| Loading | Prevent duplicate activation while preserving the action label or outcome context |
| Invalid | Semantic error indication linked to explanatory copy |

## Input behavior

- Buttons activate once on `Enter`, `Space`, or a completed primary click;
  destructive actions use explicit labels.
- Checkboxes and switches toggle with `Space` when focused. Their label is
  part of the activation target.
- Text inputs preserve caret, selection, and composition behavior and never
  interpret input-method preedit as a submitted value.
- Sliders expose a name, current value, range, and keyboard increments.
- Tooltips appear from both pointer hover and keyboard focus, contain no
  required interactive content, and dismiss without stealing focus.
- An icon-only control has an accessible name. Decorative icons are removed
  from the semantic tree.

## Visual rules

Use the resolved theme and semantic foundation tokens. Interaction states may
change color, material focus, or foreground treatment but must not move the
control under the pointer. Minimum target metrics remain partial until the
spacing system adopts a target-size role; new components must nevertheless
meet the accessibility target documented in
[Accessibility](../guidelines/accessibility.md).

## Adoption work

- Build a state catalog for the generic primitives used by Aegis.
- Centralize tooltip trigger and dismissal behavior after current consumers
  are inventoried.
- Define icon-size and focus-indicator roles.
- Add keyboard, disabled, loading, and translated-label examples for each
  adopted primitive.
