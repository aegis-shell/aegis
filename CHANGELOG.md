# Changelog

Notable user-visible and contributor-visible changes to ass. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once the
project cuts a tagged release.

## Unreleased

### ass-ctl: command-line driver for the IPC
- A new `ass-ctl` binary (and `ass_ctl` library) drives a running compositor
  over its IPC socket — the reference external tool (ADR-0027). Subcommands:
  `windows`, `workspaces`, `focus <id>`, `close <id>`,
  `switch <next|prev>`, `tiling`, `notify <summary> [body]`, `quit`, and
  `help` (which works without a server). Connects to
  `$XDG_RUNTIME_DIR/ass.sock`; the library's `run` entry point is
  unit-tested against a loopback server.
- This makes the compositor scriptable from the shell and validates the
  client end of the IPC end to end.

### Notifications (M9, over the IPC)
- ass has notifications. The IPC `Notify { summary, body, app_id }` command
  (control) posts one; subscribers receive a `Notified { notification }`
  event, and `GetNotifications` queries the live queue. Notifications live
  in a time-expiring queue (default 5 s TTL) owned by the binary and shared
  with a new `Toast` chrome component, which renders them as a top-right
  stack (newest on top, capped at 5). No `Chrome` trait change — the toast
  reads the shared queue directly each frame.
- New pure module `ass_core::notify` (`Notification`, `NotificationQueue`
  with `push`/`expire`/`recent`/`snapshot`), unit-tested. The main loop
  pushes on `Notify` (broadcasting the event) and expires entries once per
  frame.
- This is ass's own notification path (ADR-0027 rejected D-Bus); a
  `org.freedesktop.Notifications` bridge is a possible later addition.

### Configurable tiling + output-geometry work-area
- The tiling gaps and master ratio are now configurable via a `[layout]`
  table (`gaps`, `master_ratio`), applied live on config reload. New
  `ass-config` `LayoutConfig` section converts to `ass_core::layout::LayoutParams`.
- The tiling work-area now comes from the focused output's geometry
  (ADR-0028) rather than a hardcoded rect: the server tracks an
  `OutputGeometry` (`set_output_geometry`, called by the backend on resize)
  and `apply_tiling` tiles into its logical rect. With the nested backend
  (scale 1, no transform) the work-area is unchanged; real per-output scale
  and transform take effect when M7 wires backend geometry.
- `apply_tiling` no longer takes a work-area argument.

### Output geometry groundwork (ADR-0028)
- New pure module `ass_core::output` models one output's physical mode
  (`OutputMode`: width, height, refresh in millihertz), scale (`Scale`,
  fractional for HiDPI), transform, and global logical position
  (`OutputGeometry`). The logical size the chrome and clients see is derived:
  physical mode, axes swapped for 90°/270° transforms, divided by the
  (integer or fractional) scale. Unit-tested with exact assertions for
  identity, integer and fractional scale, axis-swap, their composition, and
  non-positive-scale fallback. This is the foundation for the multi-output
  milestone (M7) and the chrome-aware tiling work-area; server/backend wiring
  lands with M7. `Transform` gained serde derives so a geometry serializes.

### Window rules (ADR-0026)
- Config-driven placement rules, written as `[[window_rule]]` tables. A rule
  matches a newly-mapped toplevel by `app_id` and/or `title` (case-insensitive
  substring, AND-ed) and prescribes a workspace move and/or a forced layout
  role. The first match applies at first map.
  ```toml
  [[window_rule]]
  app_id = "firefox"
  workspace = 2
  role = "tiled"

  [[window_rule]]
  title = "calculator"
  role = "floating"
  ```
- A rule with no matchers matches nothing (a bare `{ role = "floating" }` does
  not catch every window). `workspace` is a 1-based index on the focused
  output and applies only if that workspace exists. `role` is `floating` or
  `tiled` (now lowercase on the wire).
- Tiling now respects the layout role: a `floating`-role window is exempt
  from tiling even when its workspace is in tiled mode (ADR-0024 floating
  exceptions). New pure module `ass_core::window_rule` owns the matching
  logic, unit-tested in isolation; `ass-config` deserializes the rules and
  `ass-server` applies them on map and on config reload.
- Limitation: rules evaluate at first map; `app_id`/`title` set after mapping
  are not re-evaluated yet (follow-up).

### Workspace replug-restore (ADR-0025)
- The workspace model now restores a disconnected output's workspaces when
  its connector returns. Each workspace remembers its birth connector
  (`Workspace.origin`); outputs carry a stable `connector` identity
  (`Output.connector`, surfaced in the IPC snapshot). Unplugging an output
  relocates its non-empty workspaces to the primary survivor (origin
  preserved); re-adding the same connector moves them home. `add_output`
  now takes a connector name. Fully unit-tested in isolation (the
  single-output server passes "nested" and never hotplugs, so its behavior
  is unchanged; real hotplug wiring lands with the multi-output backend,
  ADR-0028).

