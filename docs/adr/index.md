# Architecture Decision Records

Durable technical decisions for ass. Records are numbered, immutable, and
append-only: supersede an accepted record with a new one rather than
editing it. New records start from [the template](template.md). For
background and how the decisions fit together, see
[Architecture](../explanation/architecture.md).

| ADR | Title | Status |
|-----|-------|--------|
| [0001](0001-scope-and-responsibility-boundary.md) | Scope and responsibility boundary | Accepted |
| [0002](0002-hand-rolled-wayland-server.md) | Hand-rolled Wayland server on raw libwayland | Accepted |
| [0003](0003-nested-first-bring-up.md) | Nested-first bring-up, DRM/KMS later | Accepted |
| [0004](0004-client-buffers-via-flux-dmabuf-import.md) | Client buffers via flux dmabuf import | Accepted |
| [0005](0005-flux-core-binding-crate-in-flux-repo.md) | flux core binding crate in the flux repo | Accepted |
| [0006](0006-ffi-soundness-discipline.md) | FFI soundness discipline for hand-rolled protocol handlers | Accepted |
| [0007](0007-logging-and-backend-input-contract.md) | Logging facade and the `Backend` input contract | Accepted |
| [0009](0009-input-pipeline-and-pointer-focus.md) | Input pipeline and pointer focus model | Accepted |
| [0010](0010-keyboard-pipeline-and-xkbcommon-ownership.md) | Keyboard pipeline and xkbcommon ownership | Accepted |
| [0011](0011-subsurface-tree-and-z-split-rendering.md) | Subsurface tree model and z-split rendering | Accepted |
| [0012](0012-toplevel-metadata-and-state-machine.md) | Toplevel metadata and state machine (M3 partial) | Accepted |
| [0013](0013-interactive-move-and-resize.md) | Interactive move and resize | Accepted |
| [0014](0014-buffer-transform-and-viewport-crop.md) | Buffer transform (CPU staging) and viewport crop | Accepted |
| [0015](0015-damage-tracking.md) | Per-commit damage tracking | Accepted |
| [0016](0016-shell-server-window-management-bridge.md) | Shell ↔ server window-management bridge | Accepted |
| [0017](0017-server-side-decorations-via-overlays.md) | Server-side decorations via flux-ui overlays | Accepted |
| [0018](0018-wallpaper-crate.md) | Wallpaper as an independent crate | Accepted |
| [0019](0019-dock-as-bottom-center-overlay.md) | macOS-style dock via a bottom-center overlay | Accepted |
| [0020](0020-buffer-scale-applied-at-composite.md) | Apply buffer_scale at composite time | Accepted |
| [0021](0021-chrome-component-trait.md) | Chrome component trait (pure core shell) | Accepted |
