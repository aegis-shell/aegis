# aegis-design

`aegis-design` defines the product-specific visual language shared by aegis
chrome components on top of lens.

## Responsibilities

- Define semantic colors, radii, and stroke widths.
- Build lens themes for shared product surfaces.
- Build data-only material options for popovers, panels, and cards.
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
let design = aegis_design::Design::dark();
let theme = aegis_design::themes::menu(frame.theme(), &design);
let popover = aegis_design::materials::popover(&design);
```

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [Design system decision](../../docs/adr/0046-design-system-crate.md)
