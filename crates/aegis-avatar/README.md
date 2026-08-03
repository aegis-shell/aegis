# aegis-avatar

VRM avatar animation and offscreen rendering for Aegis.

The crate consumes an explicit VRM model path, optional legacy VRMA path,
and caller-owned `VrmCamera`. It owns:

- VRM 0.x and VRM 1.0 parsing and textured material rendering;
- VRM Animation 1.0 humanoid retargeting and GPU skinning;
- shuffled idle clips, named one-shot actions, and motion state;
- animated-head tracking and a reusable offscreen portrait texture.

It does not resolve account identity, load still portraits, choose between
still and VRM sources, watch XDG paths, or draw fallback discs, rings,
keylines, and other host chrome. Those responsibilities belong to
[`aegis-identity`](../aegis-identity/README.md) and the presentation caller.

Callers pass `VrmCamera` to `Avatar::load`. Its public parameters are vertical
field of view, visible model-height ratio, center-from-top ratio, and
horizontal offset ratio. `Avatar::set_camera` supports a later composition
change without rebuilding source-selection state. The crate intentionally has
no default camera profile: the host that owns the viewport owns the framing.

See
[ADR-0106](../../docs/adr/0106-shared-identity-portrait-contract-and-vrm-renderer-boundary.md)
for the responsibility boundary,
[ADR-0096](../../docs/adr/0096-avatar-motion-library-and-semantic-playback.md)
for motion playback, and
[ADR-0098](../../docs/adr/0098-textured-vrm-materials.md) for scene-owned VRM
materials.
