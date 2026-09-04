# Contributor Documentation

Contributor-only documentation for tessera. User-facing material lives under
the other `docs/` sections; see the [documentation index](../index.md).

## Pages

| Page | Purpose |
|------|---------|
| [Setup](setup.md) | Toolchain, dependencies, and build/run |
| [Tessera and Optics Cross-Repository Development](cross-repository-development.md) | Worktree-isolated local patches, commit protection, and release promotion |
| [Nested Backend Development](nested-backend.md) | Successful-build process replacement, scene restoration, and backend validation inside a Wayland session |
| [Surface Testing](surface-testing.md) | Test model and code conventions for shell surfaces: command-panel and lock-screen previews, nested integration, agent pixel capture, and physical validation |
| [First-Party Application Development](first-party-applications.md) | Installation staging, application discovery, and focused test behavior for standalone system applications |
| [Distribution Packaging](packaging.md) | Reproducible build inputs, complete install manifest, package integration, and validation |
| [VT/DRM Manual Testing](vt-drm-testing.md) | Run and try the compositor on real display and input hardware |
| [Project Layout](project-layout.md) | Source tree map and ownership boundaries |
| [Design Language](design/index.md) | System-wide foundations, components, patterns, guidelines, tooling, and product surface specifications |
| [Observability](observability.md) | The tracing-based logging seam, log levels, and `RUST_LOG` workflow |
| [Development Environment Variables](environment-variables.md) | Contributor environment overrides for backend selection, nested UI debugging, asset dumps, and XDG isolation |
| [Issue Triage](triage/index.md) | Case-based bug-attribution know-how: invariants, diagnostic recipes, and fix ownership |
| [Documentation Governance](documentation/index.md) | Rules for writing and routing docs |
