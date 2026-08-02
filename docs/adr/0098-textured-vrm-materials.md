# ADR-0098: Textured VRM materials

- Status: Accepted
- Date: 2026-08-02

## Context

[ADR-0080](0080-avatar-crate-xdg-conformant-vrm-aware.md) established VRM as
an avatar source, but the renderer supplied one hard-coded Phong material for
the complete scene. The configured model stores its visible colors in
embedded PNG base-color textures; its glTF base-color factors are white. The
model therefore loaded and animated correctly while appearing as a white or
gray clay render.

Correct VRM output requires per-primitive material identity and glTF surface
semantics. The current avatar uses `KHR_materials_unlit`,
`KHR_texture_transform`, OPAQUE/MASK/BLEND alpha modes, alpha cutoff, and
double-sided primitives. Material loading must also preserve
[ADR-0097](0097-transactional-avatar-hot-reload.md): a partially saved image
or malformed material cannot replace the last-known-good avatar.

## Decision

Load VRM scenes through
`flux_scene_graph::Scene::from_glb_with_materials`. Pass the avatar's
offscreen color and depth formats through `MaterialTarget`, then draw with
`Scene::draw_materials` so every primitive uses its installed glTF material.

Treat the complete model, decoded base-color images, GPU samplers, materials,
mesh, rig, and motion library as one transactional build. A referenced image
decode error, unsupported texture-coordinate set, external image URI, GPU
upload failure, or material construction error fails the candidate build and
leaves the current avatar visible. Hot reload retries the same complete build;
it never falls back to a hard-coded white material.

The supported material subset is explicit: embedded PNG/JPEG sRGB base color,
UV0 and `KHR_texture_transform`, `KHR_materials_unlit`, glTF sampler state,
OPAQUE/MASK/BLEND with alpha cutoff, and double-sided rendering. The loader
uses bounded image dimensions and allocations. PBR auxiliary textures,
additional UV sets, and external resources remain load errors or future
Optics work rather than silent degradation.

## Alternatives

- **Tint the single Phong material.** Rejected because a constant tint cannot
  reproduce skin, eyes, hair, clothing, transparency, or per-primitive
  culling.
- **Decode textures directly in `aegis-avatar`.** Rejected because glTF
  material construction belongs in the reusable Optics scene content layer;
  duplicating it would diverge across Aegis consumers.
- **Ignore a material that fails and render that primitive white.** Rejected
  because silent partial rendering hides corrupt saves and defeats
  transactional hot reload.

## Consequences

- Colored VRM avatars render from their authored embedded textures while GPU
  skinning, named/random VRMA actions, head tracking, and the circular
  offscreen composition remain unchanged.
- `VrmError::Gltf` reports the richer `flux_scene_graph::LoadError`, including
  glTF validation, image decoding, unsupported material, and Flux GPU errors.
- Aegis local Optics development resolves the scene-graph loader's `gltf` and
  `image 0.25.8` dependencies through the worktree patch described by
  [ADR-0071](0071-worktree-isolated-cross-repository-development.md).
