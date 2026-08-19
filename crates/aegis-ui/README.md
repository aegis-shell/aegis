# aegis-ui

`aegis-ui` provides high-level composite UI patterns, modal dialog scaffolding, settings controls, and shared layout primitives for Aegis compositor chrome.

## Responsibilities

- **Layout & Geometry Primitives (`geom`)**: Common rectangle hit-testing, concentric centering, edge insets clamping, and standardized `LayoutOpts` helpers.
- **Modal & Dialog Scaffolding (`dialog`)**: Standardized modal layouts, full-display scrim placement, glass panel anchoring, action button bars (Cancel/Confirm, ADR-0088 runtime grant persistences), and keyboard trap contracts.
- **Settings & Controls (`settings`)**: Standardized settings cards, section headings, form rows, unavailable rows, and control wrappers.
- **Status Chips & Pills (`chip`)**: Standardized HUD and panel pills, badges, and metric chips with consistent radii and alpha attenuation.
- **Selection & List Items (`list`)**: Selectable rows, preview selection styles, and keyboard-focused list scaffolds.

## Layering Boundary

```text
lens (low-level UI engine)
  └── aegis-design (data-only design tokens & materials)
        └── aegis-ui (composite UI patterns & widgets)
              └── aegis-shell / aegis-command-panel / aegis-settings / aegis-hud / aegis-lock
```

`aegis-ui` depends only on `lens`, `aegis-design`, and `aegis-model`. It contains no compositor-specific server state (`Window`, `InteractionDomain`, `Server`) and does not depend on `aegis-shell` or `aegis-compositor`.
