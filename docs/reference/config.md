# Configuration Reference

Exact reference for the ass configuration file at
`$XDG_CONFIG_HOME/ass/config.toml` (defaulting to `~/.config/aegis/config.toml`).
For the design behind it, the loader, and the live-reload contract, see
[ADR-0026](../adr/0026-configuration-system.md).

The file is TOML. It is hot-reloaded: save it and behavior changes without a
restart. A malformed file or an unknown field is reported as a `config:`
log diagnostic and never crashes the compositor.

## Programmatic Edits

System Settings does not write TOML directly. It submits revisioned, typed
settings transactions to the compositor, which validates authority, serializes
all configuration writes, and applies live state. Dock pin changes use the
same serialized path.

`aegis-config::ConfigStore` translates the accepted dock, touchpad, and output
edits into TOML. Each edit preserves comments and unrelated keys, validates
the complete resulting schema, flushes a temporary file in the same
directory, and atomically replaces the configuration file. A malformed or
schema-incompatible existing file is left untouched.

## Top-Level Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `schema_version` | integer | required | Schema major version. Must be `1`. A different value is rejected with a diagnostic; bumping it is a documented migration event. |
| `[[keybind]]` | array of tables | built-in defaults | Global key bindings. See [Key Bindings](#key-bindings). |
| `[[window_rule]]` | array of tables | none | Placement rules applied to newly-mapped toplevels. See [Window Rules](#window-rules). |
| `[layout]` | table | gaps `8`, master_ratio `0.5` | Tiling policy parameters. See [Layout](#layout). |
| `[dock]` | table | automatic pins | Applications pinned to the dock. See [Dock](#dock). |
| `[statusbar]` | table | `enabled = true` | Whether the top status bar is registered at startup. See [Status Bar](#status-bar). |
| `[ui]` | table | borderless windows, reduced_motion `false` | Desktop-wide UI and window-presentation policy. See [UI](#ui). |
| `[input.touchpad]` | table | touchpad defaults | Touchpad pointing, tapping, and scrolling profile. See [Touchpad](#touchpad). |
| `[[output]]` | array of tables | none | Per-connector display policy: mode, scale, position, transform, primary. See [Outputs](#outputs). |
| `[screenshot]` | table | XDG Pictures directory | Screenshot save location. See [Screenshots](#screenshots). |
| `[appearance]` | table | `color_scheme = "system"` | Desktop-wide appearance preference read by the portal Settings backend. See [Appearance](#appearance). |
| `[agent]` | table | no scopes | Named automation scopes enforced by the IPC server. See [Agent Scopes](#agent-scopes). |
| `[realm_sandbox]` | table | default deny | Network, filesystem, and cgroup policy for new Realm application launches. See [Realm Sandbox](#realm-sandbox). |

## Environment

Wallpaper sources are selected at process startup and are not hot-reloaded.
The icon-theme override is checked during each application-catalog refresh.

| Variable | Default | Description |
|----------|---------|-------------|
| `ASS_ICON_THEME` | GTK `org.gnome.desktop.interface icon-theme`, then `hicolor` | Icon theme used by the dock and launcher. Theme inheritance and `hicolor` fallback still apply. |
| `ASS_WALLPAPER` | bundled `procedural-generation.png` | Image, animated image, short-video, or model-only `.glb` source. An image or video is shown without a 3D overlay unless `ASS_WALLPAPER_MODEL` is also set. |
| `ASS_WALLPAPER_MODEL` | unset | Optional 3D model drawn over an image or video with an orbiting camera and animated directional light. Set to `builtin` for the bundled procedural knot or to a `.glb` path for a custom model. Ignored when `ASS_WALLPAPER` is itself a `.glb`. |

The launcher captures image/video, 3D, and client layers into one quarter-scale
RGBA8 offscreen scene and updates a fixed-cost Dual-Kawase backdrop every
frame. Animation is capped at 60 frames per second. Allocation or
unsupported-format failures fall back to the launcher's translucent overlay
for that session.

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
| `cursor_theme` | string | none | XDG cursor theme for the software cursor on direct display. `$XCURSOR_THEME` wins when set; use this on bare-metal sessions with no cursor environment. |
| `cursor_size` | integer | `24` | Cursor size in logical pixels, 8–128. `$XCURSOR_SIZE` wins when set. |
| `window_decorations` | string | `"borderless"` | Decoration ownership for Wayland toplevels. `"borderless"` makes Aegis own window controls without drawing per-window title bars; `"client-side"` asks applications to draw their own frames. |

```toml
[ui]
reduced_motion = true
cursor_theme = "Bibata-Modern-Ice"
cursor_size = 24
window_decorations = "borderless"
```

`reduced_motion` is the single switch for animation policy; individual
effects do not override it. Changes to `window_decorations` reconfigure
existing decoration-aware Wayland windows as well as newly created windows.

## Touchpad

The `[input.touchpad]` table is applied live to every libinput touchpad in a
direct DRM session. System Settings submits this profile to the compositor,
which persists it without replacing comments or unrelated sections. In a
nested session the outer compositor owns the physical device, so changes are
saved for the next direct session.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `natural_scroll` | boolean | `true` | Move content in the same direction as the fingers. |
| `tap_to_click` | boolean | `true` | Map a light tap to a primary-button click. |
| `tap_and_drag` | boolean | `true` | Start dragging when a finger stays down after a tap. |
| `drag_lock` | boolean | `false` | Keep a tap-drag active briefly after the finger lifts. |
| `disable_while_typing` | boolean | `true` | Suppress accidental touchpad input while typing. |
| `pointer_speed` | float | `0.0` | libinput acceleration speed from `-1.0` (slowest) to `1.0` (fastest). |
| `scroll_method` | string | `"two-finger"` | `"two-finger"` or `"edge"`. Unsupported methods fall back to one supported by the device. |

```toml
[input.touchpad]
natural_scroll = true
tap_to_click = true
tap_and_drag = true
drag_lock = false
disable_while_typing = true
pointer_speed = 0.2
scroll_method = "two-finger"
```

Unsupported controls are disabled when a physical device reports its
capabilities. The selected values remain the device profile and will be
applied after hotplug when a compatible touchpad appears.

## Screenshots

The `[screenshot]` table controls where the interactive screenshot selector
writes PNG files. Changes apply on live reload. After a successful interactive
capture, the compositor also publishes `image/png` and `text/uri-list` to the
physical human seat's clipboard. IPC and Realm captures do not modify that
clipboard.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `save_dir` | string | `$XDG_PICTURES_DIR/screenshots` | Directory for timestamped screenshots. Falls back to `~/Pictures/screenshots` when the XDG user Pictures directory is unavailable. |

```toml
[screenshot]
save_dir = "/home/alice/Pictures/screenshots"
```

## Appearance

The `[appearance]` table holds the desktop-wide appearance preference. The
compositor does not theme from it yet; the portal backend (`aegis-portal`)
reads it to answer the freedesktop `org.freedesktop.appearance` settings
namespace ([ADR-0051](../adr/0051-portal-backend-dbus-bridge.md)), which is
how portal-aware and sandboxed applications learn whether to prefer a dark
or light UI. Changes apply on live reload; the portal re-reads the file per
query, so no restart is needed on either side.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `color_scheme` | string | `"system"` | `system` advertises no preference (portal `color-scheme` 0); `dark` and `light` map to 1 and 2. |

```toml
[appearance]
color_scheme = "dark"
```

## Outputs

Each `[[output]]` table overrides one aspect of a connector's
backend-reported geometry (ADR-0028). Only `connector` is required; an
entry that sets no override is rejected as having no effect. An entry
whose connector is not currently plugged in is ignored until the
connector appears. Duplicate entries for one connector resolve
last-wins.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `connector` | string | required | The backend's connector name, as shown by `aegis-ctl outputs` (e.g. `"DP-1"`, `"HDMI-A-1"`, `"nested"`). |
| `scale` | float | backend-reported | Output scale factor, 0.25–4.0. Integer scales advertise through `wl_output`; fractional scales through `wp_fractional_scale_v1`. Applied live on reload. |
| `mode` | string | highest pixel count and refresh rate | Requested display mode, `"WxH"` or `"WxH@Hz"` (e.g. `"2560x1440@144"`). Without `@Hz` the highest-refresh mode of that size is used. A mode the connector does not advertise falls back to the highest-pixel mode at its highest refresh rate with a log warning. Direct DRM sessions apply changes live after the current page flip retires; nested sessions remain host-managed. |
| `position` | table | backend arrangement | Top-left of the output in the global logical layout, written `position = { x = 1920, y = 0 }`. Applied live on reload. |
| `transform` | string | `normal` | Output transform: `normal`, `90`, `180`, `270`, `flipped`, `flipped-90`, `flipped-180`, `flipped-270` (the `wl_output` underscore spellings are also accepted). Parsed and validated now, but not yet applied: until renderer output-transform support lands, a configured transform logs a warning and the output renders untransformed. |
| `primary` | boolean | `false` | Whether this output is the primary (focused) one. When several entries claim primary, the first in the backend's output order wins. Applied live on reload. |

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

Run `aegis-ctl outputs` to see the modes each connector advertises; the
`mode` value must match one of them (resolution exactly, refresh to the
nearest whole hertz).

System Settings submits edits for these same `[[output]]` entries through the
compositor. Its display page can select a connected output, choose an
advertised resolution and refresh rate, set fractional scale, select the
primary output, and place an extended output to the right, left, above, below,
or at custom logical coordinates. Existing comments, unrelated settings, and
`transform` values are preserved.

## Dock

The `[dock]` table controls persistent application pins. Changes apply on
live reload. Each value matches a desktop-file id, desktop-file stem,
`StartupWMClass`, or icon name, case-insensitively.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `pinned` | array of strings | `[]` | Persistent applications shown in the listed order. An empty array leaves only the `Applications` tile plus transient running applications. |
| `autopopulate` | boolean | `false` | Whether an empty `pinned` list auto-selects up to 12 applications with decoded icons. Manual pin or unpin actions write this as `false`. |

```toml
[dock]
pinned = ["foot.desktop", "firefox", "org.gnome.Nautilus"]
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
restarting ass. The same refresh detects icon-theme, output-scale, and icon
file changes. Raster icons decode in process; SVG icons use `rsvg-convert`
when it is installed and otherwise fall back to the generic application
glyph.

## Status Bar

The `[statusbar]` table controls whether the top status bar (workspace state,
clock, system status, and the registered tray row) is registered at startup
(ADR-0045). Changes apply on the next launch; the flag is read once during
compositor startup.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `true` | Whether the top status bar is registered. When `false`, the bar's reserved edge and live-control panel are unavailable; the dock, launcher, and AI Workspaces are unaffected, and live controls remain available over IPC. |

```toml
[statusbar]
enabled = false
```

The bar's tray row contains only StatusNotifierItem (SNI) entries explicitly
registered on the session D-Bus. Ordinary windows do not become tray icons.
SNI support runs silently: without a session bus, or when another watcher
already owns the `org.kde.StatusNotifierWatcher` name, no tray icons appear
and startup is unaffected. SNI items that ship a dbusmenu `Menu` object path
get a compositor-rendered right-click popover; items without one fall back to
`SecondaryActivate` per the specification. The row fits a five-slot budget;
any excess collapses into a `+N` overflow indicator.

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

## Realm Sandbox

`[realm_sandbox]` defines the default policy for applications launched with
`LaunchInRealm`. `[[realm_sandbox.app]]` entries match an exact desktop-entry
id. Later matching entries override only the fields they contain. Policy
changes apply to new launches; revoke and relaunch a sandbox to narrow
existing kernel grants.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `network` | boolean | `false` | Share the host network namespace. When `false`, the sandbox receives an isolated network namespace and no resolver configuration. |
| `readable_paths` | array of absolute paths | `[]` | Host files or directories mounted read-only at their canonical absolute paths. |
| `writable_paths` | array of absolute paths | `[]` | Host files or directories mounted read-write at their canonical absolute paths. Protected `/proc`, `/dev`, `/run`, and `/sys` trees cannot be granted; system executable and configuration trees cannot be writable. |
| `memory_max_mib` | integer | `8192` | Hard cgroup memory limit in MiB, 256–1,048,576. Swap is disabled and an out-of-memory event kills the sandbox as one group. |
| `pids_max` | integer | `1024` | Hard cgroup process limit, 16–65,536. |
| `cpu_weight` | integer | `100` | cgroup CPU weight, 1–10,000. |
| `[[realm_sandbox.app]]` | array of tables | none | Exact per-desktop-entry overrides. |

Each `[[realm_sandbox.app]]` table requires `desktop_id`. The other fields
have the same types and limits as the default table. An omitted field
inherits the default; an explicit empty path array removes inherited paths
for that application.

```toml
[realm_sandbox]
memory_max_mib = 8192
pids_max = 1024
cpu_weight = 100

[[realm_sandbox.app]]
desktop_id = "org.mozilla.firefox.desktop"
network = true
readable_paths = ["/home/alice/Research"]
writable_paths = ["/home/alice/Downloads"]
memory_max_mib = 4096
```

Every path must be absolute and must exist when the application launches.
The launcher resolves symlinks before mounting and rejects protected or
non-file/non-directory targets. Realm launch also requires `/usr/bin/bwrap`,
cgroup v2, and an ASS systemd user service with delegated `cpu`, `memory`,
and `pids` controllers. Missing isolation or controller support rejects the
launch.

## Agent Scopes

Each `[[agent.scope]]` table names an IPC mutation allowlist. An IPC client
requests the name during its handshake; an explicit unknown name is refused.
Connections without a scope name remain unrestricted within their granted
capability classes.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Scope name presented by an IPC client. Duplicate and empty names are ignored. |
| `ops` | array of strings | unrestricted ordinary operations | Allowed operation classes. High-risk input, capture, picking, and Realm operations must be listed explicitly. An explicit array containing only unknown values grants no operations. |
| `windows` | array of integers | unrestricted | Allowed durable window IDs. |
| `workspaces` | array of integers | unrestricted | Allowed workspace IDs. |
| `realms` | array of integers | unrestricted | Allowed Realm IDs. |

Operation names are `Focus`, `Minimize`, `Close`, `Move`,
`SetWindowGeometry`, `InjectInput`, `InjectRealmInput`, `Cycle`,
`SwitchWorkspace`, `SwitchWorkspaceTo`, `MoveToWorkspace`, `ToggleTiling`,
`ToggleOverview`, `SystemControl`, `Notify`, `DismissNotification`,
`Screenshot`, `ScreenshotRegion`, `CaptureOutput`, `StreamOutput`,
`IdleInhibit`, `PickTarget`, `CreateRealm`, `TransactRealm`, `RevokeRealm`,
`CaptureRealm`, and `LaunchInRealm`. Names are case-insensitive; snake-case
forms are also accepted. Invalid names are logged and grant nothing. Input,
capture, interactive-picking, and Realm operations must be listed explicitly
and require their separately negotiated capability; omitting `ops` never
grants them.

```toml
[[agent.scope]]
name = "browser-helper"
ops = ["Focus", "Minimize", "Close"]
windows = [7, 9]

[[agent.scope]]
name = "window-input"
ops = ["InjectInput"]
windows = [7]

[[agent.scope]]
name = "workspace-rover"
ops = ["SwitchWorkspace", "SwitchWorkspaceTo", "MoveToWorkspace"]
workspaces = [2, 3]
```

Scope changes are hot-reloaded. Named connections resolve their scope again
for every mutation and capture, including final descriptor delivery, so
narrowing or removing a scope applies without reconnecting. A Realm
interaction-group mutation must include every affected window in the window
allowlist. The Rust reference client uses `Client::connect_scoped` and exposes
the granted allowlist through `Client::scope`.

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
| `close` | `closefocused` | Close the focused toplevel |
| `cycle` | `next` | Move focus to the next toplevel; while `Super` remains held, show the live preview strip |
| `prev` | `previous`, `cycleback` | Move focus to the previous toplevel; while `Super` remains held, show the live preview strip |
| `workspace_next` | `next_workspace`, `ws_next` | Switch to the next workspace |
| `workspace_prev` | `prev_workspace`, `ws_prev` | Switch to the previous workspace |
| `tiling` | `toggle_tiling` | Toggle tiling on the current workspace |
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
| `Super+Return` | `launcher` |
| `Super+Q` | `close` |
| `Super+Right` | `workspace_next` |
| `Super+Left` | `workspace_prev` |
| `Super+T` | `tiling` |
| `Super+Shift+Return` | `quit` |

A bare Super tap (press and release with no other key in between) also
toggles the launcher; it is detected separately and is not a `[[keybind]]`.

## Example

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

[[keybind]]
mods = ["super"]
key = "right"
action = "workspace_next"
```

## Migration from `$ASS_KEYBINDS`

The `$ASS_KEYBINDS` environment variable (`mods+key=action;...`) is
deprecated and remains honored as a transitional override that takes
precedence over the file. To migrate, move each entry into a `[[keybind]]`
table. The variable will be removed before the desktop phase closes.
