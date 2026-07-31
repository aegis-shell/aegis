# ADR-0080: Avatar as an independent crate, XDG-conformant, VRM-aware

- Status: Accepted
- Date: 2026-07-30

## Context

The lock-screen identity orb is the first thing a user sees before typing a
password. It was a solid radial-gradient disc with account initials. Two
defects and one feature request arrived together:

1. **Image overflowed the circle.** The orb's only image-capable fill was a
   `fill_rect_radial_gradient`, which paints its whole axis-aligned square.
   A square image drawn under it leaked its corners past the circular keyline
   drawn on top. Flux's 2D canvas exposes only an axis-aligned `clip_rect`;
   there is no circular clip, so a cover-fit square could never be masked to
   a disc through clipping alone.
2. **No user-configurable avatar.** The orb ignored any portrait the user had
   set, and the first implementation searched `~/.face` plus a hand-rolled
   `$XDG_CACHE_HOME` path that violated both the project's XDG conventions
   (every other crate uses `aegis-desktop-entries` / `dirs`) and the
   canonical-namespace decision (ADR-0066), which mandates `$XDG_DATA_HOME/aegis`.
3. **VRM/VRMA requested.** A 3D humanoid avatar with animation clips.

The workspace already has a clear pattern for media that the compositor and
standalone clients both consume: `aegis-wallpaper` is an independent library
crate that decodes images/video/glTF and hands back GPU-ready state, with no
Wayland or presentation coupling. Putting avatar logic inside `aegis-lock`
broke that separation and would force every future avatar consumer (settings,
shell chrome) to depend on the locker.

A key technical fact shaped the VRM decision: `flux-scene-graph` v0.0.7's
documented supported subset is static meshes only (POSITION/NORMAL/TEXCOORD_0,
node tree, Phong material). Skins, morph targets, and animation clips are
**out of subset** — so a VRM model loads and renders today, but as a posed
mesh without skeleton-driven deformation or VRMA playback.

## Decision

Promote avatar handling to a dedicated **`aegis-avatar`** crate, modelled on
`aegis-wallpaper`, and make it the single source of avatar truth for all
consumers.

**Source resolution is XDG-conformant.** Still-image candidates are searched
in this order: `$XDG_DATA_HOME/aegis/avatars/{face.png,face.jpg,face.webp,face}`
(the canonical Aegis namespace, per ADR-0066), then the freedesktop
`~/.face` / `~/.face.icon` compatibility locations that GNOME/SDDM/LightDM
already write. Resolution reuses `aegis_desktop_entries::xdg_data_dirs` rather
than hand-rolling `$XDG_DATA_HOME` again. The cache directory is **never**
used for an avatar — it is disposable and the wrong home for a deliberate
portrait. VRM models are searched only under `$XDG_DATA_HOME/aegis/avatars/`
because a 3D avatar is an explicit Aegis configuration, not something other
desktops write for us.

**All avatar kinds become one circle-masked texture.** Still images are
cover-fit, analytically circle-masked, premultiplied, and uploaded once as
`RGBA8_UNORM`. A single `draw_image` composites a perfect disc, so no square
content can ever overflow the circle — the fix for defect (1) is structural,
not a clip hack.

**VRM loads through the scene graph and degrades honestly.** `Scene::from_glb`
loads the model; `Model::render_to_circle` renders it offscreen into the same
circular texture shape as a photo. `AvatarKind::Animated3d { animation }`
reports `AnimationSupport::Static` until skins/morph/animation land in
flux-scene-graph, so a caller never silently gets a frozen avatar when it
expects motion. The integration point for future animation is defined and
unchanging: advance a clock and re-render into the cached surface.

**`aegis-lock` depends on `aegis-avatar`** and supplies only its own procedural
gradient fallback orb when `Avatar::load` returns `Ok(None)`. The locker's
`build.rs` re-emits the scene-graph native-library rpath, matching how the
terminal `aegis` binary re-emits wallpaper's.

## Alternatives

- **Keep avatar logic inside `aegis-lock`.** Rejected: it duplicated XDG
  resolution (badly), forced every future consumer to depend on the trusted
  locker, and violated the established media-crate pattern.
- **Hand-roll `$XDG_CACHE_HOME` paths and AccountsService cache reads.**
  Rejected: contradicts the project's XDG conventions and ADR-0066, and the
  cache directory is the wrong home for user data.
- **Add a circular clip to Flux.** Rejected: it pushes a 2D-canvas feature
  into the rendering engine for one consumer. Baking the mask into the texture
  is one upload, zero per-frame cost, and impossible to overflow.
- **Block on full VRM animation before shipping.** Rejected: a posed 3D avatar
  is already a real improvement, and honest `AnimationSupport` reporting lets
  the feature ship without misleading users.

## Consequences

- `aegis-avatar` is a new first-party library crate in the workspace, added to
  `[workspace.dependencies]`. It depends on `aegis-desktop-entries` (for XDG),
  `image`, `flux`, and `flux-scene-graph`, and reuses the wallpaper crate's
  `build.rs` rpath pattern.
- The orb is now a real user portrait (still or 3D) when configured, with a
  guaranteed circular crop and no overflow regression, found through proper
  XDG paths.
- VRM/VRMA animation is tracked future work owned by the scene-graph layer;
  the `aegis-avatar` integration point (render to the cached offscreen surface)
  does not change when skins/morph/animation arrive.
- Packaging gains no new binary; `aegis-avatar` is a library linked into
  `aegis-lock` (and, later, other consumers). Users place a portrait at
  `$XDG_DATA_HOME/aegis/avatars/face.png` or keep using `~/.face`.
