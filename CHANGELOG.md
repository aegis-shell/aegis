# Changelog

Notable user-visible and contributor-visible changes to ass. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once the
project cuts a tagged release.

## Unreleased

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
