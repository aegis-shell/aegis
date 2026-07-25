# ADR-0045: Status bar as a component crate with a host-rendered StatusNotifierItem tray

- Status: Accepted
- Date: 2026-07-21

## Context

[ADR-0021](0021-chrome-component-trait.md) split `aegis-shell` into a core
host and pluggable `Chrome` components and deferred one crate per
component until a component gained "a dependency or lifecycle the core
should not own". [ADR-0044](0044-dock-and-control-center-crates.md) later
promoted the dock and the Control Center on that trigger. The session
HUD (`HudBar`, in `aegis-shell/src/chrome/workspace_bar.rs`) was the last
remaining top-level chrome surface still living in the core: a ~1060
line component that combined workspace state, the active-window title,
the clock, an application tray of open windows, system status, and a
notification panel.

Two forces now meet the ADR-0021 promotion trigger for this component:

- The bar should host a real system tray. Modern applications publish
  tray icons through the freedesktop [StatusNotifierItem][sni] (SNI)
  protocol on the session D-Bus, with context menus through
  [`com.canonical.dbusmenu`][dbusmenu]. ass had no D-Bus dependency
  anywhere in the workspace and no async runtime; pulling session-bus
  I/O and a `zbus` dependency into `aegis-shell` would force every
  transitive consumer of the contract crate to inherit it.
- The bar is optional. ass runs without it (a tiling WM with only the
  dock and the launcher), and which chrome a session wants is a
  composition-root decision, not a contract decision. There was no
  per-component enable pattern in the configuration until now.

[sni]: https://www.freedesktop.org/wiki/Specifications/StatusNotifierItem/
[dbusmenu]: https://specifications.freedesktop.org/dbus-menu/

A crate boundary is not a process boundary. This decision concerns code
organization only; the bar keeps rendering in the compositor process.

## Decision

### 1. Promote the status bar to its own crate, `aegis-statusbar`

New crate `aegis-statusbar`, depending on the `aegis-shell` contract
(`Chrome`, `ChromeEvents`, `Reserved`, `BackdropRegion`, `IconSet`,
`AppCatalog`, `SystemStatus`, `Localizer`) and registered by the `ass`
binary, exactly mirroring `aegis-dock` and `aegis-ctl-center`. The
component type is renamed `HudBar` → `StatusBar`; the deprecated
`WorkspaceBar` alias is dropped (it had two stale doc references, both
updated). The dependency direction remains
`aegis-core` ← `aegis-shell` ← {component crates} ← `ass`; `aegis-shell`
depends on no component crate, and `ass` remains the composition root.

