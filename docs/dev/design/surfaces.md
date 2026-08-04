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
| `glass_surface` | white, alpha 12 | Minimal foreground tint shared by analytic glass panels |
| `glass_border` | alpha 0 | Reserved; the glass rim supplies the edge |
| `popover_surface` | white, alpha 38 | Popover and menu bodies over frost |
| `popover_border` | white, alpha 72, 1 px | Popover edge against content |
| `card_surface` | white, alpha 14 | Card fill on panels |
| `radii.glass_panel` | 18 px | Shared outer radius for Dock, live-preview, and switcher glass panels |
| `glass_focus.hover_tint` | white, alpha 6 | Immediate pointer feedback inside glass |
| `glass_focus.selected_tint` | white, alpha 3 | Near-transparent fallback beneath the optical selection field |
| `glass_focus.field_strength` | 1.0 | Canonical selected-state optical focus strength |
| `preview.inactive_content_brightness` | 0.74 | Opaque brightness for nonfocused preview siblings |
| `preview.focused` | scale 1.0, lift 0 px | Stationary focus inside an anchored preview panel |
| `preview.staged` | scale 1.06, lift 7 px | Restrained foreground staging in the window switcher |
| `sao.surface` | rgb(248, 249, 252), alpha 226 | Command panel surfaces |
| `sao.border` | SAO palette | Command panel edge |

## Liquid Glass roles

Liquid Glass uses semantic roles rather than numbered intensity levels.
Every role keeps the same refraction, adaptive tint, and rim-light identity;
only the per-body elevation shadow changes.

| Role | Shadow alpha | Blur | Y offset | Use |
|------|--------------|------|----------|-----|
| `Chip` | 0.16 | 4 px | 2 px | Compact HUD bodies |
| `Tooltip` | 0.14 | 10 px | 5 px | Dock labels and similar attached hints |
| `FloatingPanel` | 0.18 | 16 px | 8 px | Preview panels, switcher, and screenshot selection |
| `ProminentPanel` | 0.20 | 18 px | 9 px | Primary floating surfaces such as Prism |
| `Dock` | 0.20 | 12 px | 6 px | The resting Dock; morphing scales blur and offset with its body |

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

## Interaction hierarchy

Glass-hosted choices use one shared progression across components:

| State | Treatment |
|-------|-----------|
| Rest | No indicator; content and the parent material carry the hierarchy |
| Hover | Neutral alpha-6 foreground wash, no outline and no new glass body |
| Selected | Parent body's color-preserving optical focus plus neutral alpha-3 fallback; nonfocused preview content stays opaque at 74% brightness |
| Semantic accent | Reserved for meaning such as a primary action, status, warning, or application identity; never used only to mark the current structural choice |

The Dock live-preview panel and window switcher use this exact contract,
including the shared control and panel radii. Client pixels, subsurfaces, and
popups belonging to one preview are all composited through the same analytic
`control`-radius clip, so the content and interaction geometry have one
silhouette. The focus field moves between cards inside their one parent glass
body. Components must not reinterpret selection as a blue-white border,
nested glass card, or opaque fill.

Preview visibility and preview hierarchy are independent. Opacity is reserved
for opening and closing the complete panel; it must not de-emphasize sibling
preview pixels, because translucent client content blends with the pale glass
below and appears washed out. Use the shared brightness token instead.

## Preview anatomy

A preview group is one `FloatingPanel` Liquid Glass body. Each card is
ordinary content inside that parent, never a nested glass body. The card's
outer rectangle includes its live preview and label and is the authoritative
pointer and optical-focus target. The preview rectangle alone clips the live
client surface with `radii.control`.

The `Focused` selection treatment leaves card geometry stationary. The
`Staged` treatment adds the shared scale and upward lift for presentations
such as the held-modifier window switcher. Both treatments retain the same
single focus field, neutral selected wash, and sibling-brightness policy.
Staging changes geometry, not the material hierarchy.