### Tiling (ADR-0024)
- New pure module `ass_core::layout` owns the tiling policy as geometry: a
  `Layout` trait (`layout(work_area, n_tiled, params) -> Vec<Rect>`), a
  `MasterStack` policy (master column + equal stack rows), `LayoutParams`
  (gaps, master ratio), and a `LayoutRole` (`Floating`/`Tiled`). A tiled
  window is still a `Window` with a position and size — the policy just sets
  them, never a separate container type. `Window` gains a `layout_role`
  field (`Floating` by default). Unit-tested in isolation with exact
  rectangle assertions.
- Server application: `Super+T` (keybind action `ToggleTiling`, configurable)
  or the IPC `ToggleTiling` command flips the current workspace to tiled.
  The master-stack policy runs over the workspace's windows each frame and
  reconfigures only those whose target rect moved, so steady state sends no
  `xdg_toplevel.configure` events (a new `reconfigure_with_size` forces an
  explicit width/height, unlike the advisory 0×0 state-bit path). The work
  area is the full output for now; chrome-aware margins are a follow-up.
- The IPC `ToggleTiling` command makes tiling scriptable (and drove the
  end-to-end design); the layout math is unit-tested.

### Workspaces (M6, first cut)
- ass is now workspace-aware. Each output owns a dynamic set of workspaces
  with one always empty at the end (the GNOME/niri model,
  [ADR-0025](docs/adr/0025-workspace-model.md)). A toplevel maps onto the
  focused output's current workspace; rendering, the chrome snapshot, and
  focus cycling see only the visible workspace's windows. Switching away
  from a window drops its keyboard focus (a `wl_keyboard.leave` is posted)
  so keystrokes do not route to a hidden window.
- New pure model `ass_core::workspace` (`WorkspaceModel`, `Workspace`,
  `Output`, `WorkspaceId`/`OutputId`) owns the semantics: toplevel
  place/remove/move, `switch`/`switch_to`, the trailing-empty invariant,
  empty-workspace reaping, multi-output independence, and output-removal
  relocation. It is unit-tested in isolation and has no flux, lens, or
  Wayland dependency.
- Key bindings gained two actions, `WorkspaceNext`/`WorkspacePrev`, bound by
  default to `Super+Right`/`Super+Left` and configurable (action names
  `workspace_next`/`workspace_prev`, aliases `ws_next`/`ws_prev`) through
  the config file's `[[keybind]]` section. A workspace switch with no live
  client is a no-op.
- The IPC exposes the workspace model (ADR-0027): `GetWorkspaces` returns a
  serializable snapshot of every output, its current workspace, and each
  workspace's toplevel ids; `SwitchWorkspace`/`SwitchWorkspaceTo` commands
  drive the same switch path as the key bindings; and a `WorkspaceChanged`
  event is pushed to subscribers whenever the model moves (switch, place,
  remove, reap). The binary grants all capabilities to local clients on the
  `$XDG_RUNTIME_DIR` socket (the boundary becomes load-bearing for the agent
  phase). The workspace snapshot types live in `ass-core` (serde-derived) so
  the IPC sends them without reconstructing them.
- A top-center workspace indicator (`WorkspaceBar` chrome component) shows
  one numbered tile per workspace, highlights the current one (`[n]`), and
  switches on click. The `Chrome` trait's `render` now takes the workspace
  snapshot; the existing components ignore it. The bar hides while there is
  only a single workspace (nothing to switch to) and appears once a window
  maps.
- Out of scope for this cut (follow-ups): the optional tiling policy
  ([ADR-0024](docs/adr/0024-layout-model.md)) and workspace replug-restore
  ([ADR-0025](docs/adr/0025-workspace-model.md)) are not yet implemented.

### IPC and introspection surface (query, control, and events)
- ass now exposes a versioned IPC over a unix socket at
  `$XDG_RUNTIME_DIR/ass.sock`, the foundation of the extension and
  automation surface ([ADR-0027](docs/adr/0027-ipc-and-introspection.md)).
  It is the path the chrome, external tools, and the later agent layer all
  share: every capability returns the same `ass_core::window::Window` the
  renderer and chrome read, with no separate wire DTO.
- The protocol is length-framed JSON with an explicit major version
  (`PROTOCOL_VERSION = 1`); a client offering any other version is refused at
  the handshake. The handshake negotiates capabilities (`query`/`control`/
  `session`); `query` is always granted, `control`/`session` are intersected
  against server policy.
- `query`: `GetWindows` returns the live toplevel snapshot in z-order.
- `control`/`session`: `Do` submits a command — `Focus`, `Close`, `Move`,
  `Cycle` (control) and `Quit` (session) — mirroring the operations the
  chrome and key bindings already perform. Commands are fire-and-forget:
  the server acknowledges queuing with `Ok`, not completion, and applies
  them on the main loop (the Wayland server state is not `Send`, so
  connection threads forward through a channel rather than touching it).
  Re-query or subscribe to observe the effect.
