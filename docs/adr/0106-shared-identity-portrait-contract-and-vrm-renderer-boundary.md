# ADR-0106: Shared identity portrait contract and VRM renderer boundary

- Status: Accepted
- Date: 2026-08-03

## Context

The original avatar crate combined account-adjacent XDG discovery, still-image
decoding, source precedence, filesystem observation, VRM/VRMA animation, a
fixed portrait camera, and GPU rendering. Lock and command-panel surfaces also
implemented local account lookup independently. This made `avatar` mean both
an identity portrait policy and a VRM renderer, while presentation callers
could not choose camera composition.

Moving static portraits out of the VRM renderer must not remove the feature or
cause each caller to invent a different filename order. Initial selection,
live reload, account metadata, and compatibility paths require one shared
contract across every identity presentation. Outer discs, keylines, and
fallback initials remain properties of each host's chrome.

This decision supersedes the responsibility split in
[ADR-0080](0080-avatar-crate-xdg-conformant-vrm-aware.md) and preserves the
motion and transactional replacement behavior in
[ADR-0096](0096-avatar-motion-library-and-semantic-playback.md) and
[ADR-0097](0097-transactional-avatar-hot-reload.md).

## Decision

Create `aegis-identity` as the shared local-account and portrait policy layer.
It owns `Identity`, the ordered `PortraitConfig`, canonical Aegis and
freedesktop still-image paths, still-image texture preparation, selection
between still and VRM candidates, and `PortraitWatcher`. One immutable
`PortraitConfig` value drives initial loading, observation, and every reload.
An embedder can supply an explicit ordered candidate list through the same
type instead of duplicating resolution logic.

Narrow `aegis-avatar` to an explicit VRM renderer. It receives a VRM path, an
optional legacy VRMA path, and a caller-provided `VrmCamera`. It owns VRM and
VRMA parsing, humanoid retargeting, motion selection and time, skinning,
animated-head tracking, and the reusable offscreen texture. It does not load
still images, inspect account paths, choose source precedence, watch the
filesystem, or synthesize fallback portraits.

The camera contract contains vertical field of view, visible model-height
ratio, center-from-top ratio, and horizontal offset ratio. It has no renderer
default. Each host passes its profile and may replace it at runtime. The lock
screen and command panel own their outer portrait chrome and fallback visuals.

## Alternatives

- **Remove still portraits with the old avatar abstraction.** Rejected because
  the responsibility changes, not the user feature.
- **Move still lookup and decoding into every presentation caller.** Rejected
  because precedence, compatibility paths, and reload behavior would drift.
- **Keep a fixed camera in `aegis-avatar`.** Rejected because framing is a
  property of the caller's viewport and intended composition.
- **Put account, static, VRM, and chrome behavior in one larger avatar crate.**
  Rejected because it preserves the ambiguous boundary and couples identity
  configuration to one rendering implementation.

## Consequences

- Lock, command panel, and future consumers select the same account and
  portrait source while retaining independent presentation chrome.
- Static images remain supported through `aegis-identity`; callers do not
  need a static branch in the VRM renderer.
- VRM camera changes are explicit API changes at each call site and invalid
  parameters fail before a model is published.
- Hot reload continues to retain the last-known-good GPU resource, but the
  watcher and transactional orchestration now live with source policy.
- Consumers that previously called `aegis-avatar` discovery APIs migrate to
  `PortraitConfig`, `Portrait`, and `PortraitWatcher`.
