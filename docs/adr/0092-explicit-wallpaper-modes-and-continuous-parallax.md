# ADR-0092: Explicit wallpaper modes and continuous pointer parallax

- Status: Accepted
- Date: 2026-08-01

## Context

[ADR-0018](0018-wallpaper-crate.md) separates wallpaper decoding and drawing
from the compositor, but its original runtime model is one image or video
source with an optional glTF overlay. That model does not express a
multi-plane image scene, and further source types would require ambiguous
combinations of paths and feature switches.

Pointer parallax also receives discontinuous observations. The wallpaper is
exposed at two positions when the pointer crosses a client window, passes
through compositor chrome, or leaves and re-enters the output. Applying each
visible sample directly would move every image plane in one frame. Updating
through an obscuring window instead would make visible wallpaper outside the
window react to pointer activity that is not aimed at the desktop.

The effect must preserve event-driven frame pacing, respect the desktop-wide
reduced-motion policy, avoid exposing an image edge at maximum displacement,
and remain configurable without embedding compositor hit-testing in the
wallpaper crate.

## Decision

Aegis exposes four explicit wallpaper modes: `image`, `video`, `3d`, and
`parallax`. The persistent `[wallpaper]` table selects one mode. Image and
video modes use one source; 3D mode uses a built-in or glTF model with an
optional 2D background; parallax mode uses two to eight back-to-front image
layers.

Each parallax layer carries a normalized `depth` from `0.0` to `1.0`. A layer
at zero remains fixed. A layer at one receives the configured maximum
displacement, and intermediate layers receive the proportional amount. Layers
are declared in ascending depth order so their configuration order is also
their draw order. Transparent PNG or WebP images provide the visual cutouts;
opacity masks are not a separate runtime format.

The compositor supplies pointer targets only when the logical point is exposed
wallpaper. Client content, client resize affordances, shell chrome, lock
surfaces, drags, and points outside the output withhold the sample. Withholding
a sample retains the last target.

The wallpaper advances a monotonic, overshoot-free low-pass state from the
current position toward the latest target. `transition_ms` describes the
approximate 95 percent settle time and is independent of input-event and frame
rates. Time before a newly observed target is not charged to that transition,
so waking after an obscured or idle interval cannot snap directly to the new
position.

Every layer uses cover scaling with displacement-sized overscan. Near layers
therefore sample beyond the viewport before they move, and maximum
displacement cannot expose an empty edge. While the motion state is unsettled,
the existing wallpaper deadline participates in damage and event-loop pacing
at up to 60 frames per second. A pointer sample over exposed wallpaper also
disables the hardware-cursor-only presentation path for that input.

The desktop-wide reduced-motion switch centers the parallax scene and disables
pointer targets. It does not jump to the latest pointer position.

`aegis-wallpaper` owns layer decode, texture state, cover geometry, motion, and
animation deadlines. The compositor owns configuration precedence, relative
path resolution, desktop hit-testing, and live replacement. The Wallpaper
portal keeps its single-file contract and replaces the current scene with a
normal image or video wallpaper; it does not synthesize a multi-layer scene.

## Alternatives

- **Continue with one source plus optional features.** Rejected because paths
  would change meaning according to unrelated flags and invalid combinations
  would be difficult to diagnose.
- **Track the pointer through windows.** Rejected because exposed wallpaper
  would react while the user interacts with client content, and the effect
  would couple desktop presentation to client input routing.
- **Apply only exposed samples without interpolation.** Rejected because two
  samples on opposite sides of a window produce the visible one-frame jump
  that the effect is intended to remove.
- **Use a spring with overshoot.** Rejected because overshoot requires more
  image margin and makes edge safety depend on velocity. The monotonic filter
  has a bounded displacement and a direct settle-time control.
- **Encode depth in file names or layer order alone.** Rejected because
  explicit normalized depth is portable across resolutions and permits
  several visual planes to share a distance.
- **Reset to center whenever the pointer is obscured.** Rejected because it
  creates motion unrelated to an exposed pointer sample and produces another
  transition at both sides of every window.

## Consequences

Wallpaper configuration now describes source intent directly and rejects
mode-incompatible fields. Relative asset paths resolve beside `config.toml`,
and a valid wallpaper change applies through configuration hot reload. A
failed asset load keeps the previous live scene.

Parallax consumes one decoded image and one GPU texture per plane. The layer
count is capped at eight, and motion redraws the full wallpaper while it
settles. Static image, video, and 3D behavior retain their existing decode and
render paths.

Artists must supply pre-separated opaque or alpha-bearing planes ordered from
far to near. The generated Alpine example demonstrates the required asset
shape, while the exact option schema and activation workflow live in the
[Configuration Reference](../reference/config.md#wallpaper) and
[How to Configure the Wallpaper](../how-to/configure-wallpaper.md).
