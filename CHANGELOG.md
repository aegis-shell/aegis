# Changelog

Notable user-visible and contributor-visible changes to ass. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once the
project cuts a tagged release.

## Unreleased

### Shell component crates

- The dock and the Control Center moved from `ass-shell` modules into
  their own crates, `ass-dock` and `ass-control-center`, fulfilling the
  promotion path deferred in ADR-0021. `ass-shell` keeps the `Chrome`
  host, the shared contract, and the remaining components; the `ass`
  binary remains the composition root that registers every component.
  See ADR-0044.
- The shared chrome contract no longer names component-specific types:
  `Chrome::update_app_catalog` now receives one `AppCatalog` snapshot
  (applications, resolved pins, and an `IconSet` of borrowed icon
  textures) that `Shell` owns, seeds into every registered component,
  and replaces through `set_app_catalog`.
- Application match keys (`StartupWMClass`, desktop-id stem, icon name)
  are unified on `Entry::match_keys` in `ass-core`, replacing a
  binary-local helper. No user-visible behavior change.

### Control Center display settings

- Control Center now exposes connected outputs, advertised resolution and
  refresh-rate modes, fractional scale, primary-output selection, and
  right/left/above/below or custom-coordinate extended layouts. The same
  `[[output]]` configuration remains the source of truth; edits use an atomic
  comment-preserving replacement and nested sessions present a read-only,
  host-managed display summary.
- Direct DRM mode changes now apply live through the hotplug reconciliation
  path after the in-flight page flip retires. Surface resize or recreation,
  server output advertisement, input extent, and Control Center status then
  converge on the selected mode without a restart or cable replug.

### Clipboard and screenshots

- Interactive screenshots now remain available as mode-`0600` PNG files and
  are also published to the physical human seat's clipboard as `image/png`
  and `text/uri-list`. IPC and Realm captures remain side-effect-free and do
  not modify the physical clipboard.
- Added compositor-owned, per-seat clipboard selections with immutable offer
  snapshots, bounded retained data, and background fd transfer. Agent Realm
  selections remain isolated from the physical human seat.
- Removed the optional X11-style Primary Selection protocol and its unused
  data-control placeholder. Selecting text no longer mutates a second global
  channel; standard explicit copy, cut, and paste are unaffected.

### AI Workspaces and independent input Realms

- Added first-class Realms with durable principals, independent Wayland
  seats, interaction groups, atomic control transfer, read-only observer
  mirrors, pause/resume, fail-closed revocation, and Realm-local virtual
  outputs. A transferred window keeps the same `wl_surface` and client
  instance; every toplevel on one Wayland client connection moves as one
  interaction group, independent of app-side multi-seat behavior.
- Overview now reserves a Realm shelf on the right. Dragging a live window
  thumbnail onto an active AI Workspace transfers its complete interaction
  group while retaining a non-interactive physical-desktop mirror. Dragging
  the mirror onto **Physical desktop** returns control. Mirrors are physical
  input barriers, preventing accidental click-through to covered windows.
  Control Center adds
  bilingual AI Workspace creation, status, pause/resume, and confirmed
  revocation controls.
- IPC protocol version 3 adds the `realm` capability, connection-bound leases,
  `GetRealms`, synchronous optimistic Realm actions and receipts,
  `InjectRealmInput`, `LaunchInRealm`, `CaptureRealm`, Realm events, explicit
  Realm scope axes, and the owner-only `ass-ctl-realm-admin` recovery scope.
  The ordered mutation journal now records both commands and synchronous
  Realm actions with real connection ids and before/after authority
  revisions; capability, lease, validation, scope, and live-state refusals
  are retained as decisions. Interaction-group operations expand scope checks
  to every affected sibling window.
  `ass-ctl` adds `realms` and the `realm-create`, `realm-pause`,
  `realm-resume`, `realm-transfer`, `realm-launch`, `realm-capture`, and
  `realm-revoke` commands.
- Each sandboxed application receives a mount-scoped Wayland listener and
  only its Realm's `wl_seat`. The randomized host pathname is unlinked and
  every pre-gate connection is dropped before application execution, while
  the private mount supports multiple Wayland connections from one
  multi-process application instance. Agent input uses target-local
  coordinates without moving physical focus, modifiers, grabs, selection,
  drag-and-drop, text input, or compositor shortcuts.