- Events: `Subscribe` opts a connection into server-pushed events; the
  compositor broadcasts `WindowsChanged` whenever the visible window set
  moves (focus, add/remove, retitle). Each connection runs a reader thread
  (protocol) and a writer thread (sole write-half owner), so responses and
  events never contend; the subscriber registry is reaped on disconnect.
- Bind failure is non-fatal (the compositor runs without IPC); a stale
  socket from a crashed run is removed on startup and on shutdown.
- New pure crate `ass-ipc` (depends only on `ass-core`, `serde`,
  `serde_json`) owns the schema, codec, server (`Handler` trait + accept
  thread + per-connection reader/writer threads), and reference client,
  verified end-to-end by loopback tests covering query, commands, capability
  refusal, and event delivery. `ass-core` gained an optional, off-by-default
  `serde` feature deriving `Serialize`/`Deserialize` on the shared model
  types (`Window`, `WindowState`, `SizeHints`, `Point`, `Size`, `Rect`) so
  the IPC sends the same types rather than reconstructing them.

### Declarative configuration (TOML + live reload)
- Configuration now lives in a single TOML file at
  `$XDG_CONFIG_HOME/ass/config.toml` (defaulting to `~/.config/ass/config.toml`),
  replacing the ad hoc `$ASS_KEYBINDS` environment variable as the source of
  truth for user-tunable behavior. The file carries an explicit
  `schema_version`; this build supports `1`. See
  [ADR-0026](docs/adr/0026-configuration-system.md) and the
  [configuration reference](docs/reference/config.md).
- The first section is key bindings, written as an array of `[[keybind]]`
  tables:
  ```toml
  schema_version = 1

  [[keybind]]
  mods = ["super"]
  key = "space"
  action = "launcher"

  [[keybind]]
  mods = ["super", "shift"]
  key = "q"
  action = "quit"
  ```
  Entries layer over the built-in defaults (a file with one binding keeps
  the rest). Modifier names: `shift`, `ctrl`/`control`, `alt`/`mod1`,
  `super`/`meta`/`win`/`mod4`. Action names: `launcher`, `close`, `cycle`
  (alias `next`), `prev`, `quit`. Key names cover letters, digits, and the
  common controls (`return`, `escape`, `tab`, `f1`–`f12`, arrows, …).
- The file is hot-reloaded: editing it on disk changes behavior without a
  restart, checked once per frame by mtime. A malformed file, an unknown
  `schema_version`, or an unresolvable `[[keybind]]` entry is reported as a
  structured `config:` diagnostic with field path and (for parse errors)
  source line, and never crashes the compositor. Good entries in a partially
  invalid file still take effect.
- New pure crate `ass-config` (depends only on `ass-core`, `serde`, `toml`,
  `dirs`) owns the schema, the loader, the watcher, and the migration logic;
  it is unit-tested in isolation. `ass_core::keybind::{mod_from_name,
  action_from_name}` are now public so the config layer reuses the existing
  name-resolution tables instead of duplicating them.
- `$ASS_KEYBINDS` remains honored as a **deprecated transitional override**
  (logged on each reload) and takes precedence over the file; it will be
  removed before the desktop phase closes. Move bindings into the
  `[[keybind]]` section of the config file.

### Real application icons in the dock
- The dock now renders decoded application icon textures instead of a fixed
  glyph when an icon is available for a window's `app_id`. The binary
  decodes each `.desktop` entry's raster icon once at startup into a flux
  texture, keyed by every `app_id` the entry might run as (`StartupWMClass`,
  the desktop-id stem, and the icon name, all lowercased), so the dock can
  look a running toplevel up by its `app_id`. Windows with no matching icon
  fall back to the glyph. SVG icons are not yet rasterized (no rasterizer
  dependency); entries whose only icon is SVG fall back to the glyph.
  Launcher-row icons remain a follow-up.
- This required a new raster-image capability in lens, which had none (its
  only icon API was a fixed glyph set). Added `lens_image` (draw a host-owned
  `flux_image` as a widget) and `lens_image_button` / `lens_image_button_active`
  (texture-backed variants of the icon buttons with identical hover / active /
  click behaviour) to lens, plus their Rust bindings. The pre-existing
  `LENS_DRAW_IMAGE` draw command (a reserved stub) is now implemented in the
  replay pass via `flux_canvas_draw_image`. `flux::Image` gained an `as_raw`
  accessor (all other flux types already had one).

### Build robustness
- `ass-protocols` build script now probes `pkg-config --cflags-only-I
  wayland-server` for the include path instead of assuming `wayland-util.h`
  is in the compiler's default search path. Required on sysroot-based
  distributions (e.g. theseus/wright) where libwayland headers live in a
  build sysroot rather than `/usr/include`.
