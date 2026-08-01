# Changelog

Notable user-visible and contributor-visible changes to Aegis. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once the
project cuts a tagged release.

## [0.0.9] - 2026-08-01

### Liquid glass

- Redesigned the analytic liquid-glass material after Apple's WWDC25 design
  language: a convex-lens rim model with magnifying refraction and chromatic
  dispersion confined to the rim band, a thin directional key light instead
  of the old broad white rim wash, luminance-adaptive pearl/smoke body tint,
  shadow-side darkening and a transmitted-light trough. The Dock's painted
  foreground layer is now minimal and borderless; the glass rim supplies the
  edge definition.
- Backdrop captures for glass regions now render at full physical
  resolution instead of quarter resolution, eliminating the low-resolution
  smear behind glass bodies.

### Lock screen

- Added animated VRM lock-screen avatars. Companion VRM Animation 1.0 clips
  retarget onto VRM 0.x or 1.0 humanoid rigs, use GPU skinning, and retain a
  head-tracking portrait crop without per-frame texture readback.

### Wallpaper

- Added explicit `image`, `video`, `3d`, and `parallax` wallpaper modes under
  the hot-reloaded `[wallpaper]` configuration table. Parallax scenes accept
  two to eight back-to-front image planes with normalized relative depth,
  displacement, and settle-time controls.
- Added continuous, frame-rate-independent pointer parallax (ADR-0092).
  Targets update only on exposed wallpaper, so crossings behind client
  windows and shell chrome interpolate between the two visible samples
  instead of snapping. Cover overscan prevents exposed edges, and
  `reduced_motion` centers and disables the effect.
- Added an original three-plane Alpine example, including alpha-separated
  assets and a ready-to-run source-checkout configuration.

### Agent capability broker and MCP production baseline

- Promoted the compositor IPC to the native agent capability broker and kept
  MCP as a replaceable northbound protocol adapter (ADR-0090). Agent
  Realms now bind to authenticated principals; lifecycle, capture, launch,
  input, and recovery reject cross-principal Realm access even when labels
  collide.
- Added stable connector instance ids, live ceiling reauthorization,
  dedicated owner/Realm/agent-admin scopes, default-on lockdown, and strict
  dangerous-operation ceiling validation. Authorization and bridge identity
  files now use crash-durable atomic replacement and memory rollback on
  persistence failure.
- Implemented only stateless MCP `2026-07-28`: side-effect-free
  `server/discover`, per-request metadata, cache hints, and complete-result
  envelopes, with no connection-level negotiation or version downgrade.
  Connector instance ids are explicit and required. The bridge uses standard
  stdio EOF shutdown, refreshed tool catalogs, aligned consent timeouts, and
  bounded frame handling. Managed Realms persist across normal connector
  restarts by default.

### Unified native command surface

- Removed the separate `aegis-cli` executable. `aegis` with no subcommand, or
  `aegis run`, starts the compositor; resource commands such as
  `aegis display`, `aegis window focus 42`, and
  `aegis workspace switch next` operate a running session through the same
  versioned IPC boundary.
- Grouped queries and mutations under display, window, workspace,
  notification, journal, Realm, permission, and system domains. Resource
  nouns list by default, `aegis events` streams session changes, and shell
  completions now target the unified executable.
- Added the flux-free, lib-only `aegis-commands` crate for parsing, IPC
  dispatch, formatting, and loopback tests. Renamed the first-party recovery
  scopes to `aegis-owner-admin`, `aegis-realm-admin`, and
  `aegis-agent-admin`.

### Idle and locking

- Added two session controls to the command panel's System section
  (`Super+S`): Lock Now locks the session immediately through the same path
  as `Super+L`, and Always On holds a session-wide idle inhibitor that
  keeps automatic dimming, locking, and display power-off suspended until
  switched off. Always On is a runtime toggle owned by the compositor; it
  is not persisted and folds into the same effective inhibitor flag as the
  connection-scoped IPC inhibitors, so neither source can clear the other.
- Added the development-only `[dev] allow_quit_while_locked` escape hatch
  (default `false`): while the session is locked, only the `quit` binding
  (`Super+Ctrl+Q` by default) still matches, so a wedged lock screen cannot
  trap a development session. This option is planned for removal before
  release; every other binding stays swallowed while locked.

### Shell and input

- Replaced the interactive top status bar with display-only HUD status
  chips (ADR-0080): system status (network, Bluetooth, battery), the
  StatusNotifierItem tray row, the clock, and the notification count on the
  left, and workspace dots in the center. The chips reserve no space, so
  tiled and maximized windows now occupy the full output; they accept no
  pointer input, so clicks fall through to windows even on tray icons; and
  each chip fades out while the cursor approaches it, honoring the
  reduced-motion policy. Fullscreen auto-hide is preserved.
- Added the command panel, a full-screen modal overlay in the Sword Art
  Online menu language — frosted white floating panels with an amber accent
  over the standard dark blurred scrim. It hosts the interactions the HUD
  gave up: quick settings (volume, brightness, Wi-Fi, Bluetooth,
  do-not-disturb, tiled layout), tray activation with host-rendered
  dbusmenu context menus, and notification dismissal. Open it with the new
  `Super+S` binding (configurable as the `command_panel` action in
  `[[keybind]]`) or a four-finger touchpad swipe down; close with the same
  binding, Escape, a scrim click, or a four-finger swipe up. Four-finger
  swipes are now compositor-owned and no longer forwarded to clients.
- Added configurable touchpad swipe bindings (ADR-0082). New defaults: a
  three-finger horizontal swipe switches to the next or previous
  workspace, and a three-finger vertical swipe cycles the current
  workspace's windows through the live window switcher until the gesture
  ends; the four-finger command-panel swipe is unchanged. A swipe latches
  its axis at 30 px and fires one step per 120 px of travel. Finger count
  and axis pairs rebind or disable through new `[[gesture]]` entries in
  `config.toml` (`workspace_switch`, `window_cycle`, `command_panel`,
  `none`), hot-reloaded alongside `[[keybind]]`. Three-finger swipes are
  now compositor-owned and no longer forwarded to clients, and gesture
  commands are journaled under the new `Origin::Gesture`.
- Split the StatusNotifierItem tray service into a shared `aegis-tray`
  crate, spawned once by the compositor and consumed read-only by the HUD
  and read-plus-command by the command panel.
- Renamed the compositor chrome crates and their user-facing identifiers
  (ADR-0081). `aegis-statusbar` is now `aegis-hud`, and its configuration
  table is renamed `[statusbar]` → `[hud]` with the same `enabled` key and
  no legacy alias. `aegis-sao-panel` is now `aegis-command-panel`, and the
  keybinding action is renamed `sao` / `sao_panel` → `command_panel`
  (aliases `commandpanel`, `panel`); the default binding stays `Super+S`.
  Update `config.toml` accordingly — the old names are rejected. The `Sao`
  design tokens in `aegis-design` keep their name as the internal codename
  of the panel's visual idiom.
- Reworked notification presentation (ADR-0083). Toasts are now a
  frameless, display-only strip at the top-right — plain floating text with
  no background panel — that captures no pointer input and disappears after
  three seconds; click-to-dismiss on popups is gone, and dismissal lives in
  the command panel's Messages section. Notification history is decoupled
  from popup lifetime: the shared queue retains entries for one hour (was
  five seconds), so the Messages list, the HUD bell count, and IPC queries
  keep working after a popup fades. The HUD's right chip is removed (the
  clock and bell moved into the left chip), and the Agent Workspaces status
  moved from the HUD to a display-only status row in the command panel's
  System section.
- Added per-window always-on-top (ADR-0084). The Dock context menu gains an
  `Always on Top` / `Not Always on Top` row next to Maximize/Restore that
  keeps the application's activated window above every normal window but
  below compositor chrome; the row targets the same window as the Maximize
  row and is unavailable for read-only mirrors and minimized or fullscreen
  windows. The flag is compositor-internal and session-scoped — no
  `xdg_toplevel` state, no configuration key, no keybind, and no
  `window_rule` action. Scripting uses the new `SetAlwaysOnTop
  { id, on_top }` IPC command (op class `SetWindowGeometry`) or
  `aegis window always-on-top <id> <on|off>`.