- Realm applications launch through a fail-closed bubblewrap policy with
  user, mount, PID, IPC, UTS, cgroup, and network isolation, no Linux
  capabilities, an ephemeral home, no host network or user files by default,
  a private Realm portal, and GPU render nodes without KMS card nodes.
  Mandatory cgroup v2 memory, process, and CPU controls are installed under a
  controller-delegated systemd user service. Realm pause, session lock, and
  inactive VT freeze the complete cgroup; resume continues it; revocation and
  compositor shutdown use `cgroup.kill` and reap it. `[realm_sandbox]` adds
  default and per-desktop-entry network, canonical path, and resource policy;
  changes apply to new launches.
- Directed Realm capture renders only the Realm surface graph into a bounded
  offscreen target. Optimistic revisions correlate pixels with authority
  state; a monotonically increasing security generation rejects in-flight
  work across lock, seat revocation, pause, and quick relock/unlock
  transitions. Final file/IPC delivery is authorized on the compositor thread
  after background encoding. `RealmDamaged` events expose bounded,
  virtual-output-local conservative damage for every surface or topology
  change, so observers do not poll. Screenshot and Realm-capture files use
  mode-`0600` atomic replace instead of exposing partial PNGs. IPC capture
  metadata is followed by a fully sealed PNG `memfd` over `SCM_RIGHTS`, which
  removes base64 expansion and correlates Realm pixels, logical region,
  window placements, target-local sizes, scale, and authority revision.
  The IPC writer rechecks the live scope, lock/VT security gate, Realm state,
  authority revision, and lease immediately before attaching that descriptor.

### Frame-time fixes (optics + compositor)

- Smoother frames under client updates: flux texture/buffer uploads
  (image create, `update_region`, mesh/buffer create, batched flush, and
  the dma-buf acquire-fence transition) no longer block the calling
  thread on a GPU fence — submissions are deferred and recycled lazily
  once their fence signals, so a client commit stops collapsing the
  render pipeline into a wait for all prior GPU work. On the compositor
  side, shm commits now copy only the damaged rows onto the retained
  snapshot (reusing its allocation) and same-size frames upload only the
  damage bounding box instead of the whole texture; the animation loop
  waits on the event queue with a deadline instead of a blind
  `thread::sleep` so input is processed as it arrives; a `begin_frame`
  fence/acquire timeout now skips the frame instead of rebuilding the
  swapchain; and on the nested backend, retired client buffers are
  released a few frames late instead of stalling on `device.wait_idle()`.

### Overview, window transitions, and pixel capture

- Bare-metal polish: the software cursor on DRM now loads real XDG cursor
  themes (`$XCURSOR_THEME`/`$XCURSOR_SIZE` or `[ui] cursor_theme/cursor_size`
  for sessions without them, `index.theme` inheritance, nearest-larger size
  selection, animated files pinned to their first frame), with the
  hand-drawn glyphs kept only as a no-theme fallback; `Ctrl+Alt+Fn`
  switches virtual terminals through libseat, matching the console behavior
  users expect while testing (no-op nested); VT resume now closes and
  re-opens the GPU through the seat instead of failing with
  `drm/kms error: permission denied` on the revoked fd, and the present and
  event-read paths treat a masterless fd during the switch window as a
  transient frame skip instead of a fatal error; per-connector scale
  overrides apply to the internal panel the same way as to external
  monitors, and the configured scale now drives the actual render scale,
  logical layout extent, and icon decode — not just the `wl_output`
  advertisement; pointer, touch, and tablet absolute coordinates now
  convert from the backend's native space (physical on KMS) into the
  compositor's logical space in one place in the main loop, so a
  configured scale no longer strands input in a physical-pixel dead zone;
  frame pacing no
  longer stalls on periodic work — the system-status probe (wpctl
  fork+exec) and the application/icon rescan moved off the compositor's
  frame thread onto helper threads; per-connector scale overrides apply to
  the internal panel the same way as to external monitors.

- Unified overview (M9): `Super+O` or `ass-ctl overview` opens a modal
  window/workspace picker — live window thumbnails on a shared grid, a
  workspace rail on the left, title labels, click a thumbnail to focus it,
  click a rail tile to switch workspaces, Escape or click-away to dismiss.
- Declarative window transitions (ADR-0029): non-interactive geometry
  changes (tiling layout, IPC `set-geometry`) interpolate position and size
  over 150 ms with an ease-out curve; subsurface trees move with their
  root. The window model, chrome, and IPC always report the target rect,
  and `[ui] reduced_motion` resolves every transition in one frame.