- `scripts/env.sh` now also configures the theseus/wright sysroot when
  present: `PKG_CONFIG_PATH` (so `wayland-server.pc` is found), `CPATH` (so
  the C compiler finds `wayland-util.h`, which its `.pc` advertises as
  `/usr/include` and pkg-config therefore drops from cflags), `PATH`
  (`wayland-scanner`), and `LD_LIBRARY_PATH` (`libwayland-server.so.0` at
  runtime). Harmless on conventional distros where wayland is in `/usr`.

### Soundness
- Fixed a use-after-free in `Server::drop`: surface boxes were reclaimed
  *before* `wl_display_destroy`, leaving each `wl_resource`'s `user_data`
  dangling for the destroy-notify fired during display teardown to
  dereference. The display is now destroyed first (its notifys free the boxes
  and null the slots); the reclaim loop then handles only orphaned slots.
  Manifested as a flaky shutdown segfault once any client had connected.

### Configurable key bindings
- Added global key bindings with a built-in default set and an optional
  `$ASS_KEYBINDS` override. The default set: `Super+Tab` cycles focus forward,
  `Super+Shift+Tab` backward, `Super+Return` toggles the launcher, `Super+Q`
  closes the focused window, and `Super+Shift+Return` quits. A bare Super tap
  still toggles the launcher alongside these.
- `$ASS_KEYBINDS` is a `;`-separated list of `mods+key=action` entries, e.g.
  `super+space=launcher;super+q=close;ctrl+alt+del=quit`. Recognised modifier
  names: `shift`, `ctrl`/`control`, `alt`/`mod1`, `super`/`meta`/`win`/`mod4`.
  Key names cover letters, digits, and common controls (`return`, `escape`,
  `tab`, `space`, `up`/`down`/`left`/`right`, `f1`–`f12`, …). Actions:
  `launcher`, `close`, `cycle`/`next`, `prev`, `quit`. User overrides take
  precedence over the defaults; the defaults remain as fallback. Malformed
  entries are logged and skipped.
- Matching is exact on the depressed modifier mask, so `Super+Q` does not also
  fire on `Ctrl+Super+Q`. A matched key is **consumed before client delivery**:
  the focused client never sees the key that triggered a global binding (a
  text editor does not insert `q` when you press `Super+Q` to close).
- New pure module `ass_core::keybind` (`Mods`, `Action`, `Keybind`, `Keymap`)
  with the parser and matcher, unit-tested in isolation (no flux/lens/Wayland
  dependency). `ass_core::input::KeyChar` gains a `mods` field carrying the
  xkbcommon depressed-modifier mask at press time.
- `Server::forward_input` now takes the keymap and returns the matched
  actions; `Server::keyboard_key` always advances xkbcommon state (so bindings
  and modifier tracking work on an empty desktop with no focused client) and
  suppresses posting for consumed keys. Added `Server::focused_toplevel_id`
  and `Server::cycle_focus(forward)` to back the `close` and `cycle` actions.

### Window minimization
- `xdg_toplevel.set_minimized` is now a real handler (was a no-op). The
  compositor hides the surface from rendering (`toplevel_frames` /
  `toplevel_dmabuf_frames` skip it) and from pointer hit-testing, but keeps
  it mapped so the client retains its buffers. If the minimized toplevel held
  keyboard focus it is dropped (`wl_keyboard.leave` posted, activated bit
  cleared) so typing no longer routes to an invisible window.
- Restore is focus-driven: any later focus gain on a minimized toplevel
  (`set_activated_for_surface` with `activated = true`, reached from the
  window list or dock click via `focus_surface_by_id`) clears the minimized
  flag, reconfigures, and brings the window back. No new chrome intent was
  needed — the existing `clicked` → `focus_surface_by_id` path restores.
- `ass_core::window::Window` gains a compositor-internal `minimized: bool`
  (not a `WindowState` bit, since `xdg-shell` defines no minimized configure
  state). The window-list panel marks minimized rows with `◌` and still
  lists them so the user can restore them; clicking a minimized row restores
  and focuses it.

### Build and dependencies
- Migrated the shell from the removed in-tree `flux-ui` binding to the split
  **flux / lens stack**. The old sibling `flux` monorepo was decomposed for
  v0.1 into focused libraries under `../optics`: `flux` (`libflux`),
  `lens` (`liblens`, the successor to `flux-ui`), and out-of-tree Rust
  bindings `flux-rs` (`flux` / `flux-sys`) and `lens-rs` (`lens` /
  `lens-sys`). The workspace now depends on `flux` / `flux-sys` from
  `flux-rs` and `lens` / `lens-sys` from `lens-rs`; every `flux_ui` reference
  in the shell became `lens`. The migration was a near-drop-in rename —
  lens's safe surface matches what `ass-shell` used, and `lens-sys`'s
  bindgen allowlist covers the `flux_*` types the device-binding seam casts
  across. See [ADR-0023](docs/adr/0023-split-flux-lens-stack.md), which
  supersedes ADR-0005.