### Desktop portal

- Added the `org.freedesktop.impl.portal.Secret` v1 backend interface to
  `aegis-portal`, backed by an encrypted at-rest vault under
  `$XDG_DATA_HOME/aegis/secrets` (XChaCha20-Poly1305, first run creates a
  mode-0600 keyfile and unlocks automatically). Sandboxed applications
  retrieve an HKDF-derived portal secret over the request's file
  descriptor; the raw vault master key never crosses D-Bus.
- Added a transitional `org.freedesktop.secrets` (Secret Service API)
  compatibility layer to the same process so un-sandboxed libsecret clients
  keep working until portal-native secret retrieval is universal. It is
  isolated in `secret::compat` and scheduled for removal together with its
  `org.freedesktop.secrets.service` activation file.
- Added the `org.freedesktop.impl.portal.FileChooser` v3 backend interface,
  running the compositor's native file-picker chrome over the new
  user-consent `PickFile` IPC (protocol 13, fail-closed like `PickTarget`,
  no screen capture involved). `OpenFile`/`SaveFile`/`SaveFiles` all map to
  the picker; filters round-trip as the spec's `(sa(us))` structures.
  GTK stays configured as the fallback while the picker proves itself.
- Added the `org.freedesktop.impl.portal.Wallpaper` v1 backend interface.
  `SetWallpaperURI` consents through the compositor's confirmation dialog,
  then decodes and swaps the image live on the compositor main loop (new
  synchronous `SetWallpaper` IPC, protocol 17; glTF stays startup-only).
- Added the `org.freedesktop.impl.portal.DynamicLauncher` v1 backend
  interface. `PrepareInstall` consents through the compositor's
  confirmation dialog before echoing the proposed name/icon with a fresh
  install token; `RequestInstallToken` is always refused, so no install
  bypasses consent.
- Added the `org.freedesktop.impl.portal.Account` v1 backend interface.
  `GetUserInformation` answers only after the compositor's yes/no consent
  dialog (new generic `PickConfirm` IPC, protocol 16) approves sharing;
  the identity comes from `getpwuid` plus the canonical avatar locations.
- Changed the portal routing default from `gtk` to `aegis;gtk`: Aegis is
  now the default backend for every interface, with GTK as the safety net
  for the few it does not implement (Access, Print, Location, Background).
  The frontend only asks Aegis for the interfaces advertised in
  `aegis.portal`.
- Added `pam_aegis`, a PAM module caching the just-verified login password
  to `$XDG_RUNTIME_DIR/aegis-pam-token` (mode 0600, written atomically).
  Password-mode secret vaults unlock silently at login and after screen
  unlock (a watcher in `aegis-portal` consumes and deletes the token);
  the `contrib/pam/aegis-lock` stack includes it as `optional`.
- Added the masked secret prompt (IPC protocol 15 `PromptSecret`):
  password-mode vaults now unlock through compositor chrome. The
  `org.freedesktop.secrets` compat `Unlock` prompts, derives the vault key
  with the Argon2id KDF, and completes the spec's Prompt object; the typed
  password is zeroized after use.
- Added the `org.freedesktop.impl.portal.Notification` v2 backend interface.
  `AddNotification` posts into the compositor's notification queue (toast,
  command panel, HUD) carrying the application's own id as `external_id`;
  `RemoveNotification` matches `(app_id, id)` against the live queue
  snapshot and dismisses the entry.
- Added the `org.freedesktop.impl.portal.AppChooser` v2 backend interface,
  running the compositor's native app-picker chrome over the new
  user-consent `PickApp` IPC (protocol 14, same fail-closed authorization as
  `PickFile`). GTK stays configured as the fallback while the picker proves
  itself.
- Added the `org.freedesktop.impl.portal.Email` v2 backend interface,
  handing compose requests to the session mail client via `xdg-email`
  (`AEGIS_PORTAL_MAILER` overrides it); attachment fds are staged into the
  portal cache directory and passed as `--attach` paths.
- Added the stateless `org.freedesktop.impl.portal.Lockdown` backend
  interface (all flags permissive; no kiosk policy engine exists).
- Fixed every backend interface to serve the spec-mandated lowercase
  `version` property. zbus auto-PascalCased it to `Version`, which made
  `xdg-desktop-portal` skip the Aegis ScreenCast/Screenshot interfaces
  entirely (no frontend interface was ever exported for them).
- Fixed `RetrieveSecret` to deliver the secret over any caller-supplied
  file descriptor. The backend wrote with a socket-only `shutdown(2)`, which
  fails `ENOTSOCK` on the plain pipe that real clients (Chrome's
  `SecretPortalKeyProvider`, libportal) pass, so the master secret never
  reached the app and stored cookies/passwords were undecryptable. A plain
  `write_all` + close now delivers EOF for both pipes and sockets.
- Fixed concurrent secret-vault unlockers to each get their own spec
  `Prompt` object and complete from a single compositor interaction. The old
  single-flight `is_unlocking` flag returned a shared `/…/prompt/pending`
  placeholder to all but the first caller, so a second `Unlock` (or a
  `RetrieveSecret`/`CreateCollection` arriving mid-prompt) never received a
  `Prompt.Completed` signal and hung. A shared unlock coordinator now queues
  every caller behind one prompt worker that prompts once and completes the
  whole batch.
- Fixed `RetrieveSecret` on a locked (password-mode) vault to prompt for the
  vault password and then deliver the derived secret, instead of failing
  outright with `IsLocked`. Portal response code 1 (cancelled) is reported
  when the user dismisses the unlock prompt.
- Fixed `CreateCollection` on a locked vault to queue behind the same unlock
  prompt and create the collection once unlocked (replying with the spec
  prompt path), so sandboxed clients that create their default collection up
  front no longer see an empty `/` path and a silent no-op.
- Fixed `Item.Delete`/`Collection.Delete` to drop the deleted object from
  the bus, not just tombstone it in the search index. Stale paths now fail
  on later calls instead of returning the deleted item's secret.

### Graphics and performance

- Direct DRM outputs now derive their default scale from the selected mode
  and the connector's validated EDID physical dimensions. Internal
  eDP/LVDS/DSI panels and external displays use viewing-distance-aware target
  densities, rounded to quarter-step fractional scales; missing or
  implausible dimensions fall back to 100%, and an explicit `scale` in
  `[[output]]` remains authoritative.
- Added linux-dmabuf v4 feedback backed by the DRM identity of the Vulkan
  physical device selected by Flux. Mesa OpenGL clients, including Flatpak
  applications, can now select the compositor GPU instead of falling back to
  `llvmpipe`.
- Cache each member of a client's reusable dma-buf swapchain by stable buffer
  identity and wait new acquire fences on the existing Flux image. Repeated
  commits no longer re-import the same two to four GPU buffers.
- Restrict direct scanout to the formats and modifiers accepted by both Flux
  and KMS, and use the KMS out-fence as the buffer-release completion fence.
- Replace distributed redraw flags and blocking page-flip waits with an
  explicit presentation-domain state machine. Input and Wayland requests stay
  live while KMS owns a frame, redraws coalesce until every CRTC flips, and
  callback-only commits no longer force empty GPU submissions.
- Pace video wallpapers from an absolute real-time 24 fps source clock.
  Unrelated high-frame-rate clients no longer advance video playback or turn
  every client event into full-output wallpaper damage.

### Reliability

- Unified observability across every first-party process through a shared
  tracing-based subscriber. `RUST_LOG` now controls `aegis` compositor and
  command modes, `aegis-idle`, `aegis-lock`, `aegis-portal`, and
  `aegis-settings` with one filter syntax (default `info`, `warn` for
  one-shot management commands);
  `AEGIS_LOG_FORMAT=json` switches to machine-readable output, and per-request
  traffic that previously ran at `info` is downgraded to `debug`. See ADR-0079.