- Screenshots and scoped pixel capture (ADR-0041): `ass-ctl screenshot
  [path.png]` writes the focused output as a PNG, and the new IPC
  `Request::CaptureOutput` returns a sealed PNG `memfd` to scoped
  agents (explicit `CaptureOutput` op, never inherited; refused while the
  session is locked or the seat is inactive). Captures now copy the exact
  output frame being submitted, including its current client buffers,
  wallpaper frame, chrome, and software cursor; later scene changes cannot
  alter the immutable snapshot. This works on both nested and DRM/KMS
  backends and shows the overview grid when it is open. Default captures
  now use the XDG user Pictures directory's
  lowercase `screenshots` subdirectory, and logical capture regions are
  converted to physical pixels so HiDPI captures match the displayed region.
  Readback staging is preallocated, GPU completion is polled across frames
  instead of waited synchronously, and PNG compression and file writes run
  on a bounded capture worker instead of pausing the
  compositor frame thread. Overlapping capture requests are refused rather
  than stacking full-frame jobs.
- Per-output scale policy (ADR-0028): `[[output]]` config entries override
  the backend-reported scale per connector for mixed-DPI setups, applied
  live on reload.
- flux gains on-demand exact-frame readback
  (`Frame::request_readback`, `Surface::prepare_readback`, and
  `Surface::read_pixels_ready`) plus `flux_surface_readback_desc` for
  always-readable offscreen surfaces, and lens gains
  `lens_set_reduced_motion` so every eased widget value resolves in one
  frame under the policy.

### Dock pinning and frosted glass

- The dock now separates pinned applications from transient running ones
  with a divider: pinned apps stay on the left, unpinned running apps appear
  on the right and disappear when their last window closes. Right-click a
  tile and choose `Keep in Dock` / `Remove from Dock` to manage pins from
  the desktop; the choice is written back to `[dock] pinned` (with
  `autopopulate = false` so an emptied list stays empty).
- The dock bar, its app-name tooltip, the application menus, and the
  launcher's search panel now share one frosted-glass material — a light
  translucent tint over the compositor's backdrop blur with a bright edge —
  replacing the opaque dark bubbles.
- Dock hover polish: the running-indicator dot is centred in the flat strip
  between the icon baseline and the panel bottom so it can no longer fall
  into the rounded corners and outside the bar, and the magnification
  spring damping was raised to remove the visible jitter at variable frame
  times while keeping the macOS-style bounce.

### Per-output display policy

- `[[output]]` entries grow from scale-only into a full per-connector
  display policy (ADR-0028): `mode = "WxH[@Hz]"` requests a display mode,
  matched against the connector's advertised modes at modeset time with a
  preferred-mode fallback and a log warning when nothing matches — a changed
  mode for an already-connected monitor queues a safe live re-modeset;
  `position = { x, y }` places
  the output in the global logical layout; `primary = true` picks the
  focused output. Scale, position, and primary apply live on reload, and a
  policy removed from the file now reverts to the backend-reported value
  instead of lingering in the live output set. `transform` is parsed and
  validated (`normal`, `90`, `180`, `270`, `flipped`, …) but logs a
  deferral warning until renderer output-transform support lands.
- `ass-ctl outputs` lists the modes each connector advertises with the
  live one marked, and the IPC `GetOutputs` reply carries them as the
  additive `available_modes` field (serde-default, no protocol bump).

### Direct-display backend, session lock, and production hardening

- New DRM/KMS backend: ass drives display hardware directly from a bare TTY
  with atomic modesetting, GBM-less dma-buf scanout of Flux offscreen images,
  libinput input (pointer, keyboard, touch, touchpad gestures, tablet tools
  with pressure/tilt), libseat session and device ownership, VT switching,
  and udev hotplug with per-connector output restore. `--backend auto|drm|nested`
  (or `ASS_BACKEND`) selects the presentation target; `auto` nests under an
  existing Wayland session and drives KMS on a TTY.
- Session lock (`ext-session-lock-v1`) and idle management
  (`ext-idle-notify-v1`, `zwp-idle-inhibit-v1`): fail-closed locking with a
  secure-frame confirmation, lock surfaces per output, and idle notifications
  that honor inhibitors only while their window is visible. While locked,
  input focus, chrome shortcuts, and IPC mutations are refused.
- Explicit synchronization (`zwp_linux_explicit_synchronization_v1`): dma-buf
  client buffers carry acquire fences through Flux import and KMS `IN_FENCE_FD`,
  and `wl_buffer.release` is deferred until the presented frame retires.
- Tablet protocol (`zwp_tablet_manager_v2`): tool announce, proximity, full
  axis routing (pressure, distance, tilt, rotation, slider, wheel), tip and
  button events with click-to-focus, and pointer emulation for clients that
  do not bind the tablet protocol.
