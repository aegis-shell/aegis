# Components

Components turn foundations into reusable controls and containers. Optics
`lens` owns generic widget mechanics. `tessera-ui` owns Tessera-specific
composition and interaction scaffolding, while the consuming crate retains
domain state and emits product intents.

## Inventory

| ID | Component group | Status | Scope |
|----|-----------------|--------|-------|
| 2.1 | [Primitive Components](primitives.md) | Partial | Buttons, inputs, choices, tooltips, and icons |
| 2.2 | [Composite Containers](containers.md) | Partial | Cards, dialogs, menus, popovers, drawers, and accordions |
| 2.3 | [Complex Data Components](complex-data.md) | Draft | Tables, trees, charts, and rich editing |

## Component contract

Every adopted component defines:

- semantic anatomy and allowed variants;
- rest, hover, pressed, focused, disabled, loading, and error behavior where
  those states apply;
- pointer, keyboard, and assistive-technology behavior;
- layout behavior under text expansion and output scaling;
- foundation tokens consumed by the component;
- examples and tests covering the supported state matrix.

Components do not accept unrestricted color, radius, or motion values merely
to bypass the design system. A new variant needs a reusable semantic role.

Return to the [Design Language](../index.md).
