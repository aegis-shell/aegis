# ADR-0023: Depend on the split flux / lens stack via out-of-tree Rust bindings

- Status: Superseded by ADR-0067
- Date: 2026-06-23

## Context

ass historically consumed the rendering and UI layers from a single sibling
`flux` monorepo: `libflux` under `core/`, `libflux-ui` under `ui/`, each
shipping in-tree Rust bindings under `bindings/rust/`. ass's workspace
referenced them as path dependencies (`flux`, `flux-sys`, `flux-ui`,
`flux-ui-sys`), and the terminal binary re-emitted rpaths the `-sys` crates
published so it found `libflux.so` and `libflux-ui.so` in the meson build
trees.

For v0.1 that monorepo was split into focused, separately-versioned libraries
living as siblings under `../optics`:

- `flux` — the C rendering engine (`libflux`; `flux-text` merged in as the
  `text` module).
- `lens` — the C immediate-mode UI engine (`liblens`), the successor to
  `flux-ui`. Its public symbols are `lens_*` and its umbrella header is
  `<lens/lens.h>`, but it draws through flux's canvas and reuses flux's
  `flux_device` / `flux_canvas` / `flux_result` types at the ABI level.
- `flux-rs` — out-of-tree Rust bindings to `libflux` (`flux` / `flux-sys`),
  following the openssl-sys / rusqlite convention.
- `lens-rs` — out-of-tree Rust bindings to `liblens` (`lens` / `lens-sys`).

The in-tree `flux-ui` binding no longer exists, so ass cannot build against
its previous dependency graph. This also overturns
[ADR-0005](0005-flux-core-binding-crate-in-flux-repo.md), which placed the
flux Rust bindings inside the flux C source tree.

## Decision

ass consumes the split stack:

- `flux` / `flux-sys` from `../optics/flux-rs/crates/{flux,flux-sys}`.
- `lens` / `lens-sys` from `../optics/lens-rs/crates/{lens,lens-sys}` (the
  successor to the old `flux-ui` / `flux-ui-sys`).

`aegis-shell` migrates from the `flux-ui` API to the `lens` API. The migration
is a near-drop-in rename: lens's safe surface (`Ui`, `Frame`, `Input`,
overlays, `Color` / `Rect` / `Icon` / `OverlayOpts`) matches the surface
`aegis-shell` used, and `lens-sys`'s bindgen allowlist covers the `flux_*` types
(`flux_device`, `flux_canvas`, `flux_result`) that `aegis-shell` casts across at
the device-binding seam, so no API drift needed correction.

The terminal binary keeps re-emitting the rpaths the `-sys` crates
publish, now keyed on `DEP_FLUX_RPATHS` and `DEP_LENS_RPATHS` (the `links`
keys are `flux` and `lens`), so the binary resolves `libflux.so` and
`liblens.so` from the meson build trees without `LD_LIBRARY_PATH`. `scripts/
env.sh` exports the dev-mode variables (`FLUX_BUILD_DIR`, `FLUX_SOURCE_DIR`,
`LENS_BUILD_DIR`, `LENS_SOURCE_DIR`) the `-sys` build scripts use to locate
the freshly-built libraries via each tree's `meson-uninstalled/*.pc`.

[ADR-0005](0005-flux-core-binding-crate-in-flux-repo.md) is superseded by this
record.

## Alternatives

- **Build on iris instead of lens.** iris is the application toolkit above
  flux and lens: `iris_app_run` owns the Wayland window, the GPU device, and
  the event loop. ass is itself a compositor and must own the Wayland server,
  swapchain, input pipeline, and main loop, so iris and ass would contend for
  the same responsibilities. lens is the right layer — it only draws UI into a
  caller-owned canvas. Rejected.
- **Keep the in-tree `flux-ui` binding.** No longer possible: the binding was
  removed when the monorepo split. Rejected.
- **Write lens bindings in-tree under ass.** Duplicates the maintained
  `lens-rs` and breaks the one-library-one-binding-repo convention. Rejected.

## Consequences

- ass builds and runs against the current flux / lens / flux-rs / lens-rs
  sources under `../optics`, verified end-to-end: Vulkan device creation,
  nested `VkSurfaceKHR`, Wayland server listening, and lens-rendered chrome on
  the first frame.
- The dev build requires the four env vars above until flux and lens are
  `meson install`-ed into a prefix on `PKG_CONFIG_PATH` (then source
  `scripts/env.sh` with `ASS_DEV_ENV_USE_INSTALLED=1`).
- References to `flux-ui` in ADR-0001 through ADR-0022 are historical: at the
  time those records were written the UI library was named `flux-ui`; it is
  now `lens`. The decisions themselves (chrome overlays, decorations, dock,
  launcher, chrome trait) are unchanged.
- A latent bug surfaced once the project compiled end-to-end for the first
  time: `aegis-shell`'s launcher `emit` helper had been placed inside the
  `impl Chrome for Launcher` block. It is moved to the inherent `impl
  Launcher` block.