- The terminal binary's rpath relay now keys on `DEP_FLUX_RPATHS` and
  `DEP_LENS_RPATHS` (the `-sys` `links` metadata) so it resolves
  `libflux.so` and `liblens.so` from the meson build trees at runtime.
- Added `scripts/env.sh`: source it once per shell to export the dev-mode
  variables (`FLUX_BUILD_DIR`, `FLUX_SOURCE_DIR`, `LENS_BUILD_DIR`,
  `LENS_SOURCE_DIR`) the `-sys` build scripts use to locate freshly-built
  flux and lens without `meson install`. Set `ASS_DEV_ENV_USE_INSTALLED=1`
  to link installed libraries instead.
- `ass-shell` now compiles end-to-end for the first time. A latent bug
  surfaced: the launcher's `emit` helper had been placed inside the
  `impl Chrome for Launcher` block (not a trait method). It is moved to the
  inherent `impl Launcher` block.
- Updated `README.md`, `docs/dev/setup.md`, `docs/dev/project-layout.md`,
  and `docs/explanation/architecture.md` for the new dependency paths and
  the `flux-ui` → `lens` rename. Older ADRs retain their original `flux-ui`
  wording as historical record; ADR-0023 notes the equivalence.

### Launcher
- Added an application launcher: enumerate every launchable `.desktop` entry
  on the host (freedesktop.org Desktop Entry Specification) and expose it in
  the chrome as a top-center toggle that expands into a centered list. Click a
  row to launch, or search: type to filter, Up/Down to move the selection,
  Enter to launch, Backspace to delete, Escape to close. The launched process
  is detached — it runs in a new session via `setsid`, inherits the Wayland /
  XDG environment, and survives the compositor exiting.
- While the launcher is open it captures the keyboard: the `Chrome` trait
  gained `captures_keyboard` and `key_char` default no-ops (only the launcher
  overrides them), the main loop routes key events to the chrome and withholds
  them from the focused client, the server sends a proper
  `wl_keyboard.leave` on grab and `wl_keyboard.enter` on release
  (`Server::{grab,release}_keyboard_focus`, restoring the pre-grab focus only
  if nothing else took it during the session), and the server's
  `Keyboard::update_key` / `Server::key_char` resolve each key to an
  xkbcommon keysym + printable char to feed the search box. The
  query/filter/selection logic lives in a pure, unit-tested
  `ass-core::launcher` module; the flux-ui component is a thin adapter.
- Added two new leaf crates: `ass-apps` (desktop-entry parsing, `XDG_DATA_HOME`
  / `XDG_DATA_DIRS` traversal, deduplication with user-overrides-system
  precedence, locale resolution via `LC_MESSAGES`, `Exec` field-code
  expansion, and Icon Theme Spec lookup with the `hicolor` fallback) and
  `ass-launch` (the detached spawn path, terminal-emulator wrapping for
  `Terminal=true` entries).
- A bare **Super tap** (press and release with no other key in between) now
  opens the launcher from anywhere, even while an app has keyboard focus.
  Detection is a pure `ass_core::input::TapDetector` fed every key event;
  Super still works as a modifier in every other combo (the tap is observed,
  not intercepted). Left and right Meta are equivalent.
- The launcher is **aware of running apps**: activating an entry whose
  `StartupWMClass` (or desktop-id stem) matches a live toplevel's `app_id`
  focuses that instance instead of spawning a duplicate, via the existing
  focus-by-surface-id path. Running rows are marked with a leading `●`.
- Added `ass-core::app::Entry` as the shared launchable-application model,
  `ass-core::launcher` (with the `Launch::{Spawn,Focus}` outcome) for the
  search state machine, and
  `ass-core::input::{KeyChar, KeyAction, key_action, TapDetector}` for the
  keyboard path — so the shell chrome needs no `ass-apps` dependency.
- `ass-shell` gains a `Launcher` chrome component, three `Chrome` trait
  methods (`captures_keyboard`, `key_char`, `toggle`), and a
  `ChromeEvents::spawn` intent; its dependency graph is unchanged. The binary
  wires enumeration to the launcher, drains the spawn intent into
  `ass-launch`, routes keyboard capture, and runs the Super-tap detector.
- See [ADR-0022](docs/adr/0022-application-launcher.md). Rendering real app
  icons as textures, a runtime application rescan, and a configurable
  keybind (e.g. `Super+Space`) are follow-up work. Apps that set neither a
  matching `app_id` nor `StartupWMClass` will not be recognized as running.

