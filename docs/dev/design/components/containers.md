# Composite Containers

Status: **Partial**.

Composite containers organize primitives, establish material hierarchy, and
often own focus or dismissal boundaries. They do not own application domain
state.

## Inventory

| Container | Current state | Contract |
|-----------|---------------|----------|
| Card | Shared material and settings composition exist | Content grouping inside a parent surface |
| Modal dialog | Shared scrim, panel placement, title, and action rows exist | One blocking decision with trapped focus and explicit exit |
| Menu | Shared metrics and row layout exist | Compact command list anchored to a source |
| Popover | Shared material exists; complete behavior remains owner-local | Transient nonmodal content anchored to a source |
| Drawer | No shared component | Reserved for edge-attached secondary tasks |
| Accordion | No shared component | Reserved for in-place disclosure of related content |

## Material and nesting

Select the container material from
[Elevation and Materials](../foundations/elevation-and-materials.md). Cards
are content inside a panel. Menus and popovers are transient surfaces. A
modal uses one scrim and one prominent panel. Nested Liquid Glass bodies are
not a generic container hierarchy.

## Behavior rules

- Keep the opener as the anchor for nonmodal surfaces and restore focus to it
  after dismissal when it still exists.
- Close menus and popovers on an accepted command, `Escape`, or an outside
  interaction according to the owning flow. Do not dismiss merely because a
  child requests more space.
- Trap keyboard focus inside a modal, define an initial focus target, and
  return focus after completion or cancellation.
- Keep a stable action order. Destructive actions are explicit and separated
  from routine confirmation where accidental activation is costly.
- Preserve scroll position while expanding or collapsing content unless the
  focused item would leave the visible region.
- Avoid placing a modal over another modal. Continue the existing flow inside
  the same boundary or complete it before opening the next one.

## Sizing and overflow

Containers solve within the usable output, then allow internal content to
scroll. They must tolerate translated labels and supported text scaling
without hiding the title, current focus, or primary recovery action. An
anchored surface may flip or shift to remain visible, but its relation to the
anchor stays apparent.

## Adoption work

- Consolidate popover placement, dismissal, and focus restoration.
- Create shared disclosure behavior before introducing an accordion visual
  variant.
- Add a drawer only when at least two surfaces need the same edge-attached
  interaction model.
- Catalog container examples for overflow, error, empty, and reduced-motion
  states.

See [State and Feedback](../patterns/state-and-feedback.md) for content states
inside these containers.
