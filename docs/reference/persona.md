# Persona

The `aegis_shell::persona` domain keeps account-backed profile defaults,
personalized still or VRM content, motion behavior, and live reload consistent
across presentation surfaces. Authenticated Actor principals are separate and
never derive from these display fields.

`Profile::current` resolves the effective account's username, display name,
initials, and group labels. These are presentation defaults, not proof of
identity. A future personalization store may replace display fields without
changing the security principal.

The lightweight `Profile` API is available with default `aegis-shell`
features. The remaining APIs on this page require the shell's optional
`persona` feature.

## Canonical Source Order

`PortraitConfig::current` selects the first usable candidate in this order:

| Priority | Source | Kind |
|----------|--------|------|
| 1 | `$XDG_DATA_HOME/aegis/avatars/face.png` | Still image |
| 2 | `$XDG_DATA_HOME/aegis/avatars/face.jpg` | Still image |
| 3 | `$XDG_DATA_HOME/aegis/avatars/face.webp` | Still image |
| 4 | `$XDG_DATA_HOME/aegis/avatars/face` | Still image |
| 5 | `~/.face` | Freedesktop-compatible still image |
| 6 | `~/.face.icon` | Freedesktop-compatible still image |
| 7 | `$XDG_DATA_HOME/aegis/avatars/avatar.vrm` | VRM 0.x or 1.0 model |

`AEGIS_AVATAR_DEBUG_ASSETS=1` adds the source-tree debug VRM before priority
1 in debug builds. It never changes release behavior. `PortraitConfig::new`
accepts an explicit ordered list for an embedder that must replace the
canonical policy.

The lock screen and command panel retain one `PortraitConfig` value and use
that same value for initial loading, filesystem observation, and every
transactional reload.

## Still Images

The persona portrait layer decodes PNG, JPEG, WebP, GIF, BMP, ICO, TIFF, TGA,
QOI, and PNM. It cover-fits the first decoded frame to a square, applies a
premultiplied circular mask, and uploads one `RGBA8_UNORM` texture. The same
portrait module selects and renders VRM content behind the common `Portrait`
interface.

## VRM and Motion Layout

The canonical model is:

- `$XDG_DATA_HOME/aegis/avatars/avatar.vrm`

Motion libraries live beside it:

- `$XDG_DATA_HOME/aegis/avatars/motions/idle/*.vrma`
- `$XDG_DATA_HOME/aegis/avatars/motions/actions/*.vrma`

Lowercase ASCII stems beginning with a letter are stable motion names. A
legacy `$XDG_DATA_HOME/aegis/avatars/avatar.vrma` acts as one looping idle
clip only when neither motion-library directory supplies clips.

## VRM Camera Contract

Every presentation caller passes a `VrmCamera` when a selected source is VRM:

| Field | Unit | Constraint |
|-------|------|------------|
| `vertical_fov_degrees` | Degrees | Finite and between `1` and `179` |
| `visible_height_ratio` | Model-height ratio | Finite and greater than `0` |
| `center_from_top_ratio` | Visible-frame ratio | Finite |
| `horizontal_offset_ratio` | Model-height ratio | Finite |

The renderer follows animated head translation but does not choose these
composition values. A caller can apply a new camera composition with
`Portrait::set_camera`; unchanged values do not trigger GPU work.

## Reload Semantics

`PortraitWatcher` observes every source in its `PortraitConfig`, including
motion-library subdirectories. It reports changes after a short trailing-edge
debounce. The render thread builds a complete replacement before publishing
it. A malformed partial save keeps the last-known-good portrait and receives
bounded retries; deleting all configured sources returns `None` so the caller
can install its own fallback chrome.
