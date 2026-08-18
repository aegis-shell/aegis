# Development Environment Variables

Contributor-only reference and how-to guide for environment variable overrides
used in local development, nested compositor testing, asset validation, and UI debugging.

User-facing configuration belongs in `config.toml`; environment variables documented
here provide development-time escape hatches, backend overrides, and testing hooks.

## Quick Recipe: Nested Testing Inside a Live Aegis Session

When developing inside an active Aegis desktop session, start the nested instance
with an isolated data directory and auto-opened command panel:

```bash
mkdir -p /tmp/aegis-dev/aegis
AEGIS_COMMAND_PANEL_OPEN=1 \
XDG_DATA_HOME=/tmp/aegis-dev \
XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share \
cargo run --locked -p aegis
```

---

## Variable Reference

### 1. Compositor & Presentation Backend

| Variable | Values | Default | Purpose |
|----------|--------|---------|---------|
| `AEGIS_BACKEND` | `auto`, `nested`, `drm` | `auto` | Selects presentation host. In a graphical Wayland terminal, `auto` picks `nested`; on a bare TTY it selects `drm`. Force `nested` when running automated tests that must not fall back to DRM. |
| `AEGIS_DRM_DEVICE` | Path (e.g. `/dev/dri/card0`) | Auto-selected primary GPU | Explicitly selects the DRM primary and render device on multi-GPU systems. |

---

### 2. Chrome & Overlay UI Debugging

| Variable | Values | Purpose |
|----------|--------|---------|
| `AEGIS_COMMAND_PANEL_OPEN` | `1`, `true` | **Auto-opens the Command Panel on startup in debug builds**. Bypasses the need for `Super+S` keystrokes (which are captured by the host compositor in nested sessions). |

> [!NOTE]
> In debug mode (`debug_assertions`), the compositor automatically pre-populates
> the notification queue with mock tactical notifications (Network, GPU Telemetry,
> Security Subsystem, Power Management) if the queue is empty, allowing immediate
> visual inspection of the top-right notification cards without manual IPC triggers.

---

### 3. Persona, VRM & 3D Avatar Tooling

| Variable | Values | Purpose |
|----------|--------|---------|
| `AEGIS_AVATAR_DEBUG_DUMP` | File path (e.g. `/tmp/aegis-avatar.png`) | When running `aegis-shell` examples, writes a headless GPU-rendered snapshot of the active VRM avatar and camera framing to disk. |
| `AEGIS_AVATAR_DEBUG_ASSETS` | `1` | Forces fallback to the in-tree debug assets directory instead of XDG directories. |
| `AEGIS_AVATAR_DEBUG_TIME` | Seconds (float, e.g. `1.5`) | Simulates animation playback elapsed time when generating offscreen avatar snapshots. |

Example standalone VRM preview:

```bash
AEGIS_AVATAR_DEBUG_DUMP=/tmp/aegis-avatar.png \
AEGIS_AVATAR_DEBUG_TIME=1.0 \
cargo run -p aegis-shell --features persona --example debug_avatar
```

---

### 4. Wallpaper, Assets & Icon Theming

| Variable | Values | Purpose |
|----------|--------|---------|
| `AEGIS_WALLPAPER_MODEL` | `builtin`, path to `.glb` | Overrides the 3D scene wallpaper model. |
| `AEGIS_ICON_THEME` | Theme name (e.g. `Papirus-Dark`) | Highest-precedence override for application icon theme resolution. |

---

### 5. XDG Isolation & File Lock Protection

When running multiple compositor instances concurrently (e.g. a nested development
instance inside a live desktop session), these variables isolate volatile state:

| Variable | Recommended Dev Value | Purpose |
|----------|-----------------------|---------|
| `XDG_DATA_HOME` | `/tmp/aegis-dev` | Relocates the audit journal (`events-v2.jsonl`) and principal store. Prevents `AuditError::Locked` caused by the host session holding exclusive `flock` on the production journal. |
| `XDG_DATA_DIRS` | `$HOME/.local/share:...` | Preserves user asset discovery (`~/.local/share/aegis/avatars/avatar.vrm`, icons, and applications) when `XDG_DATA_HOME` is redirected. |
| `XDG_CONFIG_HOME` | (Optional) Path to dev config | Isolates configuration changes from the host session's `config.toml`. |

---

### 6. Logging & Observability

| Variable | Values | Default | Purpose |
|----------|--------|---------|---------|
| `RUST_LOG` | `error`, `warn`, `info`, `debug`, `trace` | `info` | Directs logging verbosity per module (e.g. `RUST_LOG="info,aegis_backend=debug"`). |
| `AEGIS_LOG_FORMAT` | `text`, `json` | `text` | Switches logger output between human-readable text and structured JSON. |
