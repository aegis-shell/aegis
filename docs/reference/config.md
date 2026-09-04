# Configuration Reference

Exact reference for the tessera configuration file at
`$XDG_CONFIG_HOME/tessera/config.toml` (defaulting to `~/.config/tessera/config.toml`).
For the design behind it, the loader, and the live-reload contract, see
[ADR-0026](../adr/0026-configuration-system.md).

The file is TOML. It is hot-reloaded: save it and behavior changes without a
restart. A malformed file or an unknown field is reported as a `config:`
log diagnostic and never crashes the compositor.

## Programmatic Edits

The command panel's settings tabs do not write TOML directly. They submit
revisioned, typed
settings transactions to the compositor, which validates authority, serializes
all configuration writes, and applies live state. Dock pin changes use the
same serialized path.

`tessera-config::ConfigStore` translates the accepted dock, input, output,
desktop-preference, and idle-policy edits into TOML. Each edit preserves
comments and unrelated keys, validates the complete resulting schema, flushes
a temporary file in the same directory, and atomically replaces the
configuration file. A malformed or schema-incompatible existing file is left
untouched.

## Top-Level Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `schema_version` | integer | required | Schema major version. Must be `2`. A different value is rejected with a diagnostic; supported migrations are explicit through `tessera config migrate`. |
| `[[keybind]]` | array of tables | built-in defaults | Global key bindings. See [Key Bindings](#key-bindings) and [System Shortcuts](keyboard-shortcuts.md). |
| `[[window_rule]]` | array of tables | none | Placement rules applied to newly-mapped toplevels. See [Window Rules](#window-rules). |
| `[layout]` | table | gaps `8`, master_ratio `0.5` | Tiling policy parameters. See [Layout](#layout). |
| `[dock]` | table | automatic pins | Applications pinned to the dock. See [Dock](#dock). |
| `[hud]` | table | `enabled = true` | Whether the display-only session HUD chips are registered at startup. See [HUD](#hud). |
| `[ui]` | table | `hicolor` icons, `default` 24 px cursor, borderless windows, full motion | Desktop-wide UI and window-presentation policy. See [UI](#ui). |
| `[input.keyboard]`, `[input.mouse]`, `[input.touchpad]` | tables | repeat 25/250 ms, neutral mouse, natural touchpad | Keyboard repeat, and mouse and touchpad motion and scrolling. See [Input](#input). |
| `[wallpaper]` | table | built-in image | Image, video, 3D, or multi-plane parallax wallpaper. See [Wallpaper](#wallpaper). |
| `[lock_screen]` | table | cinematic layout, built-in lock artwork | Lock-screen composition and independently selected background. See [Lock Screen](#lock-screen). |
| `[idle]` | table | dim 5 min, lock 10 min, display off 11 min, suspend 30 min | Ordered inactivity, session-lock, display-power, and suspend policy. See [Idle and Locking](#idle-and-locking). |
| `[battery]` | table | warn at 20% and 5% | Low-battery warning thresholds. See [Battery](#battery). |
| `[[output]]` | array of tables | none | Per-connector display policy: mode, scale, position, transform, primary. See [Outputs](#outputs). |
| `[screenshot]` | table | XDG Pictures directory, cursor included | Screenshot output policy. See [Screenshots](#screenshots). |
| `[appearance]` | table | system color scheme, normal contrast, standard fonts and scale | Desktop-wide color, contrast, and typography preferences. See [Appearance](#appearance). |
| `[agent]` | table | lockdown on | Agent authorization policy: whether unpaired connections keep privileged capabilities. See [Agent Authorization](#agent-authorization). |
| `[audit]` | table | 2048 MiB ceiling, 512 MiB reserve | Durable authority-history storage and checkpoint policy. See [Audit Storage](#audit-storage). |
| `[interaction_domain_sandbox]` | table | default deny | Process-resource budgets for new Interaction Domain application launches; network and host files remain isolated. See [Interaction Domain Sandbox](#interaction-domain-sandbox). |
| `[dev]` | table | all off | Development-only escape hatches; planned for removal before release. See [Development Options](#development-options). |

## Environment

Wallpaper and desktop-preference environment overrides are selected at
process startup. Configuration remains the persistent source; overrides are
not written back to TOML.

| Variable | Default | Description |
|----------|---------|-------------|
| `TESSERA_ICON_THEME` | `[ui] icon_theme`, then `hicolor` | Highest-precedence icon theme override used by the Dock, launcher, and exported toolkit preference. No GNOME or KDE settings database is consulted. |
| `XCURSOR_THEME` | `[ui] cursor_theme`, then `default` | Highest-precedence cursor-theme override used by compositor rendering and exported toolkit preferences. |
| `XCURSOR_SIZE` | `[ui] cursor_size`, then `24` | Highest-precedence cursor size override. Values outside 8–128 are ignored. |
| `TESSERA_WALLPAPER` | `[wallpaper]`, then bundled image | Process-start source override. Accepts an image, animated image, short video, or model-only `.glb` and disables the configured source mode for that process. |
| `TESSERA_WALLPAPER_MODEL` | configured model, then unset | Process-start 3D-model override. Set to `builtin` for the procedural knot or to a `.glb` path. Ignored when `TESSERA_WALLPAPER` is a `.glb` or the configured mode is parallax. |

The launcher captures image/video, 3D, and client layers into one quarter-scale
RGBA8 offscreen scene and updates a fixed-cost Dual-Kawase backdrop every
frame. Animation is capped at 60 frames per second. Allocation or
unsupported-format failures fall back to the launcher's translucent overlay
for that session.

## Wallpaper

The `[wallpaper]` table selects one explicit rendering mode. It hot-reloads
with the rest of the file. Relative asset paths resolve from the directory
containing `config.toml`; a load failure keeps the previous live scene.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | string | `"image"` | `"image"`, `"video"`, `"3d"`, or `"parallax"`. |
| `source` | string | built-in image | Image path in image mode; required video path in video mode; required `builtin` or `.glb` path in 3D mode; invalid in parallax mode. |
| `background` | string | none | Optional image or video behind a 3D model. Invalid in other modes. |
| `max_shift` | float | `32.0` | Maximum logical-pixel displacement for a parallax layer at depth `1.0`; `1.0`–`256.0`. |
| `transition_ms` | integer | `240` | Approximate 95% parallax settle time after a target change; `80`–`2000` ms. |
| `[[wallpaper.layer]]` | array of tables | none | Two to eight image planes for parallax mode, ordered back-to-front by ascending `depth`. |

Each parallax layer has these fields:

| Field | Type | Description |
|-------|------|-------------|
| `path` | string | Image path. The first plane is normally opaque; later planes normally carry alpha. |
| `depth` | float | Relative distance from `0.0` (fixed/farthest) to `1.0` (nearest/full displacement). |

```toml
[wallpaper]
mode = "parallax"
max_shift = 36.0
transition_ms = 260

[[wallpaper.layer]]
path = "wallpapers/sky.png"
depth = 0.0

[[wallpaper.layer]]
path = "wallpapers/ridge.png"
depth = 0.45

[[wallpaper.layer]]
path = "wallpapers/foreground.png"
depth = 1.0
```

Every plane uses cover scaling with enough overscan for its maximum movement.
Pointer targets update only on exposed wallpaper; the scene interpolates
between samples separated by windows or shell chrome. Setting
`reduced_motion = true` under `[ui]` centers the scene and disables pointer
parallax.
See
[How to Configure the Wallpaper](../how-to/configure-wallpaper.md) for all
four mode examples.

## Lock Screen

The `[lock_screen]` table selects the lock presentation independently from
the desktop `[wallpaper]` table. `tessera-lock` reads it when a new lock client
starts; saving the file changes the next lock screen, not an already secured
one.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `style` | string | `"cinematic"` | `"cinematic"` places the clock at the upper right and a game-style credential rail at the lower right. `"centered"` uses the conventional centered identity column. `"bsod"` renders the nostalgic full-screen stop-screen composition (see below). |
| `[lock_screen.background]` | table | built-in lock artwork | Background source and artwork scrim. It never inherits or mutates `[wallpaper]`. Ignored by `style = "bsod"`, which always paints its own signature blue. |

The background table accepts these fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | string | `"builtin"` | `"builtin"`, `"solid"`, or `"image"`. |
| `source` | string | none | Required static-image path in image mode. Relative paths resolve beside `config.toml`; invalid in other modes. |
| `color` | string | scheme-aware solid | Optional `#RRGGBB` value in solid mode; invalid in other modes. |
| `dim` | float | `0.28` | Dark artwork scrim from `0.0` through `0.85`. Solid backgrounds remain pure and do not receive this scrim. |

```toml
[lock_screen]
style = "cinematic"

[lock_screen.background]
mode = "image"
source = "wallpapers/lock-screen.png"
dim = 0.24
```

Lock images use the shared wallpaper image decoder but retain their own
source and lifecycle. A missing or undecodable custom image logs a warning and
falls back to the bundled lock artwork so presentation failure cannot prevent
locking. The cinematic style is typographic and does not load an avatar; the
centered style's flat initial fallback follows `[appearance]` `color_scheme`
and `accent_color` without a gradient or white overlay. A rejected credential
briefly shakes its rail and holds it red instead of showing an error sentence;
`[ui] reduced_motion = true` disables the shake while retaining the color.
The neutral cinematic rail has no empty password slots, so it does not imply a
fixed or expected password length.

The `bsod` style reproduces the classic stop page as a lock composition: a
flat `#0078D7` field that ignores the background table, a large sad face, a
left-aligned wrapped headline, a cycling `N% complete` counter that reports
real elapsed progress while authentication is genuinely in flight, a
square white credential box that turns red on rejection, and a lower-left
support block whose stop code tracks the authentication phase
(`SESSION_LOCKED`, `CREDENTIAL_MISMATCH`, `AUTH_SERVICE_UNAVAILABLE`). A quiet
corner clock remains during the ambient privacy presentation. It loads no
avatar and honors `[ui] reduced_motion` by freezing the counter.

## Layout

The `[layout]` table tunes the master-stack tiling policy (ADR-0024). Applied
live on reload.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `gaps` | integer | `8` | Gap in logical pixels between tiles and around the work-area edge. |
| `master_ratio` | float | `0.5` | Fraction of the work-area width (0.0–1.0) for the master column. |
| `default_tiled` | boolean | `false` | Whether newly created workspaces start in tiled mode. |

```toml
[layout]
gaps = 16
master_ratio = 0.6
default_tiled = true
```

The work area is the focused output's logical rectangle minus reserved shell
chrome, including the dock. Negative gaps and master ratios outside
`0.0`–`1.0` reject the configuration.

Window rules (`role`) and transient dialogs override `default_tiled` for the
windows they cover.

## UI

The `[ui]` table holds desktop-wide UI and window-presentation policy
([ADR-0029](../adr/0029-animation-and-effect-policy.md) and
[ADR-0063](../adr/0063-compositor-owned-borderless-decoration-policy.md)).
Applied live on reload.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `reduced_motion` | boolean | `false` | Accessibility reduced-motion switch. When `true`, every chrome and lens transition (dock magnification, launcher reveal, fades, slides) resolves to its end state in at most one frame. |
| `icon_theme` | string | `"hicolor"` | Freedesktop application icon theme used by the launcher, Dock, and themed shell symbols. `$TESSERA_ICON_THEME` wins when set. Changes apply live. |
| `cursor_theme` | string | `"default"` | SVG cursor theme for the software cursor on direct display, resolved through the freedesktop cursor spec. The theme must ship `cursors/<name>.svg` files; a conventional Xcursor binary theme resolves but contributes nothing, logs a warning, and every shape falls back to the bundled art. `$XCURSOR_THEME` wins when set. When the selected theme is not installed, the bundled Tessera art is the final fallback. |
| `cursor_size` | integer | `24` | Cursor size in logical pixels, 8–128. `$XCURSOR_SIZE` wins when set. |
| `window_decorations` | string | `"borderless"` | Decoration ownership for Wayland toplevels. `"borderless"` makes Tessera own window controls without drawing per-window title bars; `"client-side"` asks applications to draw their own frames. |
| `window_shadow` | string | `"resize"` | Compositor drop-shadow style for floating windows (ADR-0139). `"resize"` draws the historic 4-px stroke shadow; `"soft"` renders a blurred shadow through the Optics shadow operator (rounded-rect mask, Gaussian blur, downward offset; focus raises its opacity); `"none"` disables shadows. Tiled, maximized, fullscreen, and minimized windows never cast one. |

```toml
[ui]
reduced_motion = true
icon_theme = "Papirus-Dark"
cursor_theme = "Bibata-Modern-Ice"
cursor_size = 24
window_decorations = "borderless"
```

Application icon themes are searched under `$HOME/.icons`,
`$XDG_DATA_HOME/icons` (normally `~/.local/share/icons`), and each
`$XDG_DATA_DIRS/icons` directory. A named theme directory must contain a
valid `index.theme`. Theme inheritance and the final `hicolor` fallback
follow the freedesktop Icon Theme Specification.

`reduced_motion` is the single switch for animation policy; individual
effects do not override it. Changes to `window_decorations` reconfigure
existing decoration-aware Wayland windows as well as newly created windows.

## Input

The `[input]` section carries the seat's device policy in three tables:
`[input.keyboard]`, `[input.mouse]`, and `[input.touchpad]`. The command
panel's Input tab submits the complete profile to the compositor, which
persists it without replacing comments or unrelated sections. In a nested
session the outer compositor owns the physical devices, so changes are saved
for the next direct session.

### Keyboard

The compositor does not repeat keys itself; it advertises these values as
`wl_keyboard.repeat_info` and clients repeat locally.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `repeat_rate` | integer | `25` | Repeats per second, `0` to disable repetition. At most `150`. |
| `repeat_delay_ms` | integer | `250` | Milliseconds a key is held before repeating starts. `1`–`2000`. |

```toml
[input.keyboard]
repeat_rate = 30
repeat_delay_ms = 200
```

### Mouse

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `natural_scroll` | boolean | `false` | Move content in the same direction as the wheel. |
| `pointer_speed` | float | `0.0` | libinput acceleration speed from `-1.0` (slowest) to `1.0` (fastest). |
| `scroll_speed` | float | `1.0` | Multiplier applied to wheel motion. `0.1`–`10.0`; `1.0` leaves device motion untouched. |

```toml
[input.mouse]
natural_scroll = false
pointer_speed = 0.0
scroll_speed = 1.0
```

### Touchpad

Applied live to every libinput touchpad in a direct DRM session.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `natural_scroll` | boolean | `true` | Move content in the same direction as the fingers. |
| `tap_to_click` | boolean | `true` | Map a light tap to a primary-button click. |
| `tap_and_drag` | boolean | `true` | Start dragging when a finger stays down after a tap. |
| `drag_lock` | boolean | `false` | Keep a tap-drag active briefly after the finger lifts. |
| `disable_while_typing` | boolean | `true` | Suppress accidental touchpad input while typing. |
| `pointer_speed` | float | `0.0` | libinput acceleration speed from `-1.0` (slowest) to `1.0` (fastest). |
| `scroll_speed` | float | `1.0` | Multiplier applied to touchpad scroll motion. `0.1`–`10.0`. |
| `scroll_method` | string | `"two-finger"` | `"two-finger"` or `"edge"`. Unsupported methods fall back to one supported by the device. |

```toml
[input.touchpad]
natural_scroll = true
tap_to_click = true
tap_and_drag = true
drag_lock = false
disable_while_typing = true
pointer_speed = 0.2
scroll_speed = 1.0
scroll_method = "two-finger"
```

Unsupported controls are disabled when a physical device reports its
capabilities. The selected values remain the device profile and will be
applied after hotplug when a compatible touchpad appears.

## Idle and Locking

The `[idle]` table is the staged inactivity policy used by the supervised
`tessera-idle` session client. Saving a valid change replaces the running policy
without restarting the compositor. The lock screen uses
`ext-session-lock-v1`; display power-off and suspend wait until the compositor
has confirmed the secure lock.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `true` | Allow inactivity to trigger configured stages. Manual locking and lock-before-sleep remain active when `false`. |
| `dim_after_seconds` | integer | `300` | Seconds after the last activity to set the hardware backlight to `dim_percent`; `0` disables the stage. |
| `lock_after_seconds` | integer | `600` | Seconds after the last activity to start `tessera-lock`; `0` disables automatic locking. |
| `display_off_after_seconds` | integer | `660` | Seconds after the last activity to power down displays after lock confirmation; `0` disables the stage. |
| `suspend_after_seconds` | integer | `1800` | Seconds after the last activity to request logind suspend after lock confirmation; `0` disables the stage. |
| `dim_percent` | integer | `30` | Hardware backlight target, 1–100 percent. |

Each timeout is either `0` or at most `604800` seconds (seven days).
Nonzero timeouts must be strictly increasing in table order. A nonzero
display-off or suspend timeout requires a nonzero lock timeout.

```toml
[idle]
enabled = true
dim_after_seconds = 300
lock_after_seconds = 600
display_off_after_seconds = 660
suspend_after_seconds = 1800
dim_percent = 30
```

Per-surface `zwp_idle_inhibit_v1` inhibitors and authorized portal
`IdleInhibit` requests keep every stage resumed. A locked session ignores
those inhibitors. Activity restores a dimmed backlight and wakes powered-down
outputs behind the lock.

The command panel's Quick Controls (`Super+S`) offer the related controls:
Lock Now locks the session immediately through the same path as `Super+L`,
and the two session switches compose into the session power mode
(ADR-0140): *Keep Screen Awake* is the display axis (never blank) and
*Automatic Lock* is the security axis. The three resulting modes:

| Mode | Dim | Lock | Display off | Suspend |
|------|-----|------|-------------|---------|
| `balanced` (default) | ✓ | ✓ | ✓ | ✓ |
| `awake` | ✓ | — | — | — |
| `secure` | ✓ | ✓ | — | — |

The mode is a session toggle; it is not persisted to this file. Manual
locking (`Super+L`, Lock Now) and lock-before-sleep apply in every mode.
Switching modes changes which stages are armed without restarting the idle
coordinator. `tessera system power-mode <mode>` selects the same mode from
the CLI.

In a nested session, locking remains active but the outer desktop retains
brightness, physical output-power, and suspend ownership. The dim,
display-off, and suspend stages are therefore not executed by the nested
session.

See [How to Configure Locking and Idle](../how-to/lock-and-idle.md) for the
panel workflow and
[ADR-0078](../adr/0078-out-of-process-idle-and-session-lock.md) for the
security boundary.

## Battery

The `[battery]` table configures the low-battery warnings on devices with a
battery. Each threshold fires one modal alert per discharge cycle — a
centered panel that captures the keyboard and pointer until dismissed; it is
not a notification and is not affected by do-not-disturb. Charging clears
the cycle, so the next discharge warns again. Changes apply on live reload.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `warn_at` | array of integers | `[20, 5]` | Discharge percentages that raise the alert, highest first. The lowest value uses the critical wording and fill. An empty list disables the feature. |

Each value must be between 1 and 99, and the list must be strictly
descending (duplicates are rejected).

```toml
[battery]
warn_at = [20, 5]
```

## Night Light

The `[night_light]` table schedules a display color-temperature shift
(ADR-0129). While active, the compositor programs a per-channel gain ramp
into each CRTC's gamma table — a KMS-level adjustment with zero
render-pipeline cost, applied live on a one-second cadence with a gradual
fade. Changes apply on live reload.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enable` | boolean | `false` | Master switch. Without a schedule, enabling warms the outputs permanently. |
| `temperature` | integer | `4000` | Color temperature in Kelvin while active, 1000–10000. Lower is warmer; 6500 is neutral daylight. |
| `start` | string | unset | Fade-in start, local `"HH:MM"` (24-hour). Requires `end`. |
| `end` | string | unset | Fade-out start (return to neutral), local `"HH:MM"`. An overnight window (`start` later than `end`) is honored. |
| `fade_seconds` | integer | `1200` | Duration of the temperature fade in seconds. |

```toml
[night_light]
enable = true
temperature = 3400
start = "19:00"
end = "07:00"
```

## Screenshots

The `[screenshot]` table controls where the interactive screenshot selector
writes PNG files and whether saved screenshots include the cursor. Changes
apply on live reload. After a successful interactive capture, the compositor
also publishes `image/png` and `text/uri-list` to the physical human seat's
clipboard. IPC and Interaction Domain captures do not modify that clipboard.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `save_dir` | string | `$XDG_PICTURES_DIR/screenshots` | Directory for timestamped screenshots. Falls back to `~/Pictures/screenshots` when the XDG user Pictures directory is unavailable. |
| `include_cursor` | boolean | `true` | Include the physical-seat cursor in saved screenshots. This does not affect output-capture or screencast IPC. |

```toml
[screenshot]
save_dir = "/home/alice/Pictures/screenshots"
include_cursor = true
```

## Appearance

The `[appearance]` table holds desktop-wide color, contrast, and typography
preferences. The compositor combines it with `[ui]` and startup overrides
into one effective profile used by chrome and the Settings IPC. The portal
subscribes to that profile and exports its standard and toolkit-compatible
projections. See
[ADR-0072](../adr/0072-desktop-preference-authority-and-toolkit-compatibility.md).

Changes apply on live reload. The command panel's Appearance tab writes the
`[appearance]` and
preference-related `[ui]` fields as one validated transaction.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `color_scheme` | string | `"system"` | `system` advertises no preference (portal `color-scheme` 0); `dark` and `light` map to 1 and 2. |
| `accent_color` | string | unset | Optional sRGB accent color in `#RRGGBB` form. When unset, the portal omits `accent-color`. |
| `contrast` | string | `"normal"` | `normal` advertises no special contrast preference; `high` requests high contrast. |
| `font_name` | string | `"Sans 10"` | Proportional interface font description exported to compatible GTK applications. |
| `monospace_font_name` | string | `"Monospace 10"` | Monospace interface font description exported to compatible GTK applications. |
| `text_scale` | float | `1.0` | Text scaling multiplier from 0.5 through 3.0. |

```toml
[appearance]
color_scheme = "dark"
accent_color = "#3584E4"
contrast = "normal"
font_name = "Inter 11"
monospace_font_name = "Iosevka 11"
text_scale = 1.0
```

## Outputs

Each `[[output]]` table overrides one aspect of a connector's
backend-reported geometry (ADR-0028). Only `connector` is required; an
entry that sets no override is rejected as having no effect. An entry
whose connector is not currently plugged in is ignored until the
connector appears. Duplicate entries for one connector resolve
last-wins.

Direct DRM sessions calculate the backend scale automatically from the
selected mode and validated EDID physical dimensions. Internal eDP, LVDS,
and DSI panels target 125 logical PPI; external displays target 110 logical
PPI. The result is rounded to the nearest 0.25 and clamped to 1.0–4.0.
Missing, inconsistent, or implausible physical dimensions fall back to 1.0.
Nested sessions retain the scale reported by the outer compositor.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `connector` | string | required | The backend's connector name, as shown by `tessera display` (e.g. `"DP-1"`, `"HDMI-A-1"`, `"nested"`). |
| `scale` | float | automatic on DRM; host-reported when nested | Output scale override, 0.25–4.0. Integer scales advertise through `wl_output`; fractional scales through `wp_fractional_scale_v1`. Applied live on reload. |
| `mode` | string | highest pixel count and refresh rate | Requested display mode, `"WxH"` or `"WxH@Hz"` (e.g. `"2560x1440@144"`). Without `@Hz` the highest-refresh mode of that size is used. A mode the connector does not advertise falls back to the highest-pixel mode at its highest refresh rate with a log warning. Direct DRM sessions apply changes live after the current page flip retires; nested sessions remain host-managed. |
| `position` | table | backend arrangement | Top-left of the output in the global logical layout, written `position = { x = 1920, y = 0 }`. Applied live on reload. |
| `transform` | string | `normal` | Output transform: `normal`, `90`, `180`, `270`, `flipped`, `flipped-90`, `flipped-180`, `flipped-270` (the `wl_output` underscore spellings are also accepted). Parsed and validated now, but not yet applied: until renderer output-transform support lands, a configured transform logs a warning and the output renders untransformed. |
| `primary` | boolean | `false` | Whether this output is the primary (focused) one. When several entries claim primary, the first in the backend's output order wins. Applied live on reload. |
| `hdr` | boolean | `false` | Allow HDR (BT.2020 PQ) output on this connector (ADR-0129). HDR engages only when *every* active output opts in here and advertises ST 2084 through its EDID — the compositor renders one shared framebuffer — and its plane supports 10-bit scanout; anything less stays SDR. HDR commits program the connector `Colorspace`/`HDR_OUTPUT_METADATA`/`max bpc` properties. |
| `deep_color` | boolean | `false` | Allow a 10-bit deep-color (RGB10A2-class) framebuffer in SDR for reduced banding. Engages when every active output opts in and every primary plane supports the format. |
| `icc_profile` | string | unset | Path to this output's ICC display profile (matrix+TRC profiles). The framebuffer is then written in the display's actual color space — exact for a single-output session, an approximation across mixed displays. LUT-only profiles and HDR mode are ignored with a log warning. |

```toml
[[output]]
connector = "DP-1"
mode = "2560x1440@144"
scale = 1.5
primary = true

[[output]]
connector = "HDMI-A-1"
mode = "1920x1080"
position = { x = 1707, y = 0 }
```

Run `tessera display` to see the modes each connector advertises; the
`mode` value must match one of them (resolution exactly, refresh to the
nearest whole hertz).

The command panel's Display tab submits edits for these same `[[output]]`
entries through the
compositor. Its display page can select a connected output, choose an
advertised resolution and refresh rate, set fractional scale, select the
primary output, and place an extended output to the right, left, above, below,
or at custom logical coordinates. Existing comments, unrelated settings, and
`transform` values are preserved.

## Dock

The `[dock]` table controls persistent application pins and the minimize
flight effect. Changes apply on live reload. Each value matches a
desktop-file id, desktop-file stem, `StartupWMClass`, or icon name,
case-insensitively.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `pinned` | array of strings | `[]` | Persistent applications shown in the listed order. An empty array leaves only the `Applications` tile plus transient running applications. |
| `autopopulate` | boolean | `false` | Whether an empty `pinned` list auto-selects up to 12 applications with decoded icons. Manual pin or unpin actions write this as `false`. |
| `minimize_animation` | string | `"genie"` | The effect played when a window minimizes into its dock tile (and, reversed, when it restores): `"genie"` funnels the window's lower edge into the icon first, `"scale"` shrinks the window uniformly into the icon, and `"suck"` collapses it into the icon's centre with accelerating ease-in. Also selectable from the command panel's Dock tab. |

```toml
[dock]
pinned = ["foot.desktop", "firefox", "org.gnome.Nautilus"]
minimize_animation = "genie"
```

Pinned applications stay on the left of the dock; running applications that
are not pinned appear on the right of a divider and disappear again when their
last window closes. Right-click a dock tile and select `Keep in Dock` or
`Remove from Dock` to change this list from the desktop; the compositor writes
the result back to this file and sets `autopopulate = false`.

Set `autopopulate = true` to opt into automatic selection when `pinned` is
empty. The first manual pin or unpin materializes the visible selection,
applies the requested change, and returns to explicit user-owned pins.

The application catalog is rescanned every five seconds. Installed, removed,
or edited desktop entries, including Flatpak exports, appear without
restarting tessera. The same refresh detects icon-theme, output-scale, and icon
file changes. Raster icons decode in process; SVG icons use `rsvg-convert`
when it is installed and otherwise fall back to the generic application
glyph.

## HUD

The `[hud]` table controls whether the session HUD is registered at
startup. The HUD (ADR-0080, ADR-0081, ADR-0083) is display-only: two
floating frosted chips composited over the desktop — system status
(network, Bluetooth, battery), the StatusNotifierItem tray row, the clock,
and the notification count on the left, and workspace dots in the center.
The top-right belongs to the frameless notification toast strip, and the
Agent Workspaces status lives in the command panel's System tab. The
chips reserve no space, so tiled and maximized windows run
underneath; they accept no pointer input, so clicks fall through to
windows; and each chip fades out while the cursor is near it. Changes apply
on the next launch; the flag is read once during compositor startup.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `true` | Whether the HUD chips are registered. When `false`, the chips are unavailable; the dock, launcher, command panel, and Agent Workspaces are unaffected, and live controls remain available in the command panel and over IPC. |

```toml
[hud]
enabled = false
```

Every interaction the old status bar hosted — quick settings, tray
activation and context menus, notification dismissal — lives in the command
panel (`Super+S`, or a four-finger touchpad swipe down; see
[Key Bindings](#key-bindings) and [System Shortcuts](keyboard-shortcuts.md)).

The tray row contains only StatusNotifierItem (SNI) entries explicitly
registered on the session D-Bus. Ordinary windows do not become tray icons.
SNI support runs silently: without a session bus, or when another watcher
already owns the `org.kde.StatusNotifierWatcher` name, no tray icons appear
and startup is unaffected. Tray icons in the HUD are display-only;
activating an item or opening its dbusmenu happens in the command panel's
always-visible tray grid. The row fits a five-slot budget; any excess collapses into a
`+N` overflow indicator.

## Window Rules

Each `[[window_rule]]` table matches a toplevel when it first maps and
prescribes a placement action. The first matching rule applies; a rule with
no matchers matches nothing.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `app_id` | string | — | Match if the toplevel's `app_id` contains this, case-insensitively. |
| `title` | string | — | Match if the toplevel's `title` contains this, case-insensitively. |
| `workspace` | integer | — | Move the window to this 1-based workspace index on the focused output. Applied only if that workspace exists. |
| `role` | string | — | Force the layout role: `floating` or `tiled`. A `floating` window is exempt from tiling. |

When both `app_id` and `title` are set, both must match (AND). Rules apply at
first map; an `app_id`/`title` set after mapping is not re-evaluated yet.

```toml
[[window_rule]]
app_id = "firefox"
workspace = 2
role = "tiled"

[[window_rule]]
title = "calculator"
role = "floating"
```

## Interaction Domain Sandbox

`[interaction_domain_sandbox]` defines the resource budget for applications launched with
`LaunchInInteractionDomain`. `[[interaction_domain_sandbox.app]]` entries match an exact desktop-entry
id. Later matching entries override only the fields they contain. Budget
changes apply to new launches.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `memory_max_mib` | integer | `8192` | Hard cgroup memory limit in MiB, 256–1,048,576. Swap is disabled and an out-of-memory event kills the sandbox as one group. |
| `pids_max` | integer | `1024` | Hard cgroup process limit, 16–65,536. |
| `cpu_weight` | integer | `100` | cgroup CPU weight, 1–10,000. |
| `[[interaction_domain_sandbox.app]]` | array of tables | none | Exact per-desktop-entry overrides. |

Each `[[interaction_domain_sandbox.app]]` table requires `desktop_id`. The
resource-limit fields have the same types and bounds as the default table;
an omitted field inherits the default.

```toml
[interaction_domain_sandbox]
memory_max_mib = 8192
pids_max = 1024
cpu_weight = 100

[[interaction_domain_sandbox.app]]
desktop_id = "org.mozilla.firefox.desktop"
memory_max_mib = 4096
```

Schema version 2 removes `network`, `readable_paths`, `writable_paths`, and
the legacy `[realm_sandbox]` alias. They are rejected instead of silently
granting ambient authority. Sandboxed applications always receive a private
network namespace without resolver configuration and no host user-file
mounts. Exact filesystem or origin access must pass through a broker that
consumes an Actor-session resource grant; a capability ceiling or TOML entry
alone cannot make the resource reachable.

### Migrating Schema 1

Run `tessera config migrate [path]`; the path defaults to the standard XDG
Tessera configuration file. Migration is explicit and comment-preserving. It
creates a synchronized, non-overwriting `config.toml.schema-v1.bak`-style
backup with mode `0600` before atomically replacing the active file.

Safe resource limits under `[realm_sandbox]` are moved to
`[interaction_domain_sandbox]`. A legacy `network = true` or a non-empty
`readable_paths`/`writable_paths` value has no safe schema-2 equivalent and
therefore aborts migration with a diagnostic. Replace that ambient authority
with an exact runtime resource grant, then rerun migration. Loading and live
reload never perform migrations implicitly.

Interaction Domain launch requires `/usr/bin/bwrap`,
cgroup v2, and an Tessera systemd user service with delegated `cpu`, `memory`,
and `pids` controllers. Missing isolation or controller support rejects the
launch.

## Agent Authorization

The `[agent]` table holds runtime authorization policy for
capability-borrowing agents (ADR-0088). Capability ceilings, pairing
records, and remembered grants live in the compositor-held principal
registry and grant store under `$XDG_DATA_HOME/tessera/` — never in this
file. Manage them with `tessera permissions`; the Agent Workspaces
application shows the same state. The `[[agent.scope]]` declarations from
earlier versions were removed in protocol v18 and are rejected as unknown
fields.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `lockdown` | boolean | `true` | Strip privileged capabilities from connections that neither present a built-in scope nor pair as an agent. First-party owner tools use explicit built-in scopes. Set `false` only for compatibility with legacy unnamed local clients. |

```toml
[agent]
lockdown = true
```

Pairing prompts, capability ceilings, and runtime grants are enforced per
request by the IPC server. See the [tessera-mcp Bridge
Reference](tessera-mcp.md) for the agent-side contract.

## Audit Storage

The startup-only `[audit]` table bounds the active durable authority history
at `$XDG_DATA_HOME/tessera/audit/events-v2.jsonl`. Saving valid changes does not
reconfigure an already-open stream; they apply on the next compositor start.
See the [IPC Reference](ipc.md) for durability, verification, archival, and
fail-stop behavior.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_store_mib` | integer | `2048` | Hard ceiling for the whole authority history (sealed plus active), 64–1048576 MiB. The next append is refused before crossing it; history is never deleted automatically. |
| `min_free_mib` | integer | `512` | Filesystem space reserved from audit growth, 64–1048576 MiB. An append is refused while preserving at least this much space. |
| `checkpoint_interval_mib` | integer | `8` | Maximum uncheckpointed byte tail, 1–256 MiB and no larger than `max_store_mib`. A checkpoint is also refreshed after 4096 events. |
| `segment_max_mib` | integer | `64` | Active-stream size that triggers sealing into a compressed immutable segment, 1–256 MiB and no larger than `max_store_mib` ([ADR-0137](../adr/0137-audit-segment-manifest-and-retention.md)). |
| `retain_segments` | integer | `0` | Sealed segments kept on disk (the newest); `0` keeps everything. Pruning also requires an export acknowledgement recorded via `tessera audit export`. |

```toml
[audit]
max_store_mib = 2048
min_free_mib = 512
checkpoint_interval_mib = 8
segment_max_mib = 64
retain_segments = 8
```

The first upgraded start of an existing stream performs one complete
verification to create its authenticated checkpoint. Later starts restore
the bounded live projection first and verify older history in the background;
new audit writes wait for that complete verification to succeed.

When the active stream reaches `segment_max_mib`, it is sealed into a
compressed immutable segment under `audit/segments/`, and the chain
continues in a fresh active file without resetting sequence numbers. Manage
the lifecycle — status, verification, export acknowledgement, and retention
pruning — with `tessera audit`; see [How to Manage Audit
History](../how-to/manage-audit-history.md).

## IPC

The `[ipc]` table holds process-identity policy for the built-in IPC
scopes
([ADR-0128](../adr/0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md)).
Every built-in scope has a compiled-in executable allowlist: a connection
naming the scope resolves only when the peer's canonicalized
`/proc/<pid>/exe` appears in the list. The compiled-in defaults are:

| Scope | Executables |
|-------|-------------|
| `atrium-portal` | `xdg-desktop-portal-atrium` in `/usr/bin`, `/usr/libexec`, `/usr/lib`, `/usr/local/bin` |
| `tessera-owner-admin` | `tessera` in `/usr/bin`, `/usr/local/bin` |
| `tessera-agent-admin` | `tessera` in `/usr/bin`, `/usr/local/bin` |
| `tessera-interaction-domain-admin` | `tessera` in `/usr/bin`, `/usr/local/bin` |

The `[ipc.scope_executables]` sub-table replaces the compiled-in allowlist
per scope: a scope named there admits exactly the listed paths — an empty
list refuses every claim — and a scope absent from the table keeps its
compiled-in defaults. Changes apply at the next connection handshake
through the ordinary live reload. Entries match literally or through
their own canonicalization, so a distribution symlink at either spelling
satisfies an entry.

```toml
[ipc.scope_executables]
atrium-portal = ["/opt/tessera/bin/xdg-desktop-portal-atrium"]
tessera-owner-admin = ["/usr/bin/tessera"]
```

## Development Options

The `[dev]` table holds development-only escape hatches. These options are
not a stable interface: they exist to unblock compositor development, are
planned for removal before release, and must not be relied upon.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allow_quit_while_locked` | boolean | `false` | Allow the `quit` binding (`Super+Ctrl+Q` by default) to match while the session is locked, so a wedged lock screen cannot trap a development session. No other binding matches while locked. |

```toml
[dev]
allow_quit_while_locked = true
```

## Key Bindings

Each `[[keybind]]` table binds a modifier set plus a key to an action.
Bindings layer over the [built-in defaults](#default-key-bindings): a file
with one binding keeps the rest. Several `[[keybind]]` tables may bind the
same action to different keys.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mods` | array of strings | `[]` | Modifier names, OR'd together. See [Modifier Names](#modifier-names). |
| `key` | string | required | Key name. See [Key Names](#key-names). |
| `action` | string | required | Action name. See [Action Names](#action-names). |

### Modifier Names

| Name | Aliases | Bit |
|------|---------|-----|
| `shift` | | Shift |
| `ctrl` | `control` | Control |
| `alt` | `mod1` | Mod1 (Alt) |
| `super` | `meta`, `win`, `mod4` | Mod4 (Super/Logo) |

Matching is exact on the depressed modifier mask: `mods = ["super"]` does
not also fire when Ctrl is held. Lock state (CapsLock, NumLock) does not
affect matching.

### Key Names

Letters (`a`–`z`, lowercased), digits (`0`–`9`), and the common controls:

| Name | Key |
|------|-----|
| `space` | Space |
| `return`, `enter` | Return / Enter |
| `escape`, `esc` | Escape |
| `tab` | Tab |
| `backspace`, `bs` | Backspace |
| `up`, `down`, `left`, `right` | Arrows |
| `home`, `end` | Home, End |
| `pageup`, `pgup`, `pagedown`, `pgdn` | Page Up / Down |
| `delete`, `del` | Delete |
| `f1` … `f12` | Function keys |

### Action Names

| Name | Aliases | Effect |
|------|---------|--------|
| `launcher` | `togglelauncher`, `apps` | Open or close the application launcher |
| `prism` | `toggleprism`, `spotlight` | Open or close Prism application search |
| `overview` | `toggleoverview` | Open or close the window and workspace overview |
| `command_panel` | `commandpanel`, `panel` | Open or close the command panel (quick settings, settings modules, tray, notifications) |
| `close` | `closefocused` | Close the focused toplevel |
| `cycle` | `next` | Move focus to the next toplevel; while `Super` remains held, show the live preview strip |
| `prev` | `previous`, `cycleback` | Move focus to the previous toplevel; while `Super` remains held, show the live preview strip |
| `workspace_next` | `next_workspace`, `ws_next` | Switch to the next workspace |
| `workspace_prev` | `prev_workspace`, `ws_prev` | Switch to the previous workspace |
| `fullscreen` | `toggle_fullscreen`, `togglefullscreen` | Toggle the focused toplevel between fullscreen and its prior state |
| `screenshot` | `snapshot`, `prtsc` | Open the interactive screenshot region selector |
| `lock` | `lockscreen`, `lock_screen` | Secure the session with the first-party lock screen |
| `quit` | `exit` | Quit the compositor |

A matched binding is consumed before delivery to the focused client, so the
client never sees the key that triggered it.

### Default Key Bindings

These ship as built-in defaults and remain in effect when no override is
configured:

| Binding | Action |
|---------|--------|
| `Super+Tab` | `cycle` |
| `Super+Shift+Tab` | `prev` |
| `Super+A` | `launcher` |
| `Super+Space` | `prism` |
| `Super+O` | `overview` |
| `Super+S` | `command_panel` |
| `Super+Q` | `close` |
| `Super+Right` | `workspace_next` |
| `Super+Left` | `workspace_prev` |
| `Super+F11` | `fullscreen` |
| `Super+L` | `lock` |
| `Print` | `screenshot` |
| `Super+Ctrl+Q` | `quit` |
| `Super+Shift+Return` | `quit` |

See [System Shortcuts](keyboard-shortcuts.md) for the complete default
keyboard and pointer shortcut reference.

## Touchpad Gestures

Each `[[gesture]]` table binds a touchpad swipe — a finger count plus an
axis — to an action. Bindings layer over the
[built-in defaults](#default-gesture-bindings): a file with one binding
keeps the rest, and a binding for the same finger count and axis shadows
its built-in default.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `fingers` | integer | required | Finger count. Swipe gestures require at least 3 fingers. |
| `axis` | string | required | `horizontal` or `vertical`. |
| `action` | string | required | Gesture action name. See [Gesture Action Names](#gesture-action-names). |

A swipe whose finger count has any binding is compositor-owned and never
reaches client `zwp_pointer_gestures_v1` objects, even on an axis with no
binding. The gesture latches its axis once it travels 30 px, then fires
one step per 120 px of travel. Gestures do not run while the session is
locked.

### Gesture Action Names

Unlike key-binding actions, gesture actions are directional pairs: the
swipe's dominant direction selects the direction inside the action.

| Name | Aliases | Effect |
|------|---------|--------|
| `workspace_switch` | `workspaces`, `workspace` | Horizontal: swipe left switches to the next workspace, right to the previous one |
| `window_cycle` | `cycle_windows`, `windows`, `switcher` | Vertical: swipe up focuses the next toplevel on the current workspace, down the previous one; the live switcher stays open until the gesture ends |
| `command_panel` | `commandpanel`, `panel` | Vertical: swipe down opens the command panel, up closes it; fires once per gesture |
| `overview` | `window_overview`, `picker` | Vertical: swipe up opens the window/workspace overview, down closes it; fires once per gesture |
| `none` | `unbind`, `disabled` | Consume the swipe without acting; shadows the built-in default on this axis |

### Default Gesture Bindings

These ship as built-in defaults and remain in effect when no override is
configured:

| Fingers | Axis | Action |
|---------|------|--------|
| 3 | horizontal | `workspace_switch` |
| 3 | vertical | `window_cycle` |
| 4 | vertical | `command_panel` |

The four-finger vertical swipe opened the overview between ADR-0116 and
ADR-0119. Re-add that binding (shadowing the command panel default) with:

```toml
[[gesture]]
fingers = 4
axis = "vertical"
action = "overview"
```

## Example

```toml
schema_version = 2

[[keybind]]
mods = ["super"]
key = "space"
action = "launcher"

[[keybind]]
mods = ["super", "shift"]
key = "q"
action = "quit"

[[keybind]]
mods = ["super"]
key = "right"
action = "workspace_next"

# Move workspace switching from three fingers to four, and disable the
# built-in three-finger window cycle.
[[gesture]]
fingers = 4
axis = "horizontal"
action = "workspace_switch"

[[gesture]]
fingers = 3
axis = "vertical"
action = "none"
```
