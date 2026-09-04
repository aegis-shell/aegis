# Assets

Status: **Partial**.

Assets include icons, cursors, identity marks, illustrations, wallpapers,
portraits, and other media placed inside Tessera-owned UI. They carry content;
they do not bypass the color, shape, accessibility, or platform contracts.

## Current asset paths

| Asset class | Current contract |
|------------|------------------|
| Application icons | Resolve through XDG desktop entries and the configured icon theme. Preserve application identity. |
| Generic chrome icons | Prefer vector or `lens` drawing primitives that inherit a semantic foreground role. |
| Cursors | Use the bundled Tessera SVG cursor theme with standard cursor-name aliases and configured size fallback. |
| Illustrations | No shared illustration family is adopted; treat current illustrations as owner-local editorial assets. |
| Wallpapers | Treat as user content beneath chrome; never assume a friendly luminance or hue. |
| Persona portraits | Apply role-based frame styles while keeping profile content outside the design token contract. |

## Icon rules

- Use a standard semantic icon before adding a product-specific drawing.
- Keep icon meaning stable across surfaces. A glyph used for close does not
  become delete elsewhere without an explicit destructive context.
- Do not recolor application icons to fit the chrome palette.
- Pair unfamiliar or safety-critical icons with visible text.
- Keep the vector view box, optical center, and stroke weight consistent
  within an icon family; padding is part of the asset contract.
- Supply a text alternative or semantic label for interactive icons.

## Media geometry

No global media aspect-ratio tokens are adopted. Components must declare
their supported ratios and fit mode. Portraits crop from a stable focal
region; live window previews preserve client aspect ratio; wallpapers follow
their configured fit or parallax mode. Cropping must never hide essential
status or instructional content.

## Asset intake

Every bundled asset records its source, license, intended role, format, and
fallback. Prefer SVG for scalable chrome art and lossless formats where
pixel identity matters. Raster variants must cover the output scales at
which interpolation becomes visibly soft.

## Adoption work

- Create an inventory for non-cursor product icons and their semantic names.
- Define icon size and optical-padding roles after repeated use is measured.
- Add aspect-ratio tokens only for recurring media components.
- Add automated license and missing-fallback checks to the asset pipeline.

See [Persona Portraits](../persona.md) for portrait-specific rules.