- `xdg_toplevel.set_window_geometry` frame insets are honored end to end:
  client-decorated windows place, hit-test, and receive input by their
  visible window rect, excluding shadow margins.
- Nested subsurfaces render and receive input correctly: subsurface trees
  walk recursively with per-node stacking, and pointer input routes to the
  topmost surface in the tree rather than always to the root toplevel.
- `[layout] default_tiled` starts new workspaces tiled; transient dialogs
  always float, even on tiled workspaces and during the tiling sweep.
- `[ui] reduced_motion` accessibility switch: every chrome and lens
  transition (dock magnification, launcher reveal, fades, slides) resolves
  in one frame, live on config reload.
- DRM backend resilience: the present path tolerates transient flip timeouts,
  VT-switch and hotplug races (re-modeset on resume, surface recreation when
  the modifier set changes), and exits cleanly on GPU removal instead of
  spinning.

### Animated 3D wallpapers

- The default procedural wallpaper now includes a depth-tested torus-knot
  glTF layer with an orbiting camera, moving directional light, and animated
  specular highlights. `ASS_WALLPAPER` accepts a model-only `.glb`, while
  `ASS_WALLPAPER_MODEL` overlays a `.glb` on an image or video wallpaper.
- The model renderer auto-frames scene bounds and owns one depth target per
  frame-in-flight slot. The launcher now captures image/video, 3D, and client
  layers into one quarter-scale offscreen scene and applies a frame-slot-safe
  Dual-Kawase blur every frame, so model motion and lighting stay live without
  device-wide synchronization stalls or GPU-watchdog-scale Gaussian kernels.
  Animated rendering is capped at 60 frames per second. Captures normalize
  BGRA swapchains to RGBA8 storage, and render-target transitions stay in the
  owning frame, avoiding Intel i915 hangs from invalid storage formats and
  nested one-shot submissions.

### Daily-use reliability and interaction polish

- Restored compatibility with flux's typestate frame API, so the workspace
  builds against the current renderer bindings again.
- Updated the Quick Start and `scripts/env.sh` for the unified `../optics`
  Meson build. The script now makes library test harnesses find both shared
  libraries as well as configuring the binding build directories.
- `ass-ctl` now prints query and command output instead of discarding it.
  Added `notifications`, `journal [since]`, `switch-to`, and
  `subscribe-journal`; local help also works without `XDG_RUNTIME_DIR`.
- Shell overlays own their pointer regions. Clicking or scrolling the dock,
  launcher, workspace bar, notification stack, or decorations no longer
  leaks input to a client underneath. Clicking a notification dismisses it.
- Floating windows resize from an 8-pixel inside border. Edge and corner
  grabs honor size hints and publish the xdg-shell `resizing` state. Focusing
  a window raises it, and hidden-workspace windows no longer participate in
  pointer hit-testing.
- Borderless window controls now include `Super` + left-drag to move and
  `Super` + right-drag to resize from the nearest edge or corner. Layout-owned
  windows become floating before either gesture, and compositor-owned resize
  cursors identify invisible borders and active grabs.
- Right-clicking a launcher or Dock application opens a shared context menu
  that lists every matching window and offers focus/restore, open/new window,
  minimize, and graceful close actions. The compact menu is anchored to its
  application tile instead of the pointer, and the Dock freezes its current
  magnification while the menu is open. Dock application names appear above
  their animated icons after a short hover. Multi-window minimize and close
  actions are journaled once per toplevel.
- Added the scoped IPC `Minimize { id }` command and
  `ass-ctl minimize <id>`; compositor chrome uses the same command path as IPC
  so minimization remains observable in the mutation journal.
- Added deterministic `SetWindowGeometry` IPC control and
  `ass-ctl set-geometry`, using logical coordinates and client size hints
  instead of a simulated pointer grab. Added a separate, named-scope-only
  `input` capability for target-local pointer moves, clicks, scrolls, and key
  presses. Synthetic input validates the full batch, refuses hidden,
  overlapping, or shell-covered targets, bypasses global bindings, and records
  live-state refusals in the mutation journal. Pixel capture remains deferred.
- The application catalog refreshes every five seconds, dock pins update on
  config reload, and SVG icons render through `rsvg-convert` when available.
- Application discovery now follows Flatpak-exported desktop-file symlinks,
  honors `Hidden`, `OnlyShowIn`, `NotShowIn`, executable `TryExec` checks, and
  XDG base-directory precedence. Relative XDG paths are ignored, and explicit
  `XDG_DATA_DIRS` values are no longer silently extended with system defaults.
