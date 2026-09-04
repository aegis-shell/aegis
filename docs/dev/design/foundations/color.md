# Color System

Status: **Adopted**, with accent and high-contrast variants still partial.

The color system names product meaning rather than palette position. A
component consumes roles such as application text, critical status, or menu
hover; it does not select a numbered neutral or copy an RGBA literal.

## Ownership

`tessera-design::colors` is the only production source of literal product
colors inside `tessera-design`. `tessera-design::Design` combines those colors
with shape, material, typography, and motion policy into complete dark and
light snapshots.

| Type | Scope |
|------|-------|
| `ProductColors` | Shared application, chrome, interaction, and semantic roles |
| `CommandPanelColors` | Scheme-adaptive solid command-panel hierarchy |
| `SceneColors` | Compositor clear, scrim, and glass-tint roles |
| `DockColors` | Scheme-invariant foreground colors painted inside Dock glass |

Components consume the resolved `Design` or a scoped color type. They do not
construct `lens::Color` values for an adopted product role and do not select
dark or light values themselves.

## Naming grammar

Rust color fields use `snake_case` and the ordered grammar:

```text
[scope_]role[_variant][_state]
```

| Segment | Meaning | Examples |
|---------|---------|----------|
| Scope | Surface or domain when the containing type does not already supply it | `menu`, `application`, `launcher` |
| Role | What the color does | `text`, `surface`, `border`, `accent`, `track` |
| Variant | A stable hierarchy within that role | `heading`, `muted`, `recessed` |
| State | The interaction or availability state; always last | `hover`, `active`, `selected`, `disabled` |

The containing type supplies an implicit scope. For example,
`CommandPanelColors::surface_recessed` does not repeat `command_panel`, while
`ProductColors::application_surface_hover` needs the `application` scope.

### Accepted names

| Name | Interpretation |
|------|----------------|
| `menu_text` | Default menu content tone |
| `menu_text_disabled` | Menu text in the disabled state |
| `menu_surface_hover` | Menu surface feedback in the hover state |
| `application_surface_active` | Application surface feedback while active |
| `launcher_selection_surface` | Surface that carries launcher selection |
| `critical` | Product-wide semantic critical emphasis |

### Rejected names

| Name | Reason |
|------|--------|
| `blue_500` | Encodes pigment and a numbered scale instead of meaning |
| `white_12` | Encodes a literal value and opacity |
| `frosted_white` | Encodes a temporary visual treatment |
| `menu_hover` | Omits whether text, border, or surface changes |
| `selected_blue` | Reverses the grammar and encodes pigment |
| `classic` or an inspiration codename | Describes history or exploration, not a product role |

Type names use `<Domain>Colors` for scoped collections. Appearance names
such as `dark` and `light` belong on constructors, not fields or role names.
The same role exists in every supported appearance even when its value is
unchanged.

## Role families

| Role family | Examples | Rule |
|-------------|----------|------|
| Content | primary text, muted text, disabled text | Pair with the surface role on which the content appears. |
| Surface | application, card, popover, launcher field | Select by hierarchy and material, not desired opacity. |
| Interaction | hover, active, selection, accent | State must remain distinguishable without hue alone. |
| Semantic | critical, validation | Reserve for meaning; do not use as structural decoration. |
| Scene | clear color, scrim, glass tint | Owned by compositor presentation, not widgets. |
| Identity | Dock marks, avatar frames, application icons | Preserve source identity unless a role explicitly recolors it. |

Exact adopted values live in `tessera-design::colors`. The material-role values
used by chrome are also summarized in [Surfaces](../surfaces.md).

## Appearance resolution

Dark and light snapshots retain the same semantic role names. A component
receives the resolved snapshot and does not branch on scheme to choose local
colors. `ColorScheme::System` currently resolves to the dark snapshot.

Desktop settings already carry an optional accent color and normal or high
contrast preference. These values are not yet applied across the complete
`Design` snapshot, so accent customization and a high-contrast palette remain
partial and must not be advertised as complete design variants.

## Usage rules

- Keep product RGBA literals in `tessera-design::colors`. A source-scanning unit
  test rejects literals that escape into another `tessera-design` module.
- Use semantic roles in Tessera-owned UI. Source media, protocol-defined values,
  debug visualization, and application identity colors remain with their
  authoritative owner and are not product color tokens.
- Do not derive hover, disabled, or selected colors at the call site when the
  state is an adopted role. Alpha adjustment is allowed only for a documented
  continuous transition from a named base role.
- Keep text and icons opaque enough to meet the contrast requirement on the
  resolved surface. Transparency is not a substitute for a muted role.
- Use accent for action, status, or identity. Current selection on glass uses
  neutral optical focus, not a blue structural outline.
- Validate both appearances over hostile bright, dark, and high-chroma
  backdrops when a surface is translucent.
- Never infer state from color alone; pair it with text, shape, position, or
  another perceivable cue.

## Adding a color role

1. Name the scope, role, stable variant, and state in that order.
2. Confirm that no existing semantic role expresses the same meaning.
3. Add the field to the narrowest appropriate `*Colors` type.
4. Define every supported appearance in `tessera-design::colors`.
5. Add exact-value and cross-appearance relationship tests.
6. Consume the role through `Design`, a theme, or a material factory.
7. Validate contrast and non-color state cues in every supported appearance.
8. Update this page or the relevant surface specification.

Do not promote a copied literal by giving it a token name. A new role needs a
stable product meaning and an identified consumer boundary.

## Adoption work

- Map desktop accent and high-contrast preferences into semantic snapshots.
- Add automated contrast checks for opaque pairs and reference-image checks
  for adaptive materials.
- Extend literal and token-consumer linting from `tessera-design` to all
  Tessera-owned UI crates without flagging source-owned media values.

See [Accessibility](../guidelines/accessibility.md) for contrast and
non-color requirements. The product-semantic vocabulary boundary is recorded
in [ADR-0144](../../../adr/0144-product-semantic-design-vocabulary.md).
