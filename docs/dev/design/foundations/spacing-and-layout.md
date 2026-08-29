# Space and Layout

Status: **Partial**.

Aegis has shared component metrics and layout helpers, but it does not yet
have one complete spacing scale, responsive grid, or breakpoint vocabulary.
Existing shared values remain authoritative for their components; this page
defines how they grow into a system without relabeling local constants as
global tokens.

## Coordinate model

| Space | Use | Rule |
|------|-----|------|
| Logical pixels | Component size, gaps, padding, pointer targets | Default unit for `lens` layout and Aegis tokens. |
| Physical pixels | Raster coverage, optical sampling, hairline verification | Derive from logical values at the output scale. |
| Output space | Chrome placement and work areas | Respect output geometry, usable area, and scale. |
| Content space | Client buffers and captured previews | Convert explicitly; do not reuse chrome coordinates by accident. |

Fractional output scale must not change semantic layout. Raster edges may
snap during rendering, but the solved logical geometry remains stable.

## Current shared metrics

`aegis-design::materials::surface_layout` preserves a 4 px child gap and a
6 px surface pad where that legacy surface contract applies. `aegis-ui`
centralizes repeated menu, dialog, picker, chip, and settings metrics. Other
screen and surface layouts remain with their owning components.

These values do not yet constitute a universal spacing scale. A metric is a
system token only when its name describes a reusable relationship, such as
compact row height or modal content padding.

## Layout rules

- Prefer parent-owned gaps and padding over child margins.
- Keep pointer and visual rectangles explicit when the visible shape is
  smaller than the interaction target.
- Derive nested corner and inset geometry concentrically; do not visually
  center by trial-and-error offsets.
- Allow translated copy and user text scale to grow vertically. Truncation is
  acceptable only when the full value remains available through another
  accessible presentation.
- Place transient surfaces within the usable output and preserve an anchor
  relationship when space permits.
- Treat a breakpoint as a semantic layout change, not an arbitrary width at
  which one component happened to fit.

## Sizing and grids

Prefer intrinsic content size with explicit minimum and maximum bounds.
Fixed size is reserved for controls, media, or stable chrome geometry whose
role requires it. Repeated surfaces align their major regions to shared
parent guides; children do not simulate a grid through unrelated offsets.

No column grid is adopted. A future layout grid defines its container inset,
column count, gutter, density, and behavior under narrow bounds as one
contract. Until then, owner-local grids stay documented with their surface
and do not export product-wide breakpoint names.

## Responsive model

No named breakpoints are adopted. Until they are, components solve from
available bounds and document local minimum sizes. A shared breakpoint set
requires evidence from multiple surfaces and tests at each boundary. Desktop
chrome must cover narrow nested outputs, common laptop sizes, ultrawide
outputs, and fractional scaling before that set becomes adopted.

## Adoption work

- Inventory repeated gaps, insets, row heights, and target sizes.
- Define semantic compact, regular, and spacious density roles only where
  component evidence supports them.
- Add layout tests for long English, Simplified Chinese, and maximum supported
  text scale.
- Establish responsive component examples before naming product breakpoints.
