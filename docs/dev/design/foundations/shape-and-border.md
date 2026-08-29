# Shape and Border

Status: **Adopted** for shared radii and strokes; keyboard focus treatment is
partial.

Shape communicates hierarchy and hit geometry. A visible silhouette, its
clip, and its interaction bounds use the same semantic role unless the
component explicitly provides a larger invisible target.

## Radius roles

The current `Radii` snapshot is scheme-invariant.

| Token | Logical radius | Typical use |
|------|----------------|-------------|
| `menu_item` | 7 px | Compact menu rows |
| `popover` | 12 px | Frosted popover bodies |
| `control` | 12 px | Controls and live-preview clips |
| `card` | 16 px | Content cards |
| `chip` | 16 px | HUD chips |
| `glass_panel` | 18 px | Floating glass panels |
| `cell` | 22 px | Large cells and grouped controls |
| `application` | 24 px | Application-level surfaces |
| `scrollbar` | 2.5 px | Scrollbar thumb |

Use a role token instead of choosing the nearest number. If an element has no
matching role, keep its radius local until multiple components establish a
shared meaning.

## Stroke roles

`Strokes::hairline` is 1 logical px and `Strokes::scrollbar` is 5 logical px.
A hairline still resolves at the physical output scale; it must not disappear
or become uneven after rasterization.

Borders separate painted surfaces. Analytic Liquid Glass uses its rim,
absorption, and shadow for edge definition and therefore does not receive a
painted structural border. Popovers and opaque panels may use their semantic
border role where the material needs that separation.

## Geometry rules

- Keep nested shapes concentric. Inner radii follow the outer radius and
  inset instead of introducing unrelated curves.
- Use one authoritative rounded geometry for paint, clip, hit testing, and
  optical focus.
- Do not add an outline merely to indicate hover or selection inside glass.
- Reserve rings for semantic focus, identity, progress, or status roles.
- Test very small shapes after output scaling; a nominal radius may collapse
  into a capsule or disc and should do so intentionally.

## Focus gap

Glass selection and keyboard focus are distinct. The current neutral glass
focus field is adopted for structural selection, but a complete cross-surface
keyboard focus indicator is not yet centralized. New components must still
provide a visible keyboard focus state and document how it remains distinct
from selection and validation.

See [Liquid Glass](../liquid-glass.md) for analytic silhouette rules and
[Accessibility](../guidelines/accessibility.md) for focus requirements.
