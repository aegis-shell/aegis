# ADR-0132: aegis-ui Composite Component Library and Chrome Scaffolding

- Status: Accepted
- Date: 2026-08-19
- Extends: [ADR-0021](0021-chrome-component-trait.md), [ADR-0046](0046-design-system-crate.md), [ADR-0080](0080-hud-status-chips-and-sao-command-panel.md), [ADR-0088](0088-agent-runtime-interaction-grants.md), [ADR-0114](0114-panel-hosted-settings-and-hud-command-panel.md)

## Context

[ADR-0046](0046-design-system-crate.md) established `aegis-design` as a data-only design token and material crate, while intentionally deferring a shared widget crate until repeated structure, layout, and interaction patterns appeared across at least three product surfaces.

As the desktop evolved:
1. Modal security prompts ([ADR-0088](0088-agent-runtime-interaction-grants.md), `ConfirmPrompt`, `SecretPrompt`, `BatteryAlert`, `CapabilityPrompt`, `AppPicker`) independently implemented identical scrim placement, centered Liquid Glass panel math, button hover/press states, and four-tier grant action button strips.
2. Settings modules ([ADR-0114](0114-panel-hosted-settings-and-hud-command-panel.md)) across `aegis-settings` and `aegis-command-panel` duplicated card containers, section headers, setting row layouts, and unavailable state placeholders.
3. Chrome surfaces across `aegis-hud`, `aegis-command-panel`, `aegis-shell`, and `aegis-prism` duplicated geometric hit-testing (`contains`), motion curves (`ease_out_cubic`, `stagger`), concentric shapes (`render_disc`, `render_ring`), status chips, and popup menu metrics.

Leaving these implementations isolated inside individual crates caused silent visual drift, duplicate boilerplate, and divergent keyboard/pointer interaction handling.

## Decision

Create `aegis-ui`, an internal workspace crate positioned between `aegis-design` and the compositor chrome crates:

```text
lens (generic immediate-mode UI engine)
  └── aegis-design (pure data tokens & materials)
        └── aegis-ui (composite UI patterns & scaffolding)
              ├── geom (hit-testing, concentric centering, layout options)
              ├── motion (easing curves, stagger choreography, lerp)
              ├── shapes (discs, rings, indicator dots)
              ├── dialog (modal scrims, glass panels, action buttons, grant strips)
              ├── settings (card containers, section headings, row layouts)
              ├── chip (HUD chips, text/glyph outline styles, workspace dots)
              ├── menu (popup menu metrics, panel layouts, item rows)
              └── picker (scroll-window calculations, candidate row layouts)
```

### Architectural Principles

1. **Strict Layering**: `aegis-ui` depends only on `lens`, `aegis-design`, and `aegis-model`. It is completely decoupled from Wayland server runtime state, IPC connection handles, and compositor scheduling.
2. **Safety & Robustness**: Crate root strictly enforces `#![forbid(unsafe_code)]`.
3. **Composability**: Scaffolding functions accept `lens::Frame` and standard closures or geometry descriptors, allowing chrome components to retain domain-specific event dispatch while guaranteeing unified visual constraints and typography.
4. **Gradual Workspace Adoption**: Chrome crates (`aegis-shell`, `aegis-settings`, `aegis-command-panel`, `aegis-hud`, `aegis-prism`) migrate local helpers to delegate to `aegis-ui`.

## Consequences

- **Design Consistency**: Modal dialogs, HUD chips, settings rows, and action buttons adhere to uniform dimensions, radii, padding, and hover states.
- **Code Reduction**: Eliminates hundreds of lines of duplicate hit-testing, layout option construction, and button rendering logic across chrome crates.
- **Testability**: Pure UI calculations, scroll window clamping, and motion curves are directly unit-tested within `aegis-ui`.