- Icon lookup now implements `index.theme` directory, scale, size-range, and
  recursive inheritance rules, followed by `hicolor` and unthemed pixmap
  fallback. The compositor uses `ASS_ICON_THEME` when set, otherwise the host
  GTK icon theme, and refreshes cached textures when a theme, output scale, or
  symlink target changes.
- The launcher is now a full-screen, responsive application library with a
  live multi-resolution-blurred desktop, spring opening/closing motion, search,
  keyboard grid navigation, wheel/trackpad paging, and access to the complete
  application catalog instead of a fixed render cap. HiDPI icon lookup now
  targets 128 pixels, with stable colored initial tiles for missing icons.
- Configuration rejects unknown fields and invalid layout ranges. Removing
  the config restores default layout parameters instead of retaining stale
  values.
- Named IPC scopes from `[[agent.scope]]` are now enforced at handshake and
  command dispatch. Explicit unknown names are refused, and scope changes or
  removals made by hot reload apply to existing connections. The IPC socket is
  created with mode `0600`, refuses to replace non-socket paths, and cannot be
  stolen from a running server by a second instance.
- The declared minimum Rust version is now 1.88, matching `image 0.25.10`
  instead of promising an unbuildable 1.74 toolchain. Security-fixed lockfile
  versions include `crossbeam-epoch 0.9.20`, `anyhow 1.0.103`, and
  `memmap2 0.9.11`.
- `LaunchOpts::foreground` now waits for the child and reports nonzero exits as
  errors. Its tests use a headless command instead of launching the host's
  graphical terminal.

### ass-ctl --json
- `ass-ctl` accepts a global `--json`/`-j` flag: the query commands
  (`windows`, `workspaces`, `outputs`, `notifications`, `journal`) then print
  machine-readable JSON (serialized straight from the IPC types) instead of
  human text, so scripts and the agent can parse the output. Control commands
  keep their text ack.
  ass-ctl gained a `serde`/`serde_json` dependency and enables ass-core's
  serde feature. Loopback-tested.

### ass-ctl subscribe: stream server events
- `ass-ctl subscribe` connects, subscribes, and prints each server-pushed
  event as a line until the connection closes — making the IPC's event
  surface (WindowsChanged / WorkspaceChanged / Notified) consumable from the
  shell for scripts and the agent. A pure `format_event` helper formats each
  variant (unit-tested); the streaming loop is thin glue over the client.

### Dismiss notifications
- `NotificationQueue::dismiss(id)` removes a notification by id (returns
  whether it was present), mirroring a user "dismiss" before the TTL. The
  IPC `DismissNotification { id }` command (control), `ass-ctl dismiss <id>`,
  a main-loop drain, and toast click-to-dismiss wire it up. Unit-tested in
  `ass-core::notify`.

### GetOutputs: query the live output list
- New `GetOutputs` IPC query and `ass-ctl outputs` expose the live outputs
  (connector + geometry), completing the introspection surface — windows,
  workspaces, notifications, and outputs are all queryable. A new
  `ass_core::output::OutputInfo` pairs the connector with its geometry; the
  server's `output_infos` builds it and the IPC handler mirrors it. Loopback-
  tested.

### Per-workspace tiling
- Tiling is now per-workspace (ADR-0024), not a single global flag. Each
  workspace remembers whether it is tiled, so one workspace can tile while
  another floats, and the state persists across switches. `Workspace` and the
  IPC snapshot carry a `tiled` flag; `set_tiling`/`ToggleTiling` flip the
  *current* workspace; `apply_tiling` tiles only when the current workspace
  is tiled. The server's global `tiling` bool is gone. Unit-tested in
  `ass-core::workspace`.

### IPC: move a window to a workspace
- New `MoveToWorkspace { window, workspace }` IPC command (control) and
  `ass-ctl move-to <window> <workspace>` move a toplevel to a workspace at
  runtime — the script/agent analogue of the map-time window-rule
  assignment (ADR-0025). Backed by `Server::move_to_workspace`, which routes
  through the workspace model and drops focus if the window leaves the
  visible set.

### Chrome-aware tiling work-area
- Tiled windows no longer render under the dock. The `Chrome` trait gained a
  `reserved() -> Reserved` edge API (default none); the Dock reserves the
  bottom edge (`DOCK_HEIGHT + margin`). The Shell aggregates every
  component's reservation, and `apply_tiling` now tiles into the output's
  logical rect inset by those edges (`Reserved::inset`, unit-tested). The
  server gained `output_logical_rect` and `apply_tiling` again takes a
  work-area; the binary computes the chrome-aware rect each frame.

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