- Distinguish powered-off scanout, a temporarily absent output target, and
  backend/VT loss in the presentation state machine. Input remains live for
  wake and hotplug, stale input ownership cannot cross a later VT loss, and
  visual work from a targetless interval is not replayed on resume.
- Embed the default static wallpaper in the executable and reject missing
  wallpaper paths before attempting image decode. Installed builds no longer
  repeatedly try to open a build-tree-only wallpaper path.
- Make live-system IPC controls return the compositor main loop's
  authoritative apply result instead of only acknowledging that a command was
  queued. Policy clients can now retry refused hardware transitions without
  inventing local state.

### Session security and power

- Added the first-party `aegis-lock` multi-output session locker with an
  Aegis glass lock screen, locale-aware clock and date, keyboard-layout and
  Caps Lock feedback, bounded credential entry, PAM authentication, retry
  backoff, and explicit credential-memory clearing.
- Added the supervised `aegis-idle` policy client with ordered backlight dim,
  secure lock, physical display power-off, and logind suspend stages. Sleep
  delay is released only after compositor-confirmed lock readiness; input
  wakes powered-down displays behind the lock.
- Added the available Power Management page in System Settings and the
  persistent `[idle]` configuration table. IPC protocol version 12 adds
  `IdleSettings` to `SettingsSnapshot` and the atomic `SetIdle` action.
- Added `Super+L` as the default lock shortcut. The compositor remains the
  authority for exclusive lock input, idle inhibitors, output power, and an
  opaque fail-closed scene if the locker exits unexpectedly.
- Core packaging now includes `aegis-idle`, `aegis-lock`, and the
  `/etc/pam.d/aegis-lock` service profile.
- Distinguished a genuine wrong password from a misconfigured PAM stack on
  the lock screen. When PAM rejects an attempt without ever prompting for
  the credential — the signature of a missing or deny-all `aegis-lock`
  service profile — the screen now reports *Authentication misconfigured ·
  Install the aegis-lock PAM profile* instead of looping *Incorrect
  password*, which previously made every password look wrong.
- Added the `aegis-avatar` crate, an independent library for user-avatar
  loading and rendering modelled on `aegis-wallpaper`. It resolves the avatar
  from XDG-conformant locations (reusing `aegis-desktop-entries`, never
  hand-rolling `$XDG_DATA_HOME`), and produces a single circle-masked GPU
  texture. Still images (PNG, JPEG, WebP, GIF, BMP, ICO, TIFF, TGA, QOI, PNM)
  are cover-fit, analytically circle-masked, and premultiplied. VRM models
  load through `flux-scene-graph` and render offscreen into the same circular
  texture, with honest `AnimationSupport` reporting since skins/morph/animation
  are not yet in the scene graph's supported subset. See ADR-0080.
- Added user-avatar support to the lock-screen identity orb via `aegis-avatar`.
  The canonical location is `$XDG_DATA_HOME/aegis/avatars/face.*` (per the
  Aegis namespace, ADR-0066), with `~/.face` / `~/.face.icon` searched for
  compatibility. Fixed the orb's gradient leaking square corners past the
  circular keyline. VRM models render as posed 3D figures.
- Sharpened the lock wallpaper: raised the atlas cap from 2048 to 3840 px
  and reduced the defocus blur from σ 14 to σ 6 to remove the smeared look
  on high-DPI and ultrawide panels.

### System shortcuts

- Changed the default graceful-quit shortcut from `Super+Shift+Q` to
  `Super+Ctrl+Q`. `Super+Q` still closes the focused toplevel, so the new
  binding avoids the shifted-uppercase key path and frees `Super+Shift+Q`.
  The alternate `Super+Shift+Return` quit binding is unchanged.

### Agent integration

- Agent-launched windows now appear on the physical desktop as guarded
  read-only observer mirrors by default (ADR-0091), while the Agent Realm's
  independent seat remains their only input authority. A persistent subdued
  overlay names the controlling Realm, paused mirrors remain protected, and
  hovering anywhere in the window uses the standard `not-allowed` cursor.
  Physical pointer, keyboard, touch, tablet, focus, and window-control paths
  remain blocked in the compositor; guarded mirrors also prevent
  click-through to lower windows. Agent-only groups without a human observer
  stay absent from physical presentation and window snapshots.
- Added agent capability borrowing and runtime grants (ADR-0088), replacing
  config-declared `[[agent.scope]]` entries — which are removed and now
  rejected as unknown fields — with a compositor-held principal registry.
  Agents self-declare at the handshake; first contact opens a
  capability-checklist pairing prompt in compositor chrome, and the approved
  set becomes the principal's ceiling. The compositor issues a pairing
  credential the bridge persists in its data directory (`AEGIS_MCP_DATA_DIR`,
  default `$XDG_DATA_HOME/aegis-mcp`), so restarts and upgrades never
  re-prompt; a look-alike label on a different installation triggers a
  warning. A platform-owned dangerous set (closing windows, Realm capture,
  Realm input, Realm lifecycle, sandboxed launches) always prompts on first
  use with *Deny / Allow once / Allow session / Always allow*; persisted
  grants live in `$XDG_DATA_HOME/aegis/grants.json` and are journaled along
  with pairings, revocations, and renames. Manage principals with the new
  `aegis permissions` command or the Agent Workspaces application; the
  new `[agent] lockdown` configuration option strips privileged capabilities
  from unpaired connections. IPC protocol version 18 carries the pairing
  handshake, runtime-grant decisions, agent-management requests, and
  authorization journal events; `AEGIS_MCP_SCOPE` is removed and
  `AEGIS_MCP_LABEL` is added, with no compatibility aliases (ADR-0066).
- Split the scoped MCP platform bridge out of `aegis-fuji` into the
  standalone `aegis-mcp` crate (ADR-0087): the bridge is now the platform's
  standard agent access point rather than a fuji component, and the
  `aegis-fuji` crate keeps only fuji's agent runtime. The bridge binary is
  renamed `aegis-fuji-mcp` → `aegis-mcp`, its environment variables
  `AEGIS_FUJI_*` → `AEGIS_MCP_*` (no aliases), its state directory moves to
  `$XDG_RUNTIME_DIR/aegis-mcp/` (old Realm recovery records do not
  migrate), and the `realm_transfer_window` tool's `target` value `fuji`
  becomes `agent`. fuji's default MCP configuration spawns `aegis-mcp`
  with no environment overrides; pairing (ADR-0088) replaces the declared
  scope entirely.

## [0.0.8] - 2026-07-29

### Desktop portal

- `aegis-portal` is now an independent system subpackage with a private
  `/usr/lib/aegis/aegis-portal` executable. The core compositor package no
  longer owns portal activation metadata or directly requires the portal
  frontend and backend-only runtime dependencies.
- Corrected the Screenshot, ScreenCast, Request, and Session D-Bus backend
  method contracts to use the frontend-supplied object paths and synchronous
  `(response, results)` replies. ScreenCast now advertises version 3,
  supports monitor and window selection through the compositor picker, and
  advertises only the hidden cursor mode.
- Removed the incomplete Background backend, automatic background/autostart
  grants, and private ScreenCast restore-token store. Unsupported interfaces
  now route to the GTK backend; persistent ScreenCast grants remain
  unavailable until a compatible PermissionStore policy exists.
- Fixed Inhibit to use the standard idle flag (`8`), reject unsupported
  session-manager flags, bind each inhibitor to its backend Request object,
  and renew its scoped IPC lease while active.

### System shortcuts

- The full Applications launcher now uses `Super+A`. A bare `Super` tap and
  `Super+Return` no longer open it.
- Added `Super+Space` for Prism, a new compact Spotlight-style application
  search surface with live filtering, keyboard and pointer selection, and
  start-or-focus behavior.
