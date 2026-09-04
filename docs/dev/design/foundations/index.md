# Foundations

Foundations define the values from which every Tessera-owned surface is built.
They are semantic product policy, not a bag of visual constants. Generic
drawing capabilities remain in Optics; adopted Tessera values live in
`tessera-design` and shared motion math lives in `tessera-ui`.

## Inventory

| ID | Foundation | Status | Current source of truth |
|----|------------|--------|-------------------------|
| 1.1 | [Color System](color.md) | Adopted | `colors`, `ProductColors`, `CommandPanelColors`, and `Design` |
| 1.2 | [Space and Layout](spacing-and-layout.md) | Partial | Shared component metrics plus owner-local layouts |
| 1.3 | [Shape and Border](shape-and-border.md) | Adopted | `Radii`, `Strokes`, and analytic glass geometry |
| 1.4 | [Elevation and Materials](elevation-and-materials.md) | Adopted | Material factories and Liquid Glass roles |
| 1.5 | [Typography](typography.md) | Partial | `TypeScale`; typeface and text-scale propagation remain incomplete |
| 1.6 | [Motion](motion.md) | Partial | Window transition policy and `tessera-ui::motion` |
| 1.7 | [Assets](assets.md) | Partial | XDG icon resolution, Aegis cursors, and owner-local media |
| 1.8 | [Multimodal Feedback](multimodal-feedback.md) | Draft | No shared sound or haptic contract exists |

## Dependency order

Color, space, shape, typography, and motion define primitive values.
Materials combine color, shape, and elevation. Assets and multimodal feedback
add content that must still satisfy the same accessibility and platform
rules. Components may narrow these choices by role but must not create a
second foundation vocabulary.

## Contribution rule

A value belongs in a foundation when it expresses one semantic role across
multiple surfaces. Keep one-off geometry in the owning component. Promote a
value only with a semantic name, at least one shared consumer boundary, and
a test that preserves the intended relationship.

Return to the [Design Language](../index.md).