### Shell architecture
- Split `ass-shell` into a pure core host and pluggable chrome components.
  `Shell` now owns only the flux-ui context, the per-frame window snapshot,
  the interaction sink, and a component registry; it has no built-in chrome.
  Each surface — the window-list side panel, server-side decorations, the
  dock — is a `Chrome` trait implementation in a `chrome/` module, registered
  by the binary via `Shell::add`.
- Added the `Chrome` trait and `ChromeEvents` sink as the seam: a component
  renders itself from the shared snapshot and input and pushes user intents
  (quit/focus/close/move) into the sink. The main loop's `set_windows`,
  `render`, and `take_*` calls are unchanged.
- See [ADR-0021](docs/adr/0021-chrome-component-trait.md). Adding a chrome
  surface (e.g. a future HUD bar) is now local: a new `Chrome` impl plus one
  `Shell::add` line.

### HiDPI
- `wl_surface.set_buffer_scale` is now applied at composite time on both
  the shm and dma-buf paths. A client that commits at scale N renders at
  1/N its buffer dimensions instead of N× the intended on-screen size.
- `SurfaceGeometry::buffer_scale` now defaults to 1 (the previous `i32`
  default of 0 would have divided by zero had any call site forgotten to
  populate it).
- `wp_viewport.set_source` rectangles are now also divided by
  `buffer_scale` when `viewport_dst` is unset, matching
  `weston_surface_update_size` and the `wp_viewport` spec.
- The renderer's incremental-upload path is bypassed when
  `buffer_scale > 1` (mirroring the existing `transform != Normal`
  bypass); full uploads on generation change remain correct.
- See [ADR-0020](docs/adr/0020-buffer-scale-applied-at-composite.md).

### Dock
- Added a macOS-style dock to the chrome: a rounded translucent panel
  anchored to the bottom-center of the output, holding one icon tile per
  mapped toplevel. Clicking a tile focuses that window; the activated
  window's tile is highlighted. Rendered as a `flux-ui` overlay, reusing
  the `clicked_window` → `Server::focus_surface_by_id` path with no new
  window-management API.
- See [ADR-0019](docs/adr/0019-dock-as-bottom-center-overlay.md).

### Wallpaper
- Added a new `ass-wallpaper` crate that draws a user-chosen
  background as the bottom-most layer of every frame, beneath client
  surfaces and the chrome. Loaded via `$ASS_WALLPAPER` at startup; the
  clear colour shows through when unset or load fails.
- Still images decode through the `image` crate, covering PNG, JPEG,
  GIF, WebP, BMP, TIFF, TGA, QOI, ICO, and PNM.
- Animated GIF and animated WebP advance frame-by-frame on wall-clock
  pacing; sub-rect frames are composited onto the full canvas during
  decode so consumers see uniformly-sized buffers.
- Short videos decode through an external `ffmpeg` child process
  (`-pix_fmt bgra -f rawvideo -`) consumed by a background reader
  thread, which loops the source on EOF and exposes the latest frame
  to the main loop non-blocking. Requires `ffmpeg` on the host.
- See [ADR-0018](docs/adr/0018-wallpaper-crate.md).

### Foundation repair
- Fixed workspace dependency paths so `cargo build` resolves against the
  flux monorepo layout (`../flux/core`, `../flux/ui`) instead of the
  obsolete `../flux-ui` separate-repo layout.
- Initialized the repository as git; added `.gitignore`, `rust-toolchain.toml`
  (stable channel with `rustfmt` and `clippy`).
- Corrected build-path references in `README.md`, `docs/dev/setup.md`,
  `docs/dev/project-layout.md`, `docs/index.md`, and
  `docs/explanation/architecture.md`.

### Soundness
- Added compile-time `assert_impl_opcode_count!` so every
  `*_interface_impl` struct carries exactly the request count the protocol
  advertises. The next vtable under/oversize becomes a hard build failure
  rather than latent undefined behavior.
- Fixed `wl_data_device_manager_interface_impl` (v3 binding, missing
  `destroy` opcode 2) — previously an out-of-bounds vtable read on any
  client `destroy` request.
- Removed the intentional `SurfaceRec` leak: surfaces own their slot index
  and back-pointer to `State`, the destroy notify detaches the entry and
  reclaims the box, and held dma-buf-backed buffers now receive
  `wl_buffer.release` on surface destroy.
- `seat.get_pointer` / `get_keyboard` / `get_touch` now allocate an inert
  resource for the requested new-id even when caps are zero, so a
  non-conforming client gets a no-op instead of a dangling id.
- `zwp_linux_buffer_params_v1.create_immed` failure now posts the
  protocol-required fatal `invalid_wl_buffer` error instead of silently
  leaving the client's new-id unallocated.
- The nested backend's `Drop` now explicitly destroys the `wl_compositor`
  proxy and the bound host `wl_pointer` if one was created.