- `Super+Shift+Q` now gracefully quits the current Aegis instance, providing
  a direct exit path during VT/DRM testing. `Super+Shift+Return` remains
  available as an alternate binding.
- Global binding matching now normalizes ASCII letter keysyms, so configured
  combinations such as `Super+Shift+Q` match the uppercase keysym produced by
  XKB.
- Added a dedicated system-shortcut reference covering global keyboard,
  pointer, quit, and direct-display VT controls.

### Desktop preferences

- System Settings now provides an editable Appearance page for color scheme,
  optional accent color, contrast, reduced motion, interface fonts, text
  scale, icon theme, and cursor theme and size. One confirmed transaction
  persists and applies the complete profile.
- The compositor now publishes one effective desktop-preference snapshot to
  chrome, cursor rendering, IPC, and portal consumers. It no longer probes
  GNOME GSettings for an icon theme; Aegis config and the documented explicit
  startup overrides have deterministic precedence.
- The Settings portal now exports `color-scheme`, `accent-color`, `contrast`,
  and `reduced-motion`, plus a curated GTK interface compatibility namespace.
  It subscribes to compositor IPC instead of parsing or polling
  `config.toml`, and emits per-key `SettingChanged` signals.
- IPC protocol version 10 adds `DesktopPreferences` to `SettingsSnapshot` and
  the atomic `SetDesktopPreferences` action.

### Application icons

- `[ui] icon_theme` now selects the freedesktop application icon theme used
  by the launcher, Dock, and themed shell symbols. The setting is hot-reloaded
  and discovers themes under `$XDG_DATA_HOME/icons` (normally
  `~/.local/share/icons`) as well as the standard system locations.
  `AEGIS_ICON_THEME` remains the highest-precedence override.
- Periodic application rescans now rasterize icons at the effective
  compositor output scale, including a `[[output]] scale` override, instead
  of falling back to the backend's native scale and replacing HiDPI textures
  with lower-resolution copies.

### Build and dependencies

- Updated the locked Optics Rust bindings and native-library requirement to
  `v0.0.7`, including the Rust 1.85 compatibility and synchronized C/Rust
  version surfaces required by canonical builds.

### Agent Workspaces

- Replaced the generic Fuji-branded status entry with Agent Workspaces. The
  permanent entry now reports no active workspaces, one Realm's own label, or
  a localized multi-Realm summary without claiming that the fuji process is
  online.
- Added an explicit partially-paused aggregate state and renamed manual
  creation to **Create Empty Workspace** because it creates a Realm without
  starting or connecting an agent. The fuji agent and MCP bridge remain
  separate out-of-process components.

## [0.0.7] - 2026-07-29

### Rendering performance

A Flatpak-installed GPU client such as osu! ran at ~100% GPU and was severely
laggy under Aegis relative to niri/KDE/GNOME on the same machine. Three
compositor-side causes were identified and fixed, bringing fullscreen-game GPU
cost in line with other Wayland compositors. (Requires Optics `v0.0.4`, to
which the locked Git dependency now points.)

- **Real dma-buf format/modifier advertising.** The `zwp_linux_dmabuf_v1`
  global previously advertised only `DRM_FORMAT_MOD_LINEAR`, which forced
  clients onto uncompressed, untiled buffers and disabled GPU
  compression/DCC. The renderer now queries the Vulkan device for the set of
  modifiers it can both sample and import (Optics `flux_dmabuf_format_modifiers`)
  and the server advertises them verbatim, with LINEAR retained as a universal
  fallback.
- **Hardware direct scanout of fullscreen client buffers.** A single
  fullscreen, opaque, dmabuf-backed toplevel covering the whole output is now
  page-flipped directly onto the DRM primary plane, bypassing the Vulkan
  composite entirely (zero compositor GPU cost). The candidate bar is fully
  conservative — any transform, clipping, mismatched size, visible software
  cursor, or shell chrome disqualifies it and falls back to compositing;
  leaving scanout forces a full redraw so the resumed composite is correct.
- **dmabuf import caching by `wl_buffer` identity.** Imported Vulkan images are
  now cached per surface keyed on the backing `wl_buffer` (plus modifier and
  size), not on `generation` (which was bumped every commit). A buffer-recycling
  client no longer rebuilds a `VkImage` and re-binds external memory every
  frame. Per-frame explicit-sync acquire fences still force a re-import
  (correctness), since a cached image has no slot to re-wait on.

## [0.0.6] - 2026-07-29

### Licensing

