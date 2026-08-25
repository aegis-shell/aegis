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
| [0038](0038-frame-pacing.md) | Frame pacing — event-driven loop with presentation throttling | Superseded by [0077](0077-presentation-domain-redraw-state-machine.md) |
| [0039](0039-damage-driven-shm-refresh.md) | Damage-driven shm snapshot and texture refresh (amends [0015](0015-damage-tracking.md)) | Accepted |
| [0040](0040-realms-seats-and-transferable-interaction-authority.md) | Realms, seats, and transferable interaction authority | Superseded by [0103](0103-actor-authority-and-interaction-domain-architecture.md) |
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
| [0051](0051-portal-backend-dbus-bridge.md) | xdg-desktop-portal backend as an out-of-process D-Bus bridge | Superseded by [0075](0075-independent-portal-package-and-backend-contract.md) |
| [0052](0052-scoped-output-frame-streaming.md) | Scoped output frame streaming (pacing superseded by [0126](0126-damage-driven-stream-pacing-and-geometry-renegotiation.md)) | Accepted (implemented; dmabuf transport per [0124](0124-window-open-close-fade-transitions.md) audit) |
| [0053](0053-portal-session-services-and-grants.md) | Portal backend ABI ownership and scoped session services | Superseded by [0075](0075-independent-portal-package-and-backend-contract.md) |
| [0054](0054-interactive-target-picking.md) | Interactive target picking and window-scoped stream targets | Proposed |
| [0055](0055-zero-copy-dmabuf-frame-export.md) | Zero-copy dmabuf export for ScreenCast and frame capture | Superseded by the portal slot-ring design (portal ADR-0005/0006); shipped as IPC protocol 25 |
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
| [0070](0070-svg-cursors-with-bundled-bibata-fallback.md) | SVG cursors with a bundled Bibata fallback | Superseded by [0122](0122-original-mit-cursor-theme-replaces-bibata.md) |
| [0071](0071-worktree-isolated-cross-repository-development.md) | Worktree-isolated Aegis and Optics development | Accepted |
| [0072](0072-desktop-preference-authority-and-toolkit-compatibility.md) | Desktop preference authority and toolkit compatibility | Accepted |
| [0073](0073-prism-search-and-explicit-application-shortcuts.md) | Prism search and explicit application shortcuts (amends [0022](0022-application-launcher.md), [0044](0044-dock-and-control-center-crates.md)) | Accepted |
| [0074](0074-generic-agent-workspaces-status-surface.md) | Generic Agent Workspaces status surface (amends [0050](0050-fuji-agent-product-and-bridge-rename.md), [0060](0060-statusbar-system-controls-and-live-system-ipc.md)) | Superseded by [0103](0103-actor-authority-and-interaction-domain-architecture.md) |
| [0075](0075-independent-portal-package-and-backend-contract.md) | Independent portal package and backend contract (supersedes [0051](0051-portal-backend-dbus-bridge.md), [0053](0053-portal-session-services-and-grants.md)) | Superseded by [0095](0095-independent-portal-repository-and-component-workspace.md) |
| [0076](0076-linux-dmabuf-device-feedback-and-reusable-buffer-sync.md) | Linux-dmabuf device feedback and reusable-buffer synchronization | Accepted |
| [0077](0077-presentation-domain-redraw-state-machine.md) | Presentation-domain redraw state machine (supersedes [0038](0038-frame-pacing.md)) | Accepted |
| [0078](0078-out-of-process-idle-and-session-lock.md) | Out-of-process idle policy and session lock | Accepted |
| [0079](0079-tracing-based-observability-seam.md) | Tracing-based observability seam | Accepted |
| [0080](0080-hud-status-chips-and-sao-command-panel.md) | HUD status chips and the SAO command panel | Accepted |
| [0080](0080-avatar-crate-xdg-conformant-vrm-aware.md) | Avatar as an independent crate, XDG-conformant, VRM-aware | Superseded by [0106](0106-shared-identity-portrait-contract-and-vrm-renderer-boundary.md) |
| [0081](0081-hud-and-command-panel-naming.md) | HUD and command panel naming (amends [0045](0045-statusbar-crate-and-sni-tray.md), [0080](0080-hud-status-chips-and-sao-command-panel.md)) | Accepted |
| [0082](0082-configurable-touchpad-swipe-bindings.md) | Configurable touchpad swipe bindings (amends [0080](0080-hud-status-chips-and-sao-command-panel.md)) | Accepted |
| [0083](0083-frameless-transient-toasts-and-hud-consolidation.md) | Frameless transient toasts and HUD consolidation (amends [0080](0080-hud-status-chips-and-sao-command-panel.md)) | Accepted |
| [0084](0084-session-scoped-always-on-top-window-band.md) | Session-scoped always-on-top window band (amends [0027](0027-ipc-and-introspection.md)) | Accepted |
| [0085](0085-portal-secret-absorption-and-secret-service-compat.md) | Portal secret absorption: vault, Secret backend, and a transitional Secret Service compat layer | Superseded by [0112](0112-native-portal-secret-with-portal-owned-prompts.md) |
| [0086](0086-full-stack-portal-via-user-consent-pick-chains.md) | Full-stack portal via user-consent pick chains | Superseded by [0099](0099-resource-authority-and-out-of-process-file-chooser.md) |
| [0087](0087-aegis-mcp-standalone-platform-bridge-crate.md) | aegis-mcp as the standalone platform bridge crate (amends [0050](0050-fuji-agent-product-and-bridge-rename.md), [0066](0066-canonical-aegis-namespace.md)) | Accepted |
| [0088](0088-agent-capability-borrowing-and-runtime-grants.md) | Agent capability borrowing and runtime grants (amends [0031](0031-agent-as-scoped-ipc-client.md), [0034](0034-scoped-capabilities.md), [0047](0047-neenee-agent-realm-platform-bridge.md), [0087](0087-aegis-mcp-standalone-platform-bridge-crate.md)) | Superseded by [0090](0090-native-capability-broker-and-stateless-mcp-edge.md) |
| [0089](0089-aegis-agent-product-and-fuji-identity-rename.md) | aegis-agent Crate, CLI, and Fuji Identity Preservation (amends [0050](0050-fuji-agent-product-and-bridge-rename.md), [0066](0066-canonical-aegis-namespace.md)) | Accepted |
| [0090](0090-native-capability-broker-and-stateless-mcp-edge.md) | Native capability broker and stateless MCP edge (supersedes [0088](0088-agent-capability-borrowing-and-runtime-grants.md)) | Accepted |
| [0091](0091-agent-controlled-window-physical-mirrors-and-guard.md) | Agent-controlled window physical mirrors and guard (amends [0040](0040-realms-seats-and-transferable-interaction-authority.md), [0048](0048-compositor-owned-agent-operation-feedback.md)) | Accepted |
| [0092](0092-explicit-wallpaper-modes-and-continuous-parallax.md) | Explicit wallpaper modes and continuous pointer parallax (amends [0018](0018-wallpaper-crate.md)) | Accepted |
| [0093](0093-unified-domain-oriented-aegis-command-surface.md) | Unified domain-oriented Aegis command surface (amends [0027](0027-ipc-and-introspection.md), [0066](0066-canonical-aegis-namespace.md)) | Accepted |
| [0094](0094-liquid-glass-lens-model-and-full-resolution-capture.md) | Liquid glass lens model and full-resolution backdrop capture (amends [0046](0046-design-system-crate.md)) | Accepted |
| [0095](0095-independent-portal-repository-and-component-workspace.md) | Independent portal repository and component workspace (supersedes [0075](0075-independent-portal-package-and-backend-contract.md), preserves [0085](0085-portal-secret-absorption-and-secret-service-compat.md)) | Superseded by [0099](0099-resource-authority-and-out-of-process-file-chooser.md) |
| [0096](0096-avatar-motion-library-and-semantic-playback.md) | Avatar motion library and semantic playback (amends [0080](0080-avatar-crate-xdg-conformant-vrm-aware.md)) | Accepted |
| [0097](0097-transactional-avatar-hot-reload.md) | Transactional avatar hot reload (amends [0080](0080-avatar-crate-xdg-conformant-vrm-aware.md), [0096](0096-avatar-motion-library-and-semantic-playback.md)) | Accepted |
| [0098](0098-textured-vrm-materials.md) | Textured VRM materials (amends [0080](0080-avatar-crate-xdg-conformant-vrm-aware.md), preserves [0097](0097-transactional-avatar-hot-reload.md)) | Accepted |
| [0099](0099-resource-authority-and-out-of-process-file-chooser.md) | Resource authority and out-of-process FileChooser (supersedes [0086](0086-full-stack-portal-via-user-consent-pick-chains.md), [0095](0095-independent-portal-repository-and-component-workspace.md)) | Accepted |
| [0100](0100-strict-kms-vulkan-device-affinity.md) | Strict KMS and Vulkan physical-device affinity (amends [0076](0076-linux-dmabuf-device-feedback-and-reusable-buffer-sync.md)) | Accepted |
| [0101](0101-dual-presentation-paths-and-conservative-kms-plane-allocation.md) | Dual presentation paths and conservative KMS plane allocation (amends [0076](0076-linux-dmabuf-device-feedback-and-reusable-buffer-sync.md), [0077](0077-presentation-domain-redraw-state-machine.md)) | Accepted |
| [0102](0102-actor-scoped-semantic-observation-and-transactional-actions.md) | Actor-scoped semantic observation and transactional actions (amends [0033](0033-mutation-journal.md), [0036](0036-scoped-semantic-automation.md), [0040](0040-realms-seats-and-transferable-interaction-authority.md), [0090](0090-native-capability-broker-and-stateless-mcp-edge.md)) | Accepted |
| [0103](0103-actor-authority-and-interaction-domain-architecture.md) | Actor authority kernel and Interaction Domain architecture (supersedes [0040](0040-realms-seats-and-transferable-interaction-authority.md), [0074](0074-generic-agent-workspaces-status-surface.md); amends [0090](0090-native-capability-broker-and-stateless-mcp-edge.md), [0093](0093-unified-domain-oriented-aegis-command-surface.md), [0102](0102-actor-scoped-semantic-observation-and-transactional-actions.md)) | Accepted |
| [0104](0104-actor-sessions-resource-grants-and-accessibility-adapter.md) | Actor sessions, exact resource grants, and accessibility adapter (amends [0090](0090-native-capability-broker-and-stateless-mcp-edge.md), [0099](0099-resource-authority-and-out-of-process-file-chooser.md), [0102](0102-actor-scoped-semantic-observation-and-transactional-actions.md), [0103](0103-actor-authority-and-interaction-domain-architecture.md)) | Accepted |
| [0105](0105-single-body-liquid-glass-interaction-focus.md) | Single-body Liquid Glass interaction focus (amends [0046](0046-design-system-crate.md), [0094](0094-liquid-glass-lens-model-and-full-resolution-capture.md)) | Accepted |
| [0106](0106-shared-identity-portrait-contract-and-vrm-renderer-boundary.md) | Shared identity portrait contract and VRM renderer boundary | Accepted |
| [0107](0107-rounded-color-preserving-glass-previews.md) | Rounded, color-preserving glass previews (amends [0105](0105-single-body-liquid-glass-interaction-focus.md)) | Accepted |
| [0108](0108-agent-workspaces-presentation-naming.md) | Agent Workspaces presentation naming (amends [0103](0103-actor-authority-and-interaction-domain-architecture.md)) | Superseded by [0113](0113-platform-ai-backend-and-agent-product-removal.md) |
| [0109](0109-module-first-security-and-presentation-identity-boundaries.md) | Module-first security and presentation-identity boundaries (amends [0103](0103-actor-authority-and-interaction-domain-architecture.md), [0104](0104-actor-sessions-resource-grants-and-accessibility-adapter.md), [0106](0106-shared-identity-portrait-contract-and-vrm-renderer-boundary.md)) | Accepted |
| [0110](0110-shared-model-crate-naming-and-placement.md) | Shared model crate naming and placement (amends [0001](0001-scope-and-responsibility-boundary.md), [0103](0103-actor-authority-and-interaction-domain-architecture.md), [0109](0109-module-first-security-and-presentation-identity-boundaries.md)) | Accepted |
| [0111](0111-persona-as-shell-domain-with-feature-gated-portrait-runtime.md) | Persona as a shell domain with a feature-gated portrait runtime (amends [0106](0106-shared-identity-portrait-contract-and-vrm-renderer-boundary.md), [0109](0109-module-first-security-and-presentation-identity-boundaries.md)) | Accepted |
| [0112](0112-native-portal-secret-with-portal-owned-prompts.md) | Native portal Secret with a hardened vault and Portal-owned prompts (supersedes [0085](0085-portal-secret-absorption-and-secret-service-compat.md)) | Accepted |
| [0113](0113-platform-ai-backend-and-agent-product-removal.md) | Platform AI backend and agent product removal (supersedes [0108](0108-agent-workspaces-presentation-naming.md); amends [0103](0103-actor-authority-and-interaction-domain-architecture.md)) | Accepted |
| [0114](0114-panel-hosted-settings-and-hud-command-panel.md) | Panel-hosted settings and the HUD command panel (supersedes the standalone-application boundary of [0049](0049-standalone-modular-control-center.md) and [0056](0056-system-settings-identity-and-boundary.md), partially supersedes [0060](0060-statusbar-system-controls-and-live-system-ipc.md); amends [0080](0080-hud-status-chips-and-sao-command-panel.md), [0083](0083-frameless-transient-toasts-and-hud-consolidation.md)) | Accepted |
| [0115](0115-command-panel-desktop-behavior-scope.md) | Command panel scope — desktop-computer behavior, not compositor internals (amends [0080](0080-hud-status-chips-and-sao-command-panel.md), [0114](0114-panel-hosted-settings-and-hud-command-panel.md)) | Accepted |
| [0116](0116-overview-gesture-top-rail-and-spatial-slots.md) | Overview gesture, top workspace rail, and spatial slot assignment (amends [0080](0080-hud-status-chips-and-sao-command-panel.md), [0082](0082-configurable-touchpad-swipe-bindings.md)) | Accepted |
| [0117](0117-per-window-content-capture.md) | Per-window content capture (amends [0090](0090-native-capability-broker-and-stateless-mcp-edge.md), [0102](0102-actor-scoped-semantic-observation-and-transactional-actions.md)) | Accepted |
| [0118](0118-launch-placement-and-workspace-isolation.md) | Launch placement and workspace isolation (amends [0025](0025-workspace-model.md), [0027](0027-ipc-and-introspection.md), [0090](0090-native-capability-broker-and-stateless-mcp-edge.md)) | Accepted |
| [0119](0119-four-finger-command-panel-default-restored.md) | Four-finger command panel default restored (amends [0116](0116-overview-gesture-top-rail-and-spatial-slots.md)) | Accepted |
| [0120](0120-glass-material-roles-and-region-level-backdrop-adaptation.md) | Glass material roles and region-level backdrop adaptation (amends [0046](0046-design-system-crate.md), [0094](0094-liquid-glass-lens-model-and-full-resolution-capture.md)) | Accepted |
| [0121](0121-neutral-mask-feedback-movable-mirrors-and-non-raising-agent-input.md) | Neutral mask feedback, movable mirrors, and non-raising Agent input (amends [0048](0048-compositor-owned-agent-operation-feedback.md), [0091](0091-agent-controlled-window-physical-mirrors-and-guard.md)) | Accepted |
| [0122](0122-original-mit-cursor-theme-replaces-bibata.md) | Original MIT cursor theme replaces the bundled Bibata fallback (supersedes [0070](0070-svg-cursors-with-bundled-bibata-fallback.md)) | Accepted |
| [0123](0123-chrome-fades-via-lens-opacity.md) | Chrome enter/exit fades go through lens opacity | Accepted |
| [0124](0124-window-open-close-fade-transitions.md) | Window open/close fade transitions (extends [0029](0029-animation-and-effect-policy.md)) | Accepted |
| [0125](0125-ipc-primitive-families-and-shared-ipc-client.md) | IPC primitive families and the shared IPC client library (amends [0027](0027-ipc-and-introspection.md), [0090](0090-native-capability-broker-and-stateless-mcp-edge.md)) | Accepted |
| [0126](0126-damage-driven-stream-pacing-and-geometry-renegotiation.md) | Damage-driven stream pacing and stream renegotiation (supersedes the forced-cadence pacing of [0052](0052-scoped-output-frame-streaming.md); amends [0052](0052-scoped-output-frame-streaming.md), [0054](0054-interactive-target-picking.md)) | Accepted (Decision 1 superseded by [0130](0130-stream-paced-presentation-and-scanout-exclusion.md)) |
| [0127](0127-occlusion-safe-window-streams-and-cursor-compositing.md) | Occlusion-safe window streams and stream cursor compositing (amends [0052](0052-scoped-output-frame-streaming.md), [0054](0054-interactive-target-picking.md), [0126](0126-damage-driven-stream-pacing-and-geometry-renegotiation.md)) | Accepted |
| [0128](0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md) | Peer-identity-bound built-in scopes and the capture indicator | Accepted |
| [0129](0129-color-management.md) | Color management — wp_color_management_v1 server, uniform framebuffer encoding, KMS HDR signaling | Accepted |
| [0130](0130-stream-paced-presentation-and-scanout-exclusion.md) | Stream-paced presentation and scanout exclusion for output streams (supersedes Decision 1 of [0126](0126-damage-driven-stream-pacing-and-geometry-renegotiation.md)) | Accepted |
| [0131](0131-placement-stagger-for-colliding-origins.md) | Session-scoped placement stagger for colliding origins (extends [0012](0012-toplevel-metadata-and-state-machine.md) placement, follows the session/durable boundary of [0032](0032-durable-window-identifiers.md)) | Accepted |
| [0132](0132-aegis-ui-composite-component-library.md) | `aegis-ui` composite component library and chrome scaffolding (extends [0021](0021-chrome-component-trait.md), [0046](0046-design-system-crate.md), [0080](0080-hud-status-chips-and-sao-command-panel.md), [0088](0088-agent-runtime-interaction-grants.md), [0114](0114-panel-hosted-settings-and-hud-command-panel.md)) | Accepted |
| [0133](0133-first-map-focus-and-ext-data-control.md) | First-map focus policy (dialogs and app launches) and ext-data-control-v1 clipboard managers (amends the focus-stealing prevention introduced in [0131](0131-placement-stagger-for-colliding-origins.md)'s release) | Accepted |
| [0134](0134-compositor-driven-fullscreen-for-any-toplevel.md) | Compositor-driven fullscreen for any toplevel (extends [0012](0012-toplevel-metadata-and-state-machine.md) fullscreen state, [0064](0064-output-space-use-and-chrome-policy.md) chrome policy; amends [0027](0027-ipc-and-introspection.md) with protocol 30) | Accepted |
| [0135](0135-routine-capability-polling-is-not-durably-audited.md) | Routine capability polling is not durably audited (amends the audit vocabulary of [0104](0104-actor-sessions-resource-grants-and-accessibility-adapter.md)) | Accepted |
| [0136](0136-authenticated-bounded-audit-replay-and-storage-guards.md) | Authenticated bounded audit replay and storage guards (amends [0104](0104-actor-sessions-resource-grants-and-accessibility-adapter.md)) | Accepted |
| [0137](0137-audit-segment-manifest-and-retention.md) | Audit segment manifest and explicit retention (amends [0104](0104-actor-sessions-resource-grants-and-accessibility-adapter.md), [0136](0136-authenticated-bounded-audit-replay-and-storage-guards.md)) | Accepted |
| [0138](0138-the-input-settings-domain.md) | The Input settings domain (amends [0049](0049-standalone-modular-control-center.md), [0056](0056-system-settings-identity-and-boundary.md), [0010](0010-keyboard-pipeline-and-xkbcommon-ownership.md); IPC protocol 31) | Accepted |
| [0139](0139-animation-effect-placement.md) | Animation effect placement — Optics mechanism, Aegis policy (amends [0029](0029-animation-and-effect-policy.md)) | Accepted |
| [0140](0140-session-power-modes.md) | Session power modes over the staged idle pipeline (amends [0078](0078-out-of-process-idle-and-session-lock.md)) | Accepted |
| [0141](0141-locker-broadcasts-the-logind-session-lock-boundary.md) | The locker broadcasts the logind session-lock boundary | Accepted |
| [0142](0142-layered-glass-backdrop-compositor.md) | Layered glass — the backdrop compositor owns the frost→glass nesting (amends [0094](0094-liquid-glass-lens-model-and-full-resolution-capture.md)) | Accepted |