### Architecture
- Adopted the `log` facade in every workspace crate, with `env_logger` as
  the single concrete implementation in the binary. `RUST_LOG` controls
  verbosity (default `info`).
- Migrated `ServerError`, `NestedError`, and `ShellError` to `thiserror`,
  removing handwritten `Display`/`Error` impls.
- Added `ass_core::input` with `InputEvent`, `ButtonState`, and the
  Wayland-state mapping helper.
- Extended `ass_core::SurfacePixels` / `SurfaceDmabuf` with
  `SurfaceGeometry` (position, window geometry, transform, buffer scale)
  and added an 8-case `Transform` enum mirroring `wl_surface` semantics.
- The `Backend` trait now requires `take_input(&mut self) -> Vec<InputEvent>`
  and `take_resize(&mut self) -> Option<Size>`; the nested backend
  implements both.

### Input pipeline (M1)
- The nested backend binds the host `wl_seat` (v4) and installs seat,
  pointer, and keyboard listeners. Host pointer and keyboard events
  translate to `InputEvent`s and buffer into `state.input_events`, drained
  by `Backend::take_input`.
- The server advertises pointer and keyboard capability (keyboard only
  when the xkbcommon keymap compiled successfully at startup), creates
  tracked `wl_pointer` / `wl_keyboard` resources, and exposes
  `Server::forward_input` to drive focus transitions and event dispatch.
- `Server::forward_input` hit-tests pointer motion against mapped
  toplevels, posts `wl_pointer.enter`/`leave`/`motion`/`button` to the
  focused client's pointer resources, and clears focus on host leave.
- The keyboard pipeline compiles a default `"evdev"/"pc104"/"us"` keymap
  via xkbcommon into a sealed memfd, sends `wl_keyboard.keymap` on each
  client bind, advances `xkb_state` on every key event, and posts
  `wl_keyboard.modifiers` and `wl_keyboard.key` to the focused client.
  Default repeat is 25 cps / 250 ms delay.
- Click-to-focus: pointer-button press transitions keyboard focus to the
  surface under the cursor; pointer motion no longer steals keyboard focus.
- The main loop mirrors drained input into `flux_ui::Input` before
  forwarding to the server; the shell's Quit button is now clickable.

### Compositor geometry
- `SurfaceRec.position` is assigned on first map (diagonal cascade,
  placeholder for M3 window-manager policy) and surfaced through
  `SurfaceGeometry` to the renderer.
- The renderer's `i*32` cascade offset is removed; draws use each
  surface's authoritative `position`. Hit-test and renderer now agree.

### Subsurface tree (M2)
- `SurfaceRec` gains `parent`, `children`, `subsurface_offset`, and
  `subsurface_above_parent` fields. `get_subsurface` links parent and
  child; `set_position`, `place_above`, `place_below` are implemented.
- Destroy detaches the subsurface from its parent (and any children from
  it) so no dangling pointers survive.
- The server emits four lists per frame (`subsurface_frames_below`,
  `subsurface_frames_above`, plus the dmabuf variants) with absolute
  positions. The main loop interleaves draws in z-order: below-subsurfaces,
  toplevels, above-subsurfaces.
- M2 surfaces only direct children of mapped toplevels; nested
  subsurface-of-subsurface chains are deferred. Sync-mode cascade is
  accepted but treated as desync.

### Format coverage
- The dma-buf protocol now advertises and the renderer accepts
  `DRM_FORMAT_ABGR8888` and `DRM_FORMAT_XBGR8888` (the byte-swapped pair
  of ARGB/XRGB), mapping them to flux's `RGBA8_UNORM`. The X-variants
  carry an undefined alpha that the server forces opaque on commit.

### Viewport crop and scale (M2)
- `wp_viewport.set_source` / `set_destination` are real handlers that
  store source rect (pixel coords) and destination size (logical pixels)
  on `SurfaceRec`, threaded through `SurfaceGeometry` to the renderer.
- Added `flux_canvas_draw_image_sub` (and Rust binding
  `flux::Canvas::draw_image_sub`) — a 5-line wrapper around an
  already-shader-ready path. No flux shader or pipeline changes.
- The renderer computes destination dimensions and source UV rect from
  the four combinations of source/dst set or unset and calls the right
  flux entry point.

### Buffer transforms (M2)
- `wl_surface.set_buffer_transform` is now a real handler that stores
  the transform on `SurfaceRec` (8 cases: Normal, Rotate90/180/270,
  FlipHorizontal, FlipRotate90/180/270).
- New `transform_pixels` helper in `ass-render` applies each transform
  on the CPU at upload time, returning a borrowed `Cow` for `Normal`
  (zero cost) and an owned staging buffer for rotated/flipped cases.
  Six unit tests cover Normal-borrowed, Rotate90 (square and
  non-square), Rotate180, and FlipHorizontal.
- `wl_surface.set_buffer_scale` is also now a real handler, but its
  value is stored and not yet applied at composite (HiDPI clients
  render larger than intended until GPU-side transforms land in flux).