- The project source code is now licensed under **MIT** (was Apache-2.0).
- The bundled `Bibata-Modern-Ice` cursor theme under
  `assets/cursors/Bibata-Modern-Ice/` remains a third-party **GPL-3.0-only**
  asset (derived from [Bibata Cursor](https://github.com/ful1e5/Bibata_Cursor)).
  Its `LICENSE`/`NOTICE` in that directory are preserved and govern the asset
  only. The shipped binary is therefore a combined work under
  `MIT AND GPL-3.0-only`; distribution must keep the bundled theme's GPL
  disclosure, which the install manifest stages under
  `/usr/share/licenses/aegis/Bibata-Modern-Ice/`.
- The intent is to replace the bundled cursor theme with original MIT-licensed
  art in a future release, at which point the whole project becomes MIT.

## [0.0.5] - 2026-07-29

### Cursors

- Cursor rendering switched from binary Xcursor parsing to SVG rasterization
  via `resvg`/`usvg`/`tiny-skia`. SVG cursors from the active theme and from
  Wayland clients are rasterized at runtime; see ADR-0070.
- A full Bibata-Modern-Ice SVG cursor theme is now bundled in the binary
  (`assets/cursors/Bibata-Modern-Ice`, embedded via `include_dir`) as the
  default cursor source, so a sane pointer exists even with no
  `XCURSOR_THEME` and a bare TTY.
- **Licensing note:** Bibata-Modern-Ice is GPL-3.0. Its `LICENSE` and `NOTICE`
  are staged under `/usr/share/licenses/aegis/Bibata-Modern-Ice/` by the
  distribution packaging contract; downstream packagers must preserve that
  disclosure. The project code itself remains Apache-2.0.

### Build and dependency acquisition

- Canonical builds now resolve the Optics Rust bindings from the locked
  `v0.0.3` Git release and link system-installed C libraries through
  `pkg-config`, so CI and package builds no longer require `../optics`.
- Cross-repository development keeps an opt-in local Cargo override that can
  be activated once as the ignored project Cargo configuration.
- The full GitHub Actions job now installs Optics before testing Aegis and
  fails on checkout, native build, workspace test, or release-build errors.
- Removed the redundant development-session wrapper and its shell-only test.
  `cargo run --locked -p aegis` is the standard development entry point, and
  `AEGIS_BACKEND` is the only backend override.
- Removed `scripts/install.sh`. The distribution install contract now lives in
  the [Distribution Packaging](docs/dev/packaging.md) guide, which also
  documents the `aegis-portal`, `fuji`, and portal metadata the script
  omitted. Tests that need XDG discovery stage the debug binary and its
  metadata into a throwaway `mktemp` prefix instead of writing to `~/.local`.

### Wayland browser compatibility

- Browser menus now retain the correct popup grab and keyboard-focus owner.
  Firefox `xdg_popup` menus receive complete clicks, while Chromium UI
  bubbles implemented as `wl_subsurface` trees keep keyboard focus on their
  owning `xdg_toplevel` instead of closing before the button action runs.
- Newly mapped, controllable toplevels now receive keyboard focus. Activation
  configures preserve the mapped window size, and remembered application
  geometry applies only to main windows rather than same-application
  dialogs. Unsupported window-state file versions are rejected as a unit
  instead of retaining one-off migration code or replaying ambiguous state.

### Screen casting

- The portal ScreenCast stream now publishes a PipeWire output port.
  Consumers such as OBS can link the node and receive compositor frames
  instead of seeing a source with no video flow.
- ScreenCast sessions and restore tokens are bound to the application id that
  selected them. Another application cannot reuse a discovered session handle
  or restore token.

### Window resize

- Floating windows now expose an eight-logical-pixel outer resize border.
  Corner targets extend 24 logical pixels along both adjacent edges, making
  diagonal resize easier to acquire without consuming application content.

### Screenshots

- Saved screenshots now include the physical-seat cursor by default on
  nested and direct-display backends, including themed and client-provided
  cursor surfaces. Set `[screenshot] include_cursor = false` to omit it;
  output-capture and screencast IPC policy is unchanged.

### Dock interaction

- Maximized windows now collapse the Dock into its local reveal handle by
  default and gain the complete work area without requiring `autohide`
  configuration. Other visible windows collapse it only when they intersect
  its stable resting rectangle.
- Normal Dock hover, click, and pointer-capture bounds now remain fixed to
  the unmagnified panel. Icon animation no longer expands chrome ownership
  into application content, while the visual glass backdrop still follows
  the animated width.

### Configuration and maintenance

- Removed the deprecated `$AEGIS_KEYBINDS` parser and environment override.
  Versioned `[[keybind]]` entries in `config.toml` are the only key-binding
  configuration surface.
- Removed unused protocol handlers, cursor animation state, token-revocation
  scaffolding, and unused ordinary dependencies.
- Split Dock state/layout, rendering, and tests into focused modules, and
  separated status-bar rendering and configuration tests from their runtime
  modules. Popup placement and Unicode-safe label truncation now use one
  shared shell implementation across the Dock, status bar, menus, switcher,
  and feedback surfaces.

## [0.0.4] - 2026-07-28

### Input-method keyboard grab

- The input-method keyboard grab now owns the hardware key stream for its
  whole resource lifetime, not only while a `zwp_text_input_v3` field is
  active. A modifier pressed before a focus-change boundary (Super while
  Super+Tab window switching) is no longer stranded in the input method's
  XKB state, which previously caused foot to swallow subsequent
  composition. The grab forwards unconsumed keys through
  `zwp_virtual_keyboard_v1`.

### Cursor theming

- The software cursor now draws from **SVG** cursor themes instead of the
  legacy Xcursor binary format, following the same freedesktop cursor naming
  and theme-inheritance spec (`$XCURSOR_THEME`/`$XCURSOR_SIZE`, icon roots,
  `index.theme`). SVG is rasterized on demand with the pure-Rust `resvg`, so
  cursors stay crisp at any scale and HiDPI factor. The Xcursor parser is
  removed. See [ADR-0070](docs/adr/0070-svg-cursors-with-bundled-bibata-fallback.md).
- A full **Bibata-Modern-Ice** SVG theme is bundled in the binary and used as
  the universal fallback, so a standard cursor always exists — even on a bare
  TTY with no `XCURSOR_THEME` and no installed icon theme. (Bibata is GPL-3.0;
  its license ships with the theme.)
- `wp_cursor_shape` name resolution still prefers the protocol/CSS name
  (`default`, `text`, `e-resize`, ...) with legacy cursor-name aliases
  (`left_ptr`, `xterm`, ...) as fallback, matching what modern themes ship.
- `$XCURSOR_PATH` is honored for theme search roots when set.

### Overlay compositing

- Input-method popups, drag icons, and client cursor surfaces are now
  drawn above ordinary shell chrome, so a Dock or status bar can no
  longer cover the candidate panel or cursor.
- The screenshot freeze snapshot now includes protocol overlays, so the
  frozen trigger frame matches what was on screen when the selector
  opened.

## [0.0.3] - 2026-07-28

### Canonical Aegis namespace

- Standardized the compositor, desktop identity, Realm isolation, portal,
  internal runtime identifiers, and diagnostics on the `aegis` namespace.
- Renamed compositor environment variables to the `AEGIS_*` prefix and moved
  the default configuration path to `$XDG_CONFIG_HOME/aegis/config.toml`.
- Renamed the fuji MCP server to `aegis`, its public tools to
  `mcp__aegis__*`, and its desktop skill to `aegis-desktop-realm`.
- Removed legacy namespace aliases. Existing environment, MCP, permission,
  and skill configurations must use the canonical Aegis names.

### Borderless window decoration policy

- Aegis now negotiates compositor-owned decorations by default, so
  decoration-aware Wayland clients such as foot omit client-side title bars.
  Window movement, resizing, closing, and state controls remain available
  through compositor gestures, invisible borders, the Dock, and other shell
  surfaces.
- Added the live-reloadable `[ui] window_decorations` setting. Its default is
  `"borderless"`; set it to `"client-side"` when application-drawn frames are
  preferred.
- Removed the dormant server-side title-bar chrome component and its unused
  window-action plumbing.

### Wayland client stability and responsiveness

- Input-method candidate popups now follow the focused text surface when its
  window moves, instead of retaining the caret's old compositor position.
- Input-method preedit and commit transactions now continue to the focused
  application when their serial references an older text-input state, as
  required by `zwp_input_method_v2`; this prevents intermittent missing
  preedit in clients such as foot.
- Compositor-owned overlays, including the launcher, overview, and screenshot
  selector, now intercept key sequences without faking a Wayland keyboard
  focus change. Input-method preedit and commits therefore remain active in
  focused applications such as foot, and each key release follows the route
  that received its matching press.
- Minimizing a focused window now updates keyboard, text-input, selection, and
  shortcut-inhibition focus together; rebinding a `wl_keyboard` resource also
  restores the seat's existing focus and modifier state.
- Fixed a compositor crash when an input method destroyed a keyboard resource
  while virtual-keyboard modifiers were being forwarded.
- Chromium-based Wayland clients now fall back to the compositor's working
  surface frame callbacks instead of receiving incomplete presentation-time
  feedback, avoiding severe browser sluggishness.
- Nested sessions now release their canvas and Vulkan surface before tearing
  down the host display, preventing shutdown-order crashes.

### StatusNotifierItem interoperability

- StatusNotifierItem registration now replies before reading the item's
  properties, preventing synchronous tray clients and input methods from
  timing out or deadlocking during startup.

### Wallpaper and frame pacing

- Animated rendering remains capped at 60 frames per second on high-refresh
  outputs to avoid unnecessary full-resolution wallpaper GPU work.
- The built-in 3D wallpaper model is now opt-in. Set
  `AEGIS_WALLPAPER_MODEL=builtin` to enable it, or set the variable to a `.glb`
  path to use a custom model.

### Smart Dock visibility

- Maximized windows now collapse the Dock into a centered translucent capsule.
  Hovering near the capsule reveals the Dock; the rest of the bottom edge stays
  client-owned.
- Fullscreen windows remove the Dock, capsule, hover target, and status bar
  until fullscreen ends. Maximized windows keep the status bar visible.
- IPC protocol version 8 adds explicit `available`, `maximized`, and
  `fullscreen` space-use transition events.

### Window switching

- Holding `Super` while using `Tab` or `Shift+Tab` now presents live previews
  of every visible window and highlights the focused selection until `Super`
  is released.
- Global shortcut releases are consumed with their matching presses, so the
  newly focused client no longer receives a stray Tab release.

### Status bar and tray

- Removed the active-window title and icon from the left side of the status
  bar.
- The tray now displays only applications that explicitly register a
  StatusNotifierItem. Ordinary windows such as Chrome and foot no longer
  appear as synthetic tray entries.

## [0.0.2] - 2026-07-28

### Wayland input methods and browser stability

- Added host-side Wayland input-method support through
  `zwp_input_method_manager_v2`, `zwp_virtual_keyboard_manager_v1`, keyboard
  grabs, virtual-keyboard forwarding, and compositor-positioned candidate
  popups. Native Wayland applications continue to use
  `zwp_text_input_manager_v3`; privileged input-method globals are hidden
  from Realm clients.
- Fixed `wp_viewport.set_source(-1, -1, -1, -1)` decoding. The protocol
  transports `-1.0` as the 24.8 fixed-point value `-256`; treating the wire
  value as integer `-1` disconnected Chromium-based clients with an invalid
  viewport error.

### Device defaults

- Touchpads now use natural scrolling by default.
- Direct display sessions now select the highest-pixel mode at its highest
  refresh rate when no output mode is configured. A configured resolution
  without an explicit refresh rate also selects its highest available rate.

### Smart Dock visibility

- The Dock now hides automatically when a visible window is maximized and
  returns when the pointer reaches the bottom edge. Fullscreen windows use a
  stricter lock that keeps the Dock hidden and disables its hover trigger
  until fullscreen ends.

### Client surface compositing

- Fixed client-side title bars, popup-internal surfaces, and other
  above-parent `wl_subsurface` content from lower windows rendering over a
  foreground window. Physical output, overview thumbnails, and directed
  Realm capture now composite each toplevel and its complete surface tree as
  one z-ordered unit while preserving mixed shm and dma-buf order.

## [0.0.1] - 2026-07-27

### Release v0.0.1 Preparation & Installation
- Added automated release installation script `scripts/install.sh` supporting user (`~/.local`) and system prefix installations.
- Configured systemd user service (`aegis.service`), desktop entry (`io.github.ming2k.aegis.Settings.desktop`), icons, and XDG portal D-Bus definitions.
- Enhanced GitHub Actions CI workflow (`.github/workflows/ci.yml`) to validate workspace-wide tests, clippy, and release builds.

### System Settings and compositor application boundaries

- Renamed the standalone settings product to **System Settings**. Its Cargo
  package and executable are now `aegis-settings`, its Wayland application id
  is `io.github.ming2k.aegis.Settings`, and its desktop file is
  `io.github.ming2k.aegis.Settings.desktop`
  ([ADR-0059](docs/adr/0059-first-party-application-installation-and-development-staging.md)).
- Added a private first-party application staging prefix for development.
  `scripts/dev.sh` builds, stages, and starts one integrated nested or
  explicit DRM session. It exposes System Settings through standard `PATH`
  and `XDG_DATA_DIRS` discovery instead of a compositor-only source-tree
  fallback.
- Split persistent settings modules and their IPC application host into the
  `aegis-settings` crate.
- Removed the remaining `aegis-control-center` compatibility host. Immediate
  controls now live in the status bar; Realm lifecycle and authority
  management remain in `aegis-ai-workspaces`
  ([ADR-0060](docs/adr/0060-statusbar-system-controls-and-live-system-ipc.md)).
- Removed the independent `aegis-quick-settings` crate, built-in application
  identity, and launcher entry. IPC protocol version 7 adds a shared
  `SystemStatus` snapshot, typed `SystemAction` controls, change events, and a
  scopeable `SystemControl` operation so external clients use the same runtime
  control path as compositor chrome. The reference CLI exposes that path
  through `aegis-ctl system`.
- The status bar notification panel no longer uses the Control Center name.
  Its audio, network, and notification controls open one status-and-controls
  panel, while the Fuji indicator opens AI Workspaces directly.
- Development installations must remove the obsolete
  `io.github.ming.aegis.ControlCenter.desktop` file. The former
  `aegis-control-center` application binary, crate, stale `aegis-ctl-center`
  fallback command, built-in identity, and old application id have no
  compatibility aliases.

### Atomic configuration persistence

- Added a path-bound `aegis-config::ConfigStore` and typed `ConfigEdit`
  operations for dock pins, touchpad profiles, and output settings. Every
  programmatic edit now preserves unrelated TOML, validates the complete
  resulting schema, and uses a flushed same-directory atomic replacement.
- Routed all compositor-owned configuration writes through one serialized
  worker, preventing rapid dock and System Settings edits from losing one
  another while keeping authorization and live-state application outside
  `aegis-config`.

### xdg-desktop-portal backend (Phase 3A): Background, Inhibit, ScreenCast persistence

- `aegis-portal` now serves `org.freedesktop.impl.portal.Background` v1 and
  `org.freedesktop.impl.portal.Inhibit` v1, upgrades
  `org.freedesktop.impl.portal.ScreenCast` to v2, and emits
  `SettingChanged` when `[appearance] color_scheme` changes
  ([ADR-0053](docs/adr/0053-portal-session-services-and-grants.md)).
  `ass.portal` lists the two new interfaces.
- Background: requested `background = true` is granted and recorded per
  app_id; `autostart = true` copies the application's desktop file into
  `$XDG_CONFIG_HOME/autostart/` (reported `false` when no desktop file
  exists), and `autostart = false` removes it.
- Inhibit: flag 4 (idle) is served through a new connection-scoped,
  fail-closed `SetIdleInhibit` IPC op — `control` capability plus an
  explicit `IdleInhibit` op in the built-in `aegis-portal` scope, which now
  grants exactly three operations. The compositor keeps a per-connection
  registry on the main loop and folds it into a surfaceless global
  inhibitor in the Wayland idle machinery; disconnecting releases it.
  Flags 1/2/8 (logout, user switch, suspend) are logged and ignored — they
  need a session manager ass does not have — and `QueryEndResponse` is
  declared but never emitted. An application's inhibits are released when
  its bus name vanishes or the portal restarts.
- ScreenCast v2: `persist_mode` 1 (token in memory) and 2 (token persisted)
  are accepted; `Start` returns a `restore_token`, and a valid token
  presented to `SelectSources` restores the cast with no further check.
- Portal-owned grants persist as JSON under `$XDG_DATA_HOME/aegis-portal/`
  (`background.json`, `screencast-tokens.json`, mode `0600`, atomic
  writes); delete `screencast-tokens.json` to revoke persisted casts.

### Wayland protocol fixes

- Fixed `zwp_pointer_gestures_v1` FFI struct layout in `aegis-compositor`: corrected opcode order to `get_swipe_gesture` (0), `get_pinch_gesture` (1), `release` (2), and `get_hold_gesture` (3). Previously, `destroy` was placed at opcode 0, which caused GTK3 and GTK4 applications (such as GIMP and `gtk3-demo`) to destroy the gestures manager upon binding swipe gestures and crash with Wayland protocol error `Error 22 (Invalid argument)`.

### xdg-desktop-portal backend (Phase 1)

- New `aegis-portal` binary: a standalone, D-Bus-activated xdg-desktop-portal
  backend that bridges the portal interfaces to the compositor's scoped IPC
  (ADR-0051). It serves `org.freedesktop.impl.portal.Settings` v1
  (`org.freedesktop.appearance color-scheme`, read from the new
  `[appearance] color_scheme` config key) and
  `org.freedesktop.impl.portal.Screenshot` v1 (non-interactive; pixels come
  from `CaptureOutput` over sealed-memfd IPC and are delivered as `file://`
  URIs under the portal cache directory). Interactive screenshot requests
  fail with response code 2 until Phase 3.
- The compositor grants the backend a new built-in owner-only IPC scope
  `aegis-portal` limited to exactly the `CaptureOutput` operation; no user
  configuration is required.
- New integration files under `contrib/`: `xdg-desktop-portal/portals/ass.portal`,
  `xdg-desktop-portal/aegis-portals.conf` (`default=ass;gtk`, so UI-driven
  portals fall back to the GTK backend), and
  `dbus-1/services/org.freedesktop.impl.portal.desktop.ass.service`. Install
  and verification steps are in
  [How to Install and Verify the Portal Backend](docs/how-to/portals.md).

### xdg-desktop-portal backend (Phase 2): ScreenCast

- `aegis-portal` now serves `org.freedesktop.impl.portal.ScreenCast` v1
  (monitor sources, no persistence, no cursor capture), so portal-aware
  applications — browser `getDisplayMedia()`, OBS-style recorders — can cast
  the screen through the standard portal path. Each started cast republishes
  the compositor's output frames as a PipeWire producer stream
  (`aegis-portal-screencast`, raw `BGRx` video at up to 30 fps); a running
  PipeWire session is required. Interactive source selection remains Phase 3.
- The compositor IPC grows to protocol version 5 with scoped output-frame
  streaming ([ADR-0052](docs/adr/0052-scoped-output-frame-streaming.md)):
  `StreamOutputStart`/`StreamOutputStop` plus pushed `StreamFrame` events
  whose pixels travel as sealed memfds, reusing the one-shot capture blob
  channel. Authorization matches `CaptureOutput` (`control` capability plus
  an explicit `StreamOutput` scope op); the built-in `aegis-portal` scope now
  grants exactly those two operations. Delivery is backpressured (a bounded
  two-frame lane per stream, excess frames dropped and counted), pauses
  while the session is locked or the seat is inactive, and ends on scope
  revocation, lease expiry, or output-geometry changes.

### Session environment for portals and Flatpak

- The compositor now publishes `WAYLAND_DISPLAY`, `XDG_SESSION_TYPE=wayland`,
  and `XDG_CURRENT_DESKTOP=ass` once its Wayland socket exists, and exports
  them to the D-Bus activation environment and the systemd --user manager
  (`dbus-update-activation-environment --systemd`). D-Bus-activated services
  such as xdg-desktop-portal and `flatpak-spawn` helpers now see the session,
  fixing Flatpak applications that previously failed to launch. Nested
  development sessions skip the export so the host session is unaffected.
- Launched applications now inherit `DBUS_SESSION_BUS_ADDRESS` and
  `XDG_CURRENT_DESKTOP`; the launcher's environment whitelist previously
  dropped both.
- The packaged `ass.service` now binds to `graphical-session.target`, so
  D-Bus-activated session services start and stop with the compositor.

### Screenshot selector: frozen frame and explicit confirmation

- The interactive screenshot selector (Print key) now freezes the screen at
  the trigger frame: the whole frame — desktop scene and chrome (dock,
  status bar, toasts) — is snapshotted into an offscreen image when the
  selector opens, and only the selector itself renders on top of that
  snapshot until it closes, so background window updates and live chrome
  no longer leak into the shot.
- Releasing the pointer after a drag no longer saves immediately. The
  selection stays on screen with a confirm hint; Enter/Space saves, Escape
  cancels, and a new drag replaces the staged selection (niri-style flow).

### fuji (宓姬): rename and self-contained agent runtime

- Renamed `ass-neenee` to `aegis-fuji` and its `ass-neenee-mcp` binary to
  `aegis-fuji-mcp`. The environment variables are now `ASS_FUJI_*`, the default
  scope is `fuji`, the default Realm label is `Fuji`, and bridge state moved
  to `$XDG_RUNTIME_DIR/aegis-fuji/`. The `realm_transfer_window` tool's
  `target` value `neenee` is now `fuji`; all other MCP tool names and schemas
  are unchanged. Realm recovery records under the old directory do not
  migrate. fuji is named after Lady Fu (宓妃) of the *Luoshen Fu*.
- Added fuji's own agent runtime in the same `aegis-fuji` crate, with the
  `fuji` CLI:
  streaming Anthropic and OpenAI-compatible providers, the agent loop,
  built-in file/shell/image tools, an stdio MCP client, JSONL sessions,
  `SKILL.md` discovery, and a per-tool allow/ask/deny permission policy. The
  agent is self-contained in this workspace: it no longer depends on
  `../praxion` or any external agent product, and reaches the desktop only
  through `aegis-fuji-mcp` (ADR-0050).
- Renamed the shipped skill path to `integrations/fuji/skills`; the
  `ass-desktop-realm` skill itself is unchanged for fuji wording.

### User-owned Dock defaults

- An unconfigured Dock now starts with only the `Applications` tile; running
  applications remain transient until the user selects `Keep in Dock`.
  Automatic population remains available as an explicit configuration opt-in.
- Dock menu requests now preserve explicit pin and unpin intent. Unpinning an
  automatically selected application no longer pins it by mistake, and the
  first manual edit preserves the other visible automatic selections.

### Standalone modular Control Center

- `aegis-ctl-center` is now a standalone Iris/Lens Wayland application with
  a stable desktop entry and deep links for `display`, `mouse`, `touchpad`,
  `keyboard`, `appearance`, `power`, `users`, and `window-rules`. Launcher and
  status-bar activation use the ordinary external application path. Immediate
  live controls remain compositor chrome, while AI Workspace management is a
  separate compositor-owned surface.
- Settings pages use a KCM-inspired contract: stable metadata, categories,
  search keywords, apply policy, authoritative snapshot updates, and typed
  intents. Display and touchpad are editable. The other domains expose honest
  unavailable pages until their authoritative services exist.
- IPC protocol version 4 adds revisioned settings snapshots,
  `GetSettings`, `SettingsChanged`, confirmed `Settings` transactions, and
  settings mutation-journal entries. Display and touchpad edits are validated,
  persisted, applied on the compositor main loop, and acknowledged only after
  completion; stale editor revisions fail without overwriting newer state.

### fuji Agent Realm integration

- Added `aegis-fuji` and its `aegis-fuji-mcp` stdio server, which connect the
  fuji agent to ASS without duplicating provider, credential, session, or
  agent-runtime policy in the compositor.
- Added scope-aware desktop tools plus a bridge-managed Agent Realm lifecycle:
  XDG app discovery and sandboxed launch, optimistic authority transfer,
  pause/resume, directed PNG capture with MCP image content, bounded Realm-seat
  input, crash recovery, and fail-closed revocation to the human Realm.
- Added the `ass-desktop-realm` fuji skill and configuration/reference
  guidance for an observe-operate-verify workflow. Realm ids are never model
  arguments, and one per-scope process lock prevents concurrent bridge owners.
- The synchronous ass IPC client now supports applying read/write timeouts
  before a scoped handshake, allowing async adapters to bound stalled local
  IPC without hanging the connector.
- Added `aegis-fuji-mcp smoke`, a live reversible acceptance check that reads
  start and completion notifications back from compositor state, verifies a
  temporary Agent Realm through active/paused/active transitions, leaves time
  for visual inspection, and confirms revocation. Existing managed Realms are
  preserved.
- The status bar now shows a persistent, state-colored Agent Realm indicator;
  clicking it opens Control Center directly on AI Workspaces, where Realm id,
  state, controlled-window count, pointer/keyboard/touch seat capabilities,
  and lifecycle controls are visible.
- Successfully applied Agent Realm input now has compositor-owned visual
  feedback distinct from the user's XDG cursor: a labeled circular crosshair,
  movement trail, click pulse, scroll/keyboard state, and a background-
  operation fallback. It omits key contents, hides on lock, clears on
  revocation, and is excluded from directed Realm capture. The optional
  `smoke --input-window <id>` probe verifies a real non-clicking pointer move
  through the journal and restores the selected window to the human Realm.
- Notification toasts now use a bounded two-line layout, so long agent titles
  and bodies stay inside the visible card instead of overflowing its bounds.
- Fixed two Wayland lifecycle faults exposed by live Realm smoke testing:
  `wl_data_device.release` now has its required v2+ dispatch slot, and cursor-
  shape constructors remain protocol-valid when seat capabilities are withdrawn
  in the same dispatch cycle. Realm revoke no longer crashes the compositor,
  and immediate create/pause no longer disconnects ordinary desktop clients.

### Command-line tool renamed to `aegis-ctl`

- The reference IPC client has been renamed from `ass-ctl` to `aegis-ctl`
  to align with the workspace's spelled-out naming convention
  (`aegis-ctl-center`, `aegis-backend`, `aegis-config`). The installed binary,
  the crate, the library, the module path, and the IPC scope constant
  `LOCAL_REALM_ADMIN_SCOPE` (now `"aegis-ctl-realm-admin"`) all follow.
- The CLI is now built with `clap` derive. `--help` / `-h` / `--version`
  are recognized on every subcommand; help is per-subcommand
  (`aegis-ctl realm transfer --help`); shell completions for bash, zsh,
  fish, PowerShell, and elvish are generated by
  `aegis-ctl completions <shell>`.
- Realm commands are grouped under a single `realm` subcommand instead of
  the flat `realm-*` namespace. The `realms` list command is now
  `realm list`. Migration map:

  | Old                                | New                                |
  |------------------------------------|------------------------------------|
  | `ass-ctl realms`                   | `aegis-ctl realm list`           |
  | `ass-ctl realm-create [label]`     | `aegis-ctl realm create [label]` |
  | `ass-ctl realm-pause <id>`         | `aegis-ctl realm pause <id>`     |
  | `ass-ctl realm-resume <id>`        | `aegis-ctl realm resume <id>`    |
  | `ass-ctl realm-transfer <w> <r>`   | `aegis-ctl realm transfer <w> <r>` |
  | `ass-ctl realm-launch <r> <app>`   | `aegis-ctl realm launch <r> <app>` |
  | `ass-ctl realm-capture <r> [path]` | `aegis-ctl realm capture <r> [path]` |
  | `ass-ctl realm-revoke <r>`         | `aegis-ctl realm revoke <r>`     |

- The `--region x,y,w,h` flag is now scoped to `screenshot` and
  `realm capture` only, instead of being a global flag with a runtime
  allowlist. Passing it to any other subcommand is now a usage error
  caught at parse time.
- `set-geometry` accepts negative coordinates positionally
  (`aegis-ctl set-geometry 1 -20 30 800 600`); no `--` separator needed.
- Errors are now typed (`thiserror`-backed `CliError`); exit codes are
  unchanged (0 success, 1 runtime failure, 2 usage). Library callers that
  matched on the previous `Result<_, String>` must switch to
  `Result<_, ass_control::CliError>`.

### Shell component crates

- The dock and the Control Center moved from `aegis-shell` modules into
  their own crates, `aegis-dock` and `aegis-ctl-center`, fulfilling the
  promotion path deferred in ADR-0021. `aegis-shell` keeps the `Chrome`
  host, the shared contract, and the remaining components; the `ass`
  binary remains the composition root that registers every component.
  See ADR-0044.
- The shared chrome contract no longer names component-specific types:
  `Chrome::update_app_catalog` now receives one `AppCatalog` snapshot
  (applications, resolved pins, and an `IconSet` of borrowed icon
  textures) that `Shell` owns, seeds into every registered component,
  and replaces through `set_app_catalog`.
- Application match keys (`StartupWMClass`, desktop-id stem, icon name)
  are unified on `Entry::match_keys` in `aegis-core`, replacing a
  binary-local helper. No user-visible behavior change.

### Status bar crate and StatusNotifierItem tray

- The top status bar (`HudBar`) moved from `aegis-shell` into its own
  crate `aegis-statusbar`, with the component type renamed
  `HudBar` → `StatusBar`. `aegis-shell` exposes `HUD_HEIGHT` as a `pub
  const` (consumed by `Toast` for its top margin) and no longer carries
  any status-bar code. The bar's `[statusbar] enabled` configuration
  flag is the first per-component enable switch; it defaults to `true`
  so an unconfigured session keeps the bar. See ADR-0045.
- ass now ships a real StatusNotifierItem (SNI) system tray: the
  compositor runs as the session's `StatusNotifierWatcher` + Host on
  the session D-Bus and renders registered items' icons in the bar's
  tray row. Items that expose a dbusmenu `Menu` object path get a
  compositor-rendered right-click popover (label rows, separators,
  checkmark/radio toggles, submenu navigation, `Event("clicked")`
  activation); items without one fall back to `SecondaryActivate`. The
  tray row folds open-window cells and SNI cells into a five-slot
  budget with a `+N` overflow indicator.
- The implementation adds the workspace's first `zbus` dependency
  (`zbus` v5 with `default-features = false` + `async-io` +
  `blocking-api`), running on two dedicated `std::thread`s behind
  `Arc<Mutex<_>>` and `mpsc`. No async runtime enters the dependency
  graph; `cargo tree -p aegis-statusbar` shows no `tokio`, `async-std`,
  or `smol`. Without a session bus, the SNI tray silently stays empty
  and startup is unaffected.

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
  ass-ctl gained a `serde`/`serde_json` dependency and enables aegis-core's
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
  `aegis-core::notify`.

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
  `aegis-core::workspace`.

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
  `$XDG_RUNTIME_DIR/aegis.sock`; the library's `run` entry point is
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
  `aegis-config` `LayoutConfig` section converts to `ass_core::layout::LayoutParams`.
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
  logic, unit-tested in isolation; `aegis-config` deserializes the rules and
  `aegis-compositor` applies them on map and on config reload.
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
  phase). The workspace snapshot types live in `aegis-core` (serde-derived) so
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
  `$XDG_RUNTIME_DIR/aegis.sock`, the foundation of the extension and
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
- New pure crate `aegis-ipc` (depends only on `aegis-core`, `serde`,
  `serde_json`) owns the schema, codec, server (`Handler` trait + accept
  thread + per-connection reader/writer threads), and reference client,
  verified end-to-end by loopback tests covering query, commands, capability
  refusal, and event delivery. `aegis-core` gained an optional, off-by-default
  `serde` feature deriving `Serialize`/`Deserialize` on the shared model
  types (`Window`, `WindowState`, `SizeHints`, `Point`, `Size`, `Rect`) so
  the IPC sends the same types rather than reconstructing them.

### Declarative configuration (TOML + live reload)
- Configuration now lives in a single TOML file at
  `$XDG_CONFIG_HOME/ass/config.toml` (defaulting to `~/.config/aegis/config.toml`),
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
- New pure crate `aegis-config` (depends only on `aegis-core`, `serde`, `toml`,
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
- `aegis-protocols` build script now probes `pkg-config --cflags-only-I
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
  lens's safe surface matches what `aegis-shell` used, and `lens-sys`'s
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
- `aegis-shell` now compiles end-to-end for the first time. A latent bug
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
  `aegis-core::launcher` module; the flux-ui component is a thin adapter.
- Added two new leaf crates: `aegis-desktop-entries` (desktop-entry parsing, `XDG_DATA_HOME`
  / `XDG_DATA_DIRS` traversal, deduplication with user-overrides-system
  precedence, locale resolution via `LC_MESSAGES`, `Exec` field-code
  expansion, and Icon Theme Spec lookup with the `hicolor` fallback) and
  `aegis-launcher` (the detached spawn path, terminal-emulator wrapping for
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
- Added `aegis-core::app::Entry` as the shared launchable-application model,
  `aegis-core::launcher` (with the `Launch::{Spawn,Focus}` outcome) for the
  search state machine, and
  `aegis-core::input::{KeyChar, KeyAction, key_action, TapDetector}` for the
  keyboard path — so the shell chrome needs no `aegis-desktop-entries` dependency.
- `aegis-shell` gains a `Launcher` chrome component, three `Chrome` trait
  methods (`captures_keyboard`, `key_char`, `toggle`), and a
  `ChromeEvents::spawn` intent; its dependency graph is unchanged. The binary
  wires enumeration to the launcher, drains the spawn intent into
  `aegis-launcher`, routes keyboard capture, and runs the Super-tap detector.
- See [ADR-0022](docs/adr/0022-application-launcher.md). Rendering real app
  icons as textures, a runtime application rescan, and a configurable
  keybind (e.g. `Super+Space`) are follow-up work. Apps that set neither a
  matching `app_id` nor `StartupWMClass` will not be recognized as running.

### Shell architecture
- Split `aegis-shell` into a pure core host and pluggable chrome components.
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
- Added a new `aegis-wallpaper` crate that draws a user-chosen
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
- New `transform_pixels` helper in `aegis-render` applies each transform
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
