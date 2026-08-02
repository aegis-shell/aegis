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
  Embedded PNG/JPEG base-color textures, UV transforms, unlit materials,
  alpha modes/cutoff, glTF samplers, and double-sided primitives retain the
  model's authored colors instead of using a whole-model fallback tint.
  VRM Animation 1.0 clips are retargeted onto VRM 0.x or 1.0 humanoid bones,
  sampled continuously, and skinned on the GPU. Idle clips use a shuffled
  non-repeating playlist; named actions play once and return to idle. The
  portrait camera follows the animated head while retaining a
  head-and-shoulders crop.

The crate owns decode, animation, and GPU state only — like `aegis-render` and
`aegis-wallpaper`, it does not hold a Wayland connection or a presentation
loop. Callers advance animated avatars with elapsed time and draw the current
texture with the normal 2D canvas path. They can inspect `Avatar::motions`,
start a stable file-stem name with `Avatar::play_motion`, or choose from the
action shuffle bag with `Avatar::play_random_action`.

Long-lived consumers use `AvatarWatcher` as a notification-only seam. Poll it
from the normal render loop, then call `Avatar::load_transactional` on that
same GPU thread and replace the current avatar only after the full build
succeeds. The watcher applies trailing-edge debounce, supports bounded retry,
and rearms parent directories when an avatar tree is created or replaced.

See [ADR-0080](../../docs/adr/0080-avatar-crate-xdg-conformant-vrm-aware.md)
for the avatar-as-crate and XDG-conformant design, and
[ADR-0096](../../docs/adr/0096-avatar-motion-library-and-semantic-playback.md)
for motion discovery and playback policy, and
[ADR-0097](../../docs/adr/0097-transactional-avatar-hot-reload.md) for live
replacement semantics, and
[ADR-0098](../../docs/adr/0098-textured-vrm-materials.md) for scene-owned VRM
materials.