### Damage tracking (M2)
- `wl_surface.damage` and `wl_surface.damage_buffer` are real handlers
  that accumulate damage rects on `SurfaceRec`. The server rotates
  pending into committed at commit time and lends the slice via
  `SurfacePixels.damage`.
- The renderer's toplevel path now has three branches: cache miss /
  generation change → full upload; cache hit with damage and
  `Transform::Normal` → incremental upload via the new
  `flux::Image::update_region` binding (per rect, clamped to surface
  bounds); cache hit with no damage → skip.
- Damage is bypassed under non-Normal transforms (the math interacts
  with CPU staging non-obviously; the full-upload path still produces
  correct output). Documented in ADR-0015.
- `flux::Image::update_region` is a new Rust binding mirroring the
  existing C entry point.

### Chrome window list (M3)
- The shell renders a window-list panel below the existing Quit button.
  Each row shows the title (or `<untitled>`) with a focus marker for
  activated windows, and an `x` close button.
- `Shell::set_windows(Vec<Window>)` accepts a per-frame snapshot from
  the server; `take_clicked_window` and `take_closed_window` drain
  user interactions for the main loop to forward.
- New `Server::focus_surface_by_id(id)` drives keyboard focus from
  chrome (equivalent to click-to-focus but without synthesizing
  pointer events).

### Server-side decorations (M3)
- Per-window title bars drawn as `flux-ui` overlays anchored at each
  toplevel's absolute position. The bar shows the title and a close
  gadget; background colour differentiates activated windows.
- Click on the title area starts an interactive move via the existing
  `Server::start_interactive_move` API (no serial validation;
  compositor-initiated).
- Click on the close gadget posts `xdg_toplevel.close`.
- Title bar height and close-button width are visual constants
  (`TITLE_BAR_HEIGHT = 24.0`, `CLOSE_BUTTON_WIDTH = 24.0`); full
  `xdg_toplevel.set_window_geometry` frame-inset protocol integration
  is not implemented.

### Toplevel metadata and state (M3 partial)
- New `ass_core::window` module: `Window`, `WindowState`, `SizeHints`,
  `ResizeEdges`, and `Interactive` types with serialize-to-protocol-array
  helpers. Seven unit tests cover state-bit encoding, hints round-tripping,
  edge decoding, and interactive reporting.
- `SurfaceRec.window` is initialized when `xdg_surface.get_toplevel`
  fires and updated by real handlers for `set_title`, `set_app_id`,
  `set_parent`, `set_min_size`, `set_max_size`.
- `set_maximized` / `unset_maximized` / `set_fullscreen` /
  `unset_fullscreen` flip the corresponding state bit and emit a fresh
  `xdg_toplevel.configure` with the proper states array, followed by
  `xdg_surface.configure` for the ack serial.
- Activated state follows keyboard focus automatically via
  `change_keyboard_focus` → `set_activated_for_surface`.
- New `Server` API: `windows()` snapshots live toplevels for the shell,
  `close_toplevel(id)` posts `xdg_toplevel.close`, and
  `set_toplevel_activated(id, bool)` flips the activated bit and
  reconfigures.
- **Interactive `xdg_toplevel.move` / `resize`** with serial validation
  against the last button press. Motion during a grab updates the
  window's position (move) or size (resize, clamped to size hints with
  anchor preservation). Each resize posts a fresh
  `xdg_toplevel.configure` so the client reallocates. Button release
  ends the grab.
- Server-side decorations, overview launcher, `show_window_menu`, and
  `set_minimized` remain pending.

### Tests and CI
- Added unit tests for `ass_core` geometry (`Rect::contains`,
  `Transform::swap_axes`), `ass_core::input` (`ButtonState` Wayland
  mapping), `ass_render` (`Renderer::gc`), and `ass_server`
  (`Server::new` socket lifecycle).
- Added `.github/workflows/ci.yml` covering `cargo fmt --check`, clippy,
  and the flux-free test subset.

### Documentation
- Added ADR-0006 (FFI soundness discipline), ADR-0007 (logging facade and
  `Backend` input contract), ADR-0009 (input pipeline and pointer focus
  model), ADR-0010 (keyboard pipeline and xkbcommon ownership),
  ADR-0011 (subsurface tree and z-split rendering),
  ADR-0012 (toplevel metadata and state machine),
  ADR-0013 (interactive move and resize),
  ADR-0014 (buffer transform and viewport crop),
  ADR-0015 (per-commit damage tracking),
  ADR-0016 (shell/server window-management bridge), and
  ADR-0017 (server-side decorations via overlays).
- Updated `README.md`, `docs/dev/setup.md`, `docs/explanation/architecture.md`
  to reflect the new build paths, `RUST_LOG`, `libxkbcommon` dependency,
  and the milestone status.
