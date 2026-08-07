# Aegis Design Language

The design language is the shared visual contract for every aegis chrome
surface: Dock, HUD, launcher, command panel, menus, and toasts. Components
built by different crates must read as one product, so the rules in this
directory are normative for contributor-written chrome.

The language takes its material cues from Apple's Liquid Glass (WWDC 2025)
and adapts them to a Wayland compositor: chrome is a floating functional
layer above client content, defined by bent light rather than by painted
fills.

## Principles

1. **Content first.** Chrome defers to the client's pixels. Materials bend,
   tint, and separate; they do not cover.
2. **Lensing, not scattering.** Separation comes from refraction and
   sculpted light. Broad blurs and milky fills are fallbacks, not the
   product look.
3. **Concentric geometry.** Shapes nest with continuous rounded corners.
   A body's corner radius follows its component token, never a magic number.
4. **Adaptation over fixed appearance.** Materials respond to the content
   beneath them — luminance, motion, and context — instead of carrying one
   fixed light or dark look.
5. **Motion is part of the material.** Springs, merges, and reveals are
   properties of the material itself, choreographed with its optics.
6. **Restraint.** Liquid Glass marks the floating control layer. Content
   surfaces use quiet fills. Painted borders and heavy tints do not belong
   on glass.
7. **One body, one hierarchy.** Interaction emphasis changes the optics and
   content hierarchy inside an existing glass body. Hover and selection never
   introduce a nested glass body or a structural accent outline.

## Pages

| Page | Purpose |
|------|---------|
| [Liquid Glass](liquid-glass.md) | The analytic glass material: optical model, lighting, adaptivity, parameters, and usage rules |
| [Surfaces](surfaces.md) | Material inventory for chrome surfaces with tokens and selection rules |
| [Command Panel](command-panel.md) | The three-surface HUD cluster: header band, tabbed main panel, side column, and motion |
| [Persona Portraits](persona.md) | Profile and portrait-content boundaries plus role-based frame styles |

## Related Documentation

- [Design system crate decision](../../adr/0046-design-system-crate.md)
- [Liquid glass lens model and full-resolution capture](../../adr/0094-liquid-glass-lens-model-and-full-resolution-capture.md)
- [Single-body liquid-glass interaction focus](../../adr/0105-single-body-liquid-glass-interaction-focus.md)
