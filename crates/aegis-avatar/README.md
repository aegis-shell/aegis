# aegis-avatar

User-avatar loading and rendering for Aegis. Provides the identity orb shown
on the lock screen and other chrome surfaces.

It resolves the avatar source from XDG-conformant locations, then prepares a
GPU-ready texture:

- **Still images** (PNG, JPEG, WebP, GIF, BMP, ICO, TIFF, TGA, QOI, PNM) are
  cover-fit to a square, masked to a circle, premultiplied, and uploaded as a
  single `RGBA8_UNORM` flux texture.
- **VRM models** (VRM 0.x / VRM 1.0, which are `.glb` containers) are loaded
  through `flux_scene-graph` and rendered offscreen to a circular texture.
  Animation (VRMA humanoid clips) is plumbed through the API but depends on
  skinning/morph support landing in the scene graph; until then the model is
  rendered as a posed mesh and `AvatarKind::Animated3d` reports the limitation
  honestly rather than silently dropping motion.

The crate owns decode and GPU-upload state only — like `aegis-render` and
`aegis-wallpaper`, it does not hold a Wayland connection or a presentation
loop. Callers draw the produced texture with the normal 2D canvas path.

See [ADR-0080](../../docs/adr/0080-avatar-crate-xdg-conformant-vrm-aware.md)
for the avatar-as-crate and XDG-conformant design.
