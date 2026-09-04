# Elevation and Materials

Status: **Adopted**.

Materials describe how a surface participates in the product hierarchy.
Elevation is a semantic role, not a freely chosen shadow. A surface selects
one material and one elevation role based on its function.

## Material families

| Family | Use | Implementation |
|-------|-----|----------------|
| Liquid Glass | Floating control layer | Analytic SDF lensing, lighting, tint, and role shadow |
| Frosted popover | Transient text-bearing panel without analytic lensing | Translucent painted surface over shared backdrop blur |
| Card fill | Content grouping inside a parent surface | Quiet painted fill |
| Solid grouped surface | Dense command-panel content | Opaque background, elevated panels, and recessed areas |
| Scrim | Modal separation and attention boundary | Output-level dimming behind one modal flow |

The selection table and exact glass elevation roles live in
[Surfaces](../surfaces.md). The analytic recipe, scaling, and backdrop
adaptation contract live in [Liquid Glass](../liquid-glass.md).

## Elevation rules

- Choose elevation by role: chip, tooltip, menu, floating panel, prominent
  panel, or Dock. Do not assemble local shadow tuples for static surfaces.
- Keep content cards within their parent material. A nested glass body is not
  a generic elevation step.
- Treat backdrop blur as a material input, not a universal decoration.
- Use a scrim for modal scope, not to compensate for insufficient text
  contrast inside a panel.
- Review shadow, rim, border, and tint as one separation system over both
  uniform white and uniform black content.

## Ownership boundary

`tessera-design` owns semantic material parameters and pure `lens` option
factories. Optics owns effects that touch pixels, images, or GPU command
buffers. The compositor connects those parameters to rendering. This
boundary follows
[ADR-0139](../../../adr/0139-animation-effect-placement.md).

## Verification

Material changes require tests for semantic role mapping and visual review on
bright, dark, detailed, and saturated backdrops. Translucent text-bearing
surfaces also require contrast evidence in both appearances. A screenshot on
one wallpaper is not sufficient material validation.
