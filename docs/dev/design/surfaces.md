# Surfaces

Chrome surfaces pick one material each, by role. The analytic
[Liquid Glass](liquid-glass.md) material renders in the compositor's SDF
pass; everything else is a painted lens overlay over the shared frosted
backdrop blur. Tokens live in `aegis-design` (see
[ADR-0046](../../adr/0046-design-system-crate.md)); components read them
through material factories instead of hard-coding values.

## Material selection

| Role | Material | When to use |
|------|----------|-------------|
| Floating control layer | Liquid Glass (analytic SDF) | Dock bar, HUD chips, and any persistent or modal body that floats above client content |
| Transient panels | Frosted popover | Menus, popovers, and small panels that need a readable body over arbitrary content without lensing |
| Content cards | Card fill | Settings and system-management cards inside a panel; content layer, never floating chrome |
| Light island | SAO panel | The command panel's frosted white surfaces — header band, icon rail, and content panel: deliberate light islands inside the dark appearance |

Do not improvise new materials per component. If none of the rows fits,
extend `aegis-design` with a semantic factory and document it here.

## Surface tokens (dark appearance)

| Token | Value | Used by |
|-------|-------|---------|
| `dock_surface` | white, alpha 12 | Dock resting body tint over the glass pass |
| `dock_border` | alpha 0 | Reserved; the glass rim supplies the edge |
| `popover_surface` | white, alpha 38 | Popover and menu bodies over frost |
| `popover_border` | white, alpha 72, 1 px | Popover edge against content |
| `card_surface` | white, alpha 14 | Card fill on panels |
| `sao.surface` | rgb(248, 249, 252), alpha 226 | Command panel surfaces |
| `sao.border` | SAO palette | Command panel edge |

## Layering rules

- The Dock's painted layer stays minimal on purpose: a whisper of white
  for cohesion and no border. Edge definition is the glass rim's job;
  a painted outline around glass reads as a sticker, not a material.
- Popovers keep their 1 px border: without lensing, a painted hairline
  is what separates a frosted panel from the content beneath it.
- Tint alpha on glass never exceeds the Dock's resting alpha. More tint
  means the surface is fighting the content; fix the role, not the
  alpha.
- Toast and HUD text on glass uses the product text colors at full
  strength; the material's adaptive tint preserves their contrast.
