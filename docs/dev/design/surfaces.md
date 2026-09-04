# Surfaces

Chrome surfaces pick one material each, by role. The analytic
[Liquid Glass](liquid-glass.md) material renders in the compositor's SDF
pass; other surfaces are painted Lens overlays and may be opaque or sit over
the shared frosted backdrop blur. Tokens live in `tessera-design` (see
[ADR-0046](../../adr/0046-design-system-crate.md)); components read them
through material factories instead of hard-coding values.

## Material selection

| Role | Material | When to use |
|------|----------|-------------|
| Floating control layer | Liquid Glass (analytic SDF) | Dock bar, HUD chips, and any persistent or modal body that floats above client content |
| Transient panels | Frosted popover | Menus, popovers, and small panels that need a readable body over arbitrary content without lensing |
| Content cards | Card fill | Settings and system-management cards inside a panel; content layer, never floating chrome |
| Command panel | Solid grouped surface | Opaque light/dark canvas, elevated panels, recessed cards, and system-blue interaction states |

Do not improvise new materials per component. If none of the rows fits,
extend `tessera-design` with a semantic factory and document it here.

Selection markers are not surfaces. A region selection, window highlight,
or other marquee over pixels the user is about to capture never takes a
material at all: no glass, no blur, no rounded corners. It is a square-
cornered border over untouched content, because the preview must show
exactly what the capture will include. Glass on a capture marker blurs the
content being framed and rounds corners the capture actually keeps, which
reads as a floating surface rather than a tool. Optical instruments are
the exception — the pixel-picking loupe is a lens, so it keeps its glass
body.

## Surface tokens (dark appearance)

| Token | Value | Used by |
|-------|-------|---------|
| `glass_surface` | white, alpha 12 | Minimal foreground tint shared by analytic glass panels |
| `glass_border` | alpha 0 | Reserved; the glass rim supplies the edge |
| `popover_surface` | white, alpha 110 | Popover and menu bodies over frost |
| `popover_border` | white, alpha 72, 1 px | Popover edge against content |
| `card_surface` | white, alpha 14 | Card fill on panels |
| `radii.glass_panel` | 18 px | Shared outer radius for Dock, live-preview, and switcher glass panels |
| `glass_focus.hover_tint` | white, alpha 6 | Immediate pointer feedback inside glass |
| `glass_focus.selected_tint` | white, alpha 3 | Near-transparent fallback beneath the optical selection field |
| `glass_focus.field_strength` | 1.0 | Canonical selected-state optical focus strength |
| `preview.inactive_content_brightness` | 0.74 | Opaque brightness for nonfocused preview siblings |
| `preview.focused` | scale 1.0, lift 0 px | Stationary focus inside an anchored preview panel |
| `preview.staged` | scale 1.06, lift 7 px | Restrained foreground staging in the window switcher |
| `CommandPanelColors::background` | rgb(18, 18, 20), alpha 255 | Command panel dark grouped background |
| `CommandPanelColors::surface` | rgb(28, 28, 30), alpha 255 | Command panel dark elevated surfaces |
| `CommandPanelColors::surface_recessed` | rgb(22, 22, 24), alpha 255 | Command panel dark recessed surfaces |
| `CommandPanelColors::border` | white, alpha 28 | Command panel quiet separators |
| `CommandPanelColors::accent` | rgb(10, 132, 255) | Command panel active tabs, sliders, and gauges |
| `CommandPanelColors::selection_surface` | rgb(24, 55, 86), alpha 255 | Command panel dark selected surface |

The light command-panel appearance maps the same roles to
`rgb(242, 242, 247)` for the grouped background, opaque white elevated
surfaces, dark text, and `rgb(0, 122, 255)` for the accent.

## Liquid Glass roles

Liquid Glass uses semantic roles rather than numbered intensity levels.
Every role keeps the same refraction, rim-light identity, and curve
shapes. Roles vary in two ways: the per-body elevation shadow, and —
for text-bearing bodies — the material strengths and plate polarity
that keep glyphs legible over arbitrary content (see
[Liquid Glass](liquid-glass.md)).

| Role | Shadow alpha | Blur | Y offset | Use |
|------|--------------|------|----------|-----|
| `Chip` | 0.16 | 4 px | 2 px | HUD chips |
| `Tooltip` | 0.14 | 10 px | 5 px | Dock hover labels and similar attached hints |
| `Menu` | 0.18 | 16 px | 8 px | Text-bearing transient surfaces: the Dock context menu and the launcher menu |
| `FloatingPanel` | 0.18 | 16 px | 8 px | Dock live previews, the window switcher, and the screenshot status pills |
| `ProminentPanel` | 0.20 | 18 px | 9 px | Modal prompts, the app picker, and Prism |
| `Dock` | 0.20 | 12 px | 6 px | The resting Dock; morphing scales blur and offset with its body |

`Menu` and `Tooltip` are the legibility roles: they multiply interior
frost and adaptive tint, damp the backdrop's surviving chroma, and pin
the plate polarity against their text tone (smoke under the dark
appearance's light text, pearl under the light appearance's dark text).
The other roles keep the reference recipe and the shader's per-pixel
polarity. The exact strengths are tokens in `tessera-design`.

The current role-to-component mapping:

| Component | Role |
|-----------|------|
| App context menu (Dock, launcher) | `Menu` |
| Dock bar | `Dock` |
| Dock hover surface | `Tooltip`, or `FloatingPanel` when it hosts live previews |
| Window switcher | `FloatingPanel` |
| Screenshot selection | `FloatingPanel` |
| Modal prompts and app picker | `ProminentPanel` |
| Prism | `ProminentPanel` |
| HUD | `Chip` |

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
