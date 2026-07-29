# Architecture Decision Records

Durable technical decisions for Aegis. Records are numbered, immutable, and
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
| [0011](0011-subsurface-tree-and-z-split-rendering.md) | Subsurface tree model and z-split rendering | Superseded by [0061](0061-window-tree-atomic-client-surface-compositing.md) |
| [0012](0012-toplevel-metadata-and-state-machine.md) | Toplevel metadata and state machine (M3 partial) | Accepted |
| [0013](0013-interactive-move-and-resize.md) | Interactive move and resize | Accepted |
| [0014](0014-buffer-transform-and-viewport-crop.md) | Buffer transform (CPU staging) and viewport crop | Accepted |
| [0015](0015-damage-tracking.md) | Per-commit damage tracking | Accepted |
| [0016](0016-shell-server-window-management-bridge.md) | Shell ↔ server window-management bridge | Accepted |
| [0017](0017-server-side-decorations-via-overlays.md) | Server-side decorations via flux-ui overlays | Superseded by [0063](0063-compositor-owned-borderless-decoration-policy.md) |
| [0018](0018-wallpaper-crate.md) | Wallpaper as an independent crate | Accepted |
| [0019](0019-dock-as-bottom-center-overlay.md) | macOS-style dock via a bottom-center overlay | Accepted |
| [0020](0020-buffer-scale-applied-at-composite.md) | Apply buffer_scale at composite time | Accepted |
| [0021](0021-chrome-component-trait.md) | Chrome component trait (pure core shell) | Accepted |
| [0022](0022-application-launcher.md) | Application launcher via freedesktop.org desktop entries | Accepted |
| [0023](0023-split-flux-lens-stack.md) | Depend on the split flux / lens stack via out-of-tree Rust bindings | Superseded by [0067](0067-remote-optics-dependencies-and-local-overrides.md) |
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
| [0045](0045-statusbar-crate-and-sni-tray.md) | Status bar as a component crate with a host-rendered StatusNotifierItem tray (amends [0021](0021-chrome-component-trait.md)) | Accepted |
| [0046](0046-design-system-crate.md) | Product design system as a data-only crate | Accepted |
| [0047](0047-neenee-agent-realm-platform-bridge.md) | Neenee Agent Realm platform bridge (amends [0031](0031-agent-as-scoped-ipc-client.md)) | Accepted |
| [0048](0048-compositor-owned-agent-operation-feedback.md) | Compositor-owned Agent operation feedback (amends [0040](0040-realms-seats-and-transferable-interaction-authority.md)) | Accepted |
| [0049](0049-standalone-modular-control-center.md) | Standalone modular Control Center with revisioned settings IPC (amends [0044](0044-dock-and-control-center-crates.md)) | Superseded by [0056](0056-system-settings-identity-and-boundary.md) |
| [0050](0050-fuji-agent-product-and-bridge-rename.md) | fuji agent product and the aegis-fuji bridge rename (amends [0047](0047-neenee-agent-realm-platform-bridge.md)) | Accepted |
| [0051](0051-portal-backend-dbus-bridge.md) | xdg-desktop-portal backend as an out-of-process D-Bus bridge | Proposed |
| [0052](0052-scoped-output-frame-streaming.md) | Scoped output frame streaming | Proposed |
| [0053](0053-portal-session-services-and-grants.md) | Portal session services, connection-scoped idle inhibition, and portal-owned grants | Proposed |
| [0054](0054-interactive-target-picking.md) | Interactive target picking and window-scoped stream targets | Proposed |
| [0055](0055-zero-copy-dmabuf-frame-export.md) | Zero-copy dmabuf export for ScreenCast and frame capture | Proposed |
| [0056](0056-system-settings-identity-and-boundary.md) | System Settings identity and Control Center boundary (supersedes [0049](0049-standalone-modular-control-center.md)) | Superseded by [0057](0057-system-settings-canonical-namespace.md) |
| [0057](0057-system-settings-canonical-namespace.md) | System Settings canonical namespace (supersedes [0056](0056-system-settings-identity-and-boundary.md)) | Superseded by [0059](0059-first-party-application-installation-and-development-staging.md) |
| [0058](0058-independent-quick-settings-and-ai-workspaces.md) | Independent Quick Settings and AI Workspaces applications (amends [0044](0044-dock-and-control-center-crates.md)) | Superseded by [0060](0060-statusbar-system-controls-and-live-system-ipc.md) |
| [0059](0059-first-party-application-installation-and-development-staging.md) | First-party application installation and development staging (supersedes [0057](0057-system-settings-canonical-namespace.md)) | Superseded by [0069](0069-documentation-owned-installation-and-throwaway-development-staging.md) |
| [0060](0060-statusbar-system-controls-and-live-system-ipc.md) | Status bar system controls and live-system IPC (supersedes [0058](0058-independent-quick-settings-and-ai-workspaces.md)) | Accepted |
| [0061](0061-window-tree-atomic-client-surface-compositing.md) | Window-tree-atomic client surface compositing (supersedes [0011](0011-subsurface-tree-and-z-split-rendering.md)) | Accepted |
| [0062](0062-wayland-input-method-v2-host-integration.md) | Wayland input-method-v2 host integration | Accepted |
| [0063](0063-compositor-owned-borderless-decoration-policy.md) | Compositor-owned borderless decoration policy | Accepted |
| [0064](0064-output-space-use-and-chrome-policy.md) | Output space-use state and chrome policy | Accepted |
| [0065](0065-compositor-chrome-key-routing-without-focus-churn.md) | Compositor chrome key routing without focus churn | Accepted |
| [0066](0066-canonical-aegis-namespace.md) | Canonical Aegis namespace | Accepted |
| [0067](0067-remote-optics-dependencies-and-local-overrides.md) | Remote Optics dependencies with local development overrides | Superseded by [0071](0071-worktree-isolated-cross-repository-development.md) |
| [0068](0068-cargo-native-development-and-environment-backend-selection.md) | Cargo-native development and environment-only backend selection | Superseded by [0069](0069-documentation-owned-installation-and-throwaway-development-staging.md) |
| [0069](0069-documentation-owned-installation-and-throwaway-development-staging.md) | Documentation-owned installation and throwaway development staging (supersedes [0068](0068-cargo-native-development-and-environment-backend-selection.md)) | Accepted |
| [0070](0070-svg-cursors-with-bundled-bibata-fallback.md) | SVG cursors with a bundled Bibata fallback | Accepted |
| [0071](0071-worktree-isolated-cross-repository-development.md) | Worktree-isolated Aegis and Optics development | Accepted |
| [0072](0072-desktop-preference-authority-and-toolkit-compatibility.md) | Desktop preference authority and toolkit compatibility | Accepted |
| [0073](0073-prism-search-and-explicit-application-shortcuts.md) | Prism search and explicit application shortcuts (amends [0022](0022-application-launcher.md), [0044](0044-dock-and-control-center-crates.md)) | Accepted |
| [0074](0074-generic-agent-workspaces-status-surface.md) | Generic Agent Workspaces status surface (amends [0050](0050-fuji-agent-product-and-bridge-rename.md), [0060](0060-statusbar-system-controls-and-live-system-ipc.md)) | Accepted |
