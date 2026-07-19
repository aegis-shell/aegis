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
| [0005](0005-flux-core-binding-crate-in-flux-repo.md) | flux core binding crate in the flux repo | Superseded by [0023](0023-split-flux-lens-stack.md) |
| [0006](0006-ffi-soundness-discipline.md) | FFI soundness discipline for hand-rolled protocol handlers | Accepted |
| [0007](0007-logging-and-backend-input-contract.md) | Logging facade and the `Backend` input contract | Accepted |
| [0009](0009-input-pipeline-and-pointer-focus.md) | Input pipeline and pointer focus model | Superseded by [0040](0040-realms-seats-and-transferable-interaction-authority.md) |
| [0010](0010-keyboard-pipeline-and-xkbcommon-ownership.md) | Keyboard pipeline and xkbcommon ownership | Superseded by [0040](0040-realms-seats-and-transferable-interaction-authority.md) |
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
| [0022](0022-application-launcher.md) | Application launcher via freedesktop.org desktop entries | Accepted |
| [0023](0023-split-flux-lens-stack.md) | Depend on the split flux / lens stack via out-of-tree Rust bindings | Accepted |
| [0024](0024-layout-model.md) | Layout model — floating base with an optional tiling policy | Accepted |
| [0025](0025-workspace-model.md) | Workspace model — dynamic and per-output | Accepted |
| [0026](0026-configuration-system.md) | Configuration system — one declarative file with live reload | Accepted |
| [0027](0027-ipc-and-introspection.md) | IPC and introspection seam | Accepted |
| [0028](0028-output-and-monitor-model.md) | Output and monitor model | Accepted |
| [0029](0029-animation-and-effect-policy.md) | Animation and effect policy | Accepted |
| [0030](0030-xwayland-strategy.md) | XWayland strategy | Accepted |
| [0031](0031-agent-as-scoped-ipc-client.md) | The agent as a scoped IPC client (M10 framing) | Accepted |
| [0032](0032-durable-window-identifiers.md) | Durable window identifiers | Accepted |
| [0033](0033-mutation-journal.md) | The mutation journal | Accepted |
| [0034](0034-scoped-capabilities.md) | Scoped capabilities | Superseded by [0035](0035-fail-closed-named-ipc-scopes.md) |
| [0035](0035-fail-closed-named-ipc-scopes.md) | Fail-closed named IPC scope resolution | Accepted |
| [0036](0036-scoped-semantic-automation.md) | Scoped semantic geometry and target-local input | Accepted |
| [0037](0037-scoped-pixel-capture-over-ipc.md) | Scoped pixel capture over the IPC | Superseded by [0041](0041-sealed-file-descriptor-pixel-transport.md) |
| [0038](0038-frame-pacing.md) | Frame pacing — event-driven loop with presentation throttling | Accepted |
| [0039](0039-damage-driven-shm-refresh.md) | Damage-driven shm snapshot and texture refresh (amends [0015](0015-damage-tracking.md)) | Accepted |
| [0040](0040-realms-seats-and-transferable-interaction-authority.md) | Realms, seats, and transferable interaction authority | Accepted |
| [0041](0041-sealed-file-descriptor-pixel-transport.md) | Sealed file-descriptor pixel transport | Accepted |
| [0042](0042-mount-scoped-realm-portals-and-cgroup-sandboxes.md) | Mount-scoped Realm portals and cgroup sandboxes | Accepted |
| [0043](0043-explicit-clipboard-only.md) | Explicit clipboard only; reject Primary Selection | Accepted |
| [0044](0044-dock-and-control-center-crates.md) | Dock and Control Center as component crates (amends [0021](0021-chrome-component-trait.md)) | Accepted |
