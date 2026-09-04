# tessera-design

`tessera-design` defines the product-specific visual language shared by tessera
chrome components on top of lens.

## Responsibilities

- Centralize literal product colors in `colors` and expose semantic
  `ProductColors`, `CommandPanelColors`, `SceneColors`, and `DockColors` roles.
- Define radii, stroke widths, the chrome type scale, and role-based
  presentation policies in `tokens`.
- Own the scheme palettes for chrome surfaces, the compositor scene (clear
  colors, scrims, glass tint), and the scheme-invariant dock palette.
- Build lens themes for shared product surfaces, including theme-wide alpha
  fades for entry/exit motion.
- Build data-only material options for popovers, panels, and cards, plus the
  shared transparent and fixed-size container helpers.
- Define Liquid Glass elevation and material-strength roles — elevation
  shadows plus the frost, tint, saturation, and plate-polarity recipes that
  keep text-bearing bodies legible — preview selection, and identity
  portrait frame roles without owning component state or user content.
- Keep the default appearance consistent across independently packaged chrome
  components.

## Boundaries

The crate depends only on lens and contains no rendering calls, input
handling, retained UI state, or application intents. Generic widget and
layout capabilities belong in lens. Component-specific geometry, state, and
behavior stay in the owning chrome crate.

## Use

Construct a design snapshot and pass it to a theme or material factory:

```rust
let design = tessera_design::Design::dark();
let theme = tessera_design::themes::menu(frame.theme(), &design);
let popover = tessera_design::materials::popover(&design);
let panel_colors = tessera_design::CommandPanelColors::for_scheme(design.scheme);
let preview_shadow = design
    .glass
    .for_role(tessera_design::GlassRole::FloatingPanel);
```

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [Design system decision](../../docs/adr/0046-design-system-crate.md)
- [Product-semantic vocabulary](../../docs/adr/0144-product-semantic-design-vocabulary.md)
- [Color system](../../docs/dev/design/foundations/color.md)
- [Design language](../../docs/dev/design/index.md)