The hidden coupling that forced a contract change before the split:
`aegis-shell::chrome::toast::Toast` read the bar's `pub(crate)
HUD_HEIGHT` constant for its top margin. `HUD_HEIGHT` is promoted to a
`pub const` at the `aegis-shell` crate root; `Toast` and `StatusBar`
both consume `ass_shell::HUD_HEIGHT`. The constant stays in the
contract crate because it is a layout invariant the whole chrome
surface agrees on, not a private detail of the bar.

### 2. Introduce the first per-component configuration flag

`aegis-config` gains a `[statusbar]` table with one field:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `true` | Whether the top status bar is registered at startup. |

The `ass` binary registers the bar conditionally on
`config.statusbar.enabled` (defaulting to `true` when no config file
exists, so an unconfigured session keeps the bar). This is the first
component-level enable flag in the configuration; ADR-0044 anticipated
exactly this pattern ("if configurability is needed later, a static
registry or a config flag in `ass` covers it without an ABI").

### 3. Implement the StatusNotifierItem tray in `aegis-statusbar`

The compositor becomes the session's StatusNotifierWatcher and Host.
The D-Bus work lives in `aegis-statusbar::tray` on two dedicated
`std::thread`s using `zbus` v5 with `default-features = false` and the
`async-io` + `blocking-api` features — pure Rust, no libdbus system
dependency, no tokio/async-std in the dependency graph (verified with
`cargo tree`). The threading model mirrors the existing notification
queue and status poller: state crosses the thread boundary only as
plain values through `Arc<Mutex<_>>`; the render thread reads a
`TraySnapshot` once per frame and sends `TrayCommand`s back over a
`std::sync::mpsc` channel. The compositor's main loop never blocks on
D-Bus.

The watcher implements the spec-exact surface: it serves
`org.kde.StatusNotifierWatcher` at `/StatusNotifierWatcher`
(`RegisterStatusNotifierItem`/`RegisterStatusNotifierHost`, the
`RegisteredStatusNotifierItems`/`IsStatusNotifierHostRegistered`/
`ProtocolVersion` properties, and the
`StatusNotifierItemRegistered`/`Unregistered`/`HostRegistered`
signals), tracks each item's `Id`, `Title`, `IconName`, `IconPixmap`,
`Status`, and `Menu` property, and reacts to `NewIcon`/`NewTitle`/
`NewStatus`/`PropertiesChanged` plus `NameOwnerChanged`.

Rendered icons come from two sources, both uploaded through
`flux::Image::from_bytes` against the bar's borrowed `flux::Device`
(refcounted in C, held non-owning on the render thread, matching
`Shell::new`'s existing pattern):

- `IconPixmap` — the SNI ARGB32 (network-byte-order) payload is
  converted to BGRA8 (the format flux samples) and uploaded at the
  entry closest to 48px (24px @2x).
- `IconName` — resolved through the freedesktop icon theme via
  `ass_apps::resolve_icon_scaled`, decoded in-process for raster
  formats and through `rsvg-convert` for SVG/SVGZ, with the same
  light-foreground recolor the compositor applies to its own HUD
  symbolics. Missing or undecodable names fall back to the generic
  glyph.

A texture cache keyed by item key plus `icon_generation` avoids
re-uploading on status-only updates; the watcher bumps the generation
only when the icon actually changes.

Left click sends `Activate(x, y)`. Right click on an item that exposes
a `Menu` object path opens a compositor-rendered popover implementing
the client side of `com.canonical.dbusmenu`:

- `GetLayout(0, -1, [...])` fetches the whole tree once on open;
  `AboutToShow(0)` is sent first per the spec.
- `LayoutUpdated` re-fetches only when the signal's path matches the
  open item's menu.
- Activation sends `Event(id, "clicked", ...)`.
- The popover reuses the dock's `glass_panel_opts` / `place_popup` /
  `selectable` rendering idiom (duplicated in `bar.rs`, marked for a
  future shared-helper extraction). Submenus navigate by breadcrumb
  (`menu_path: Vec<i32>`) with a `‹ Back` row rather than nested
  popovers, keeping the lens layer count flat. Toggle entries render
  `✓` (checkmark) or `●` (radio) glyphs when their state is set.

Right click on an item without a `Menu` path falls back to
`SecondaryActivate(x, y)` per the spec.

The combined tray row (SNI cells plus the existing per-application
"open window" cells) folds into a fixed slot budget:
`MAX_TRAY_ITEMS = 5`. App-tray cells keep priority (open windows stay
reachable), SNI cells fill the remaining slots, and any remainder
collapses into a leftmost `+N` overflow indicator. The decision is a
pure helper (`fold_tray`) with unit tests.

Failure stays silent. Without a session bus, when another watcher
already owns the name, or when the worker threads cannot start, the
tray simply shows no SNI icons; the compositor's startup is
unaffected. The tray worker logs at `info`/`warn` and returns `None`.

### 4. Threading and event-loop discipline

This is the workspace's first D-Bus dependency and the first chrome
component that runs background threads. The implementation deliberately
preserves the project's existing discipline:

- No async runtime. zbus's blocking API runs on `std::thread`s; no
  `tokio` / `async-std` / `smol` / `calloop` enters the dependency
  graph.
- The render thread never blocks on D-Bus. It reads `TraySnapshot`
  under a brief lock, uploads icons synchronously into the borrowed
  flux device (the same thread that created the device), and sends
  commands via mpsc.
- GPU resource ownership is unchanged. The composition root still owns
  the flux device and the application icon cache; the bar borrows the
  device non-owning and owns only its own SNI textures.

## Alternatives

- **Keep the bar in `aegis-shell` and add SNI as a module there.**
  Rejected: the moment SNI lands, `aegis-shell` carries `zbus` and two
  background threads. Every transitive consumer of the contract crate
  (the binary, the dock, the Control Center, the wallpaper) would
  inherit the dependency, and the "contract is dependency-free except
  for lens" property would be lost.
- **Async runtime (`tokio` or `smol`) for the D-Bus work.** Rejected:
  the project has been synchronous since inception by design
  (`libc::poll` over backend fds in the main loop, `std::thread` for
  workers). zbus v5's blocking API on its internal `async-io` executor
  fits the worker-thread pattern without introducing a runtime the
  rest of the project would have to reason about.
- **`dbus` (libdbus) bindings instead of `zbus`.** Rejected: libdbus
  is a C system dependency the project would have to declare on every
  target; `zbus` is pure Rust and adds no system-package requirement.
- **Route SNI icons through the composition root's icon cache like
  application icons.** Considered and rejected: it would have spread
  SNI state across `aegis-statusbar` (D-Bus + item tracking) and the
  `ass` binary (texture upload + catalog merge), violating the
  component-owns-its-presentation rule. Borrowing the flux device
  non-owning on the render thread (the same thread that created it)
  keeps the entire feature inside `aegis-statusbar` without breaking the
  GPU-resource ownership invariant.
- **Layer-shell / `StatusNotifierItem` as an external process.**
  Rejected for the same reasons ADR-0044 rejected running the dock as
  an external process: reserved edges, backdrop-capture, modal
  interplay, pointer capture, and per-frame ticks are
  compositor-internal paths with no protocol path, and the bar
  integrates with all of them.
- **Make the bar non-optional.** Rejected: a tiling-WM session that
  wants only the dock and the launcher is a real configuration, and
  ADR-0044 had already noted the absence of a per-component enable
  pattern.

## Consequences

- `aegis-shell`'s public API names no component-specific type beyond the
  `HUD_HEIGHT` layout constant, and `aegis-shell` carries no D-Bus
  dependency. The "contract is dependency-free except for lens"
  property is preserved.
- `[statusbar] enabled` becomes the first per-component enable flag,
  establishing a pattern future chrome components can follow (for
  example, an `[overview] enabled` or `[launcher] enabled`). The
  pattern is a flat `enabled: bool` on a per-component config struct
  consumed by the composition root — no dynamic plugin registry.
- The workspace gains its first `zbus` dependency. `cargo tree` for
  `aegis-statusbar` shows `zbus` and `async-io` (zbus's internal
  executor); no other crate is affected because no other crate depends
  on `aegis-statusbar`.
- The bar owns SNI textures and a borrowed flux device reference; the
  GPU-resource ownership invariant documented in ADR-0044
  (`IconSet`/`AppCatalog`) is preserved unchanged for application
  icons, with the new SNI path documented separately in `aegis-statusbar`'s
  rustdoc and crate README.
- Follow-up work this decision creates: extract the shared popover
  helpers (`glass_panel_opts`, `place_popup`) from `aegis-shell::app_menu`
  and `aegis-statusbar::bar` into a shared module; wire the selected icon
  theme (`ASS_ICON_THEME` / GSettings) into SNI `IconName` resolution;
  per-row `AboutToShow` and `ItemsPropertiesUpdated` incremental
  refreshes; menu-row icons (`icon-name`/`icon-data`); mnemonic
  underline rendering; and a live-bus smoke test harness for the
  dbusmenu popover.
