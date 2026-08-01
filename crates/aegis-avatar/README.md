# aegis-avatar

User-avatar loading and rendering for Aegis. Provides the identity orb shown
on the lock screen and other chrome surfaces.

It resolves the avatar source from XDG-conformant locations, then prepares a
GPU-ready texture:

- **Still images** (PNG, JPEG, WebP, GIF, BMP, ICO, TIFF, TGA, QOI, PNM) are
  cover-fit to a square, masked to a circle, premultiplied, and uploaded as a
  single `RGBA8_UNORM` flux texture.
- **VRM models** (VRM 0.x / VRM 1.0, which are `.glb` containers) are loaded
  through `flux-scene-graph` and rendered into a reusable offscreen texture.
  A companion VRM Animation 1.0 clip is retargeted onto VRM 0.x or 1.0
  humanoid bones, sampled continuously, and skinned on the GPU. The portrait
  camera follows the animated head while retaining a head-and-shoulders crop.

The crate owns decode, animation, and GPU state only — like `aegis-render` and
`aegis-wallpaper`, it does not hold a Wayland connection or a presentation
loop. Callers advance animated avatars with elapsed time and draw the current
texture with the normal 2D canvas path.

See [ADR-0080](../../docs/adr/0080-avatar-crate-xdg-conformant-vrm-aware.md)
for the avatar-as-crate and XDG-conformant design.
