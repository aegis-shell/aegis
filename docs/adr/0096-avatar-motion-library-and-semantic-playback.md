# ADR-0096: Avatar motion library and semantic playback

- Status: Accepted
- Date: 2026-08-02

## Context

[ADR-0080](0080-avatar-crate-xdg-conformant-vrm-aware.md) established one
optional `avatar.vrma` companion for a VRM avatar. That contract can express
only one continuously looping movement. It cannot distinguish ambient motion
from a gesture, request a gesture by semantic name, or vary repeated activity
without selecting random frames during rendering.

Avatar rendering has multiple consumers. The lock screen can greet the user,
the command panel can vary its opening gesture, and future surfaces can map
their own events to motions. These consumers need one shared selection policy
without owning VRMA parsing or file discovery. Selection must remain separate
from frame sampling so a chosen clip renders deterministically from elapsed
time.

## Decision

Extend `aegis-avatar` with an XDG motion library rooted beside `avatar.vrm`:

- `motions/idle/*.vrma` contains ambient clips selected automatically.
- `motions/actions/*.vrma` contains clips selected explicitly by stable file
  stem or randomly on request.

Motion names are lowercase ASCII identifiers that begin with a letter and may
also contain digits, hyphens, and underscores. Names are unique across both
pools. The library loads and validates every clip with its VRM scene so
configuration failures are reported at startup instead of appearing during a
later interaction.

Each pool uses its own shuffle bag. A bag plays every member once before it is
refilled and prevents the last member of one bag from becoming the first
member of the next. Idle clips advance automatically. An action plays once,
then playback returns to the idle pool or the model's rest pose when no idle
clip exists. Random selection occurs only at clip boundaries or explicit
requests; sampling the selected clip remains a pure function of elapsed time.

Keep `avatar.vrma` as a compatibility fallback. It behaves as a one-member
idle pool only when the new motion-library directories contain no clips. A
configured motion library takes precedence and never mixes implicitly with
the legacy companion.

Expose motion metadata, current-motion inspection, named playback, and random
action playback through the avatar crate. Consumers attach semantics rather
than paths: the lock screen requests `greeting` with a random fallback, while
the command panel requests a random action whenever it opens.

## Alternatives

- **Randomly choose among every VRMA file.** Rejected because ambient motion
  and gestures have different semantics; surprising actions would appear
  without a matching user event.
- **Encode roles in one TOML manifest.** Rejected for the initial contract
  because the directory split expresses the only required role distinction
  without adding a second configuration parser. A future manifest can add
  richer metadata without changing the two pools.
- **Infer behavior from names such as `idle-*`.** Rejected because a filename
  typo would silently change playback behavior. Directory membership is an
  explicit role assignment.
- **Load actions lazily.** Rejected because the first interaction could stall
  or fail. Eager loading makes the avatar configuration atomic and keeps
  action requests free of file I/O.

## Consequences

- A motion library occupies
  `$XDG_DATA_HOME/aegis/avatars/motions/{idle,actions}/`; the legacy companion
  requires no migration.
- Multiple decoded clips consume more memory and startup work than one clip,
  in exchange for predictable interaction latency and early validation.
- Consumers can request stable semantic names without knowing filesystem
  paths or animation internals.
- The shuffle-bag policy provides variation without immediate repeats, while
  explicit actions remain event-driven and return cleanly to ambient motion.
