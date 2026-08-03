# ADR-0107: Rounded, color-preserving glass previews

- Status: Accepted
- Date: 2026-08-03

## Context

ADR-0105 replaced blue-white selection outlines in Dock live previews and the
window switcher with one Liquid Glass focus field. Two visual inconsistencies
remained after that change.

First, the card indicator used the shared rounded `control` geometry while the
live client scene was constrained only by a rectangular scissor. Window pixels
therefore reached the square corners underneath a rounded selection state.
Rounding each client surface independently would also be wrong: a window is a
composed tree of its toplevel, subsurfaces, and popups, and those images must
form one silhouette.

Second, nonfocused previews were made translucent. Their pixels blended with
the pale glass effect below and looked desaturated or white instead of merely
receding. Increasing the selected white wash would make the focused target more
obvious, but would obscure the window content that the preview exists to show.

## Decision

Every image in one mapped preview surface tree is composited through the same
analytic rounded clip. The clip is the card's preview rectangle and its radius
is the same `radii.control` token used by hover and selection geometry. The
existing rectangular scissor remains only as a coarse performance bound; it is
not the visible silhouette.

Preview visibility and focus hierarchy are separate channels. Opening and
closing the panel uses opacity. During focus, the selected preview remains
opaque at full brightness and nonfocused siblings remain opaque at brightness
`0.74`. Labels follow the same hierarchy. This prevents client pixels from
mixing with the glass simply because they are not selected.

The selected foreground wash is reduced to neutral white alpha 3 and hover to
alpha 6. The parent Liquid Glass focus supplies the visible selection by
substantially reducing frost and adaptive tint, preserving more chroma, and
adding only a small multiplicative directional gain. It adds no white light
pool, accent color, outline, inner rim, or second glass body.

Card radius, selected identity, sibling brightness, and visibility participate
in exact scene/effect cache invalidation. Dock live previews and the window
switcher share this complete contract.

## Alternatives

- **Rectangular clipping beneath rounded chrome.** Rejected because content and
  state would retain visibly different silhouettes.
- **Round every toplevel, subsurface, and popup independently.** Rejected
  because internal surface boundaries would acquire unrelated rounded corners.
- **Lower sibling opacity.** Rejected because it blends previews into the pale
  material and produces the reported washed-out appearance.
- **Increase the selected white fill or restore an outline.** Rejected because
  it hides content and reintroduces a painted control convention that conflicts
  with Liquid Glass.

## Consequences

- Rounded preview coverage is antialiased by Optics and applies to the complete
  client surface tree, not reconstructed by component-specific corner masks.
- Selection is more legible through clarity, chroma, and relative luminance
  while focused client colors remain recognizably their own.
- A mapped preview draw uses source-over blending so antialiased pixels outside
  the rounded clip cannot erase the existing destination.
- Optics exposes the independent rounded image clip recorded in ADR-0051.

## References

- [ADR-0105 — Single-body Liquid Glass interaction focus](0105-single-body-liquid-glass-interaction-focus.md)
- [Liquid Glass design specification](../dev/design/liquid-glass.md)
- [Surface design specification](../dev/design/surfaces.md)
