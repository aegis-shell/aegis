# How to Configure the Wallpaper

Use the `[wallpaper]` table in
`$XDG_CONFIG_HOME/aegis/config.toml`. A valid save replaces the live scene;
an invalid schema or an asset that cannot be loaded leaves the previous
wallpaper visible.

Unset `AEGIS_WALLPAPER` while testing persistent configuration. That variable
is a process-start source override and takes precedence over the table.

## Select a Single-Source Mode

Set a still or animated image:

```toml
[wallpaper]
mode = "image"
source = "/home/user/Pictures/wallpaper.webp"
```

Omit `source` in image mode to use the built-in wallpaper. Set a video that
`ffmpeg` can decode:

```toml
[wallpaper]
mode = "video"
source = "/home/user/Videos/ambient.webm"
```

Set a model-only wallpaper with the built-in procedural model:

```toml
[wallpaper]
mode = "3d"
source = "builtin"
```

Use a `.glb` path instead of `builtin` for a custom model. Add `background`
to place the model over an image or video:

```toml
[wallpaper]
mode = "3d"
source = "/home/user/Models/sculpture.glb"
background = "/home/user/Pictures/studio.png"
```

## Build a Parallax Scene

Prepare two to eight images with the same intended aspect ratio. The first
image normally fills the output and is opaque. Later images normally use PNG
or WebP alpha so only their foreground objects cover the earlier layers.

Declare the layers from farthest to nearest:

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

Relative paths resolve from the directory containing `config.toml`.
`depth = 0.0` keeps a plane fixed; `depth = 1.0` gives it the complete
`max_shift` displacement. Intermediate values move proportionally.

`transition_ms` is the approximate time for a discontinuous target change to
settle 95 percent of the way. Values from 180 to 320 ms usually retain a
responsive pointer feel while smoothing a crossing behind a window. The
accepted range is listed in the
[Configuration Reference](../reference/config.md#wallpaper).

Move the pointer between two exposed desktop areas with a client window
between them. The scene holds its last target while the pointer crosses the
window, then moves continuously to the new target when the pointer reaches
the wallpaper again. Client content, resize regions, shell chrome, and points
outside the output do not update the target.

Set `reduced_motion = true` under `[ui]` to center the scene and disable
pointer parallax.

## Run the Alpine Example

A source checkout includes an original three-plane Alpine scene and a complete
configuration. From the repository root, run:

```bash
env -u AEGIS_WALLPAPER -u AEGIS_WALLPAPER_MODEL \
  XDG_CONFIG_HOME="$PWD/examples/parallax-wallpaper" \
  cargo run --locked -p aegis
```

The example config references the three images under
`assets/wallpapers/parallax-alpine/`. Copy those images beside a normal user
configuration, or replace their paths with another separated image set.
