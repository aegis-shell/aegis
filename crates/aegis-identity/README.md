# aegis-identity

Shared local-account identity and portrait configuration for Aegis.

`Identity` resolves the effective user's username, display name, initials,
and groups once. `PortraitConfig` is the single ordered source contract used
by the lock screen, command panel, and future identity consumers. The same
immutable value is passed to both `Portrait::load_transactional` and
`PortraitWatcher`, so initial selection and live reload cannot disagree.

`PortraitConfig::current` implements the canonical convention:

1. `$XDG_DATA_HOME/aegis/avatars/face.png`
2. `$XDG_DATA_HOME/aegis/avatars/face.jpg`
3. `$XDG_DATA_HOME/aegis/avatars/face.webp`
4. `$XDG_DATA_HOME/aegis/avatars/face`
5. `~/.face`
6. `~/.face.icon`
7. `$XDG_DATA_HOME/aegis/avatars/avatar.vrm`

An explicitly enabled source-tree debug VRM precedes this list. Embedders can
construct an explicit ordered contract with `PortraitConfig::new` instead of
reimplementing discovery.

Still images are decoded, cover-fit, circle-masked, premultiplied, and
uploaded here. VRM candidates are delegated to `aegis-avatar` with the camera
supplied by the presentation caller. `Portrait` exposes one consistent
texture and forwards VRMA motion controls only when the selected source is a
VRM. Missing content remains `None`; the host owns its visual fallback.

See the [Identity Portrait Reference](../../docs/reference/identity-portraits.md)
and
[ADR-0106](../../docs/adr/0106-shared-identity-portrait-contract-and-vrm-renderer-boundary.md).
