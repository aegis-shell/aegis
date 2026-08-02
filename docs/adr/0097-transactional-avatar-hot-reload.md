# ADR-0097: Transactional avatar hot reload

- Status: Accepted
- Date: 2026-08-02

## Context

The lock screen and command panel are long-lived processes that previously
resolved avatar sources only during construction. Changes under the XDG avatar
directory therefore required restarting the corresponding process, even
though these files are user-owned appearance data.

Avatar reload crosses two sensitive boundaries. Filesystem notifications run
on a worker thread, while Flux device, scene, and image resources belong to the
render thread. Editors may also expose a temporary missing, truncated, or
partially written file before completing an atomic rename. Replacing live GPU
state during either interval would cause thread-safety violations, visible
fallback flashes, or loss of the last-known-good avatar.

Aegis frame pacing is event driven and already has a one-second maintenance
wake. Avatar observation must not add continuous polling when sources are
unchanged.

## Decision

`aegis-avatar` owns an `AvatarWatcher` covering every path that can affect
avatar resolution: canonical XDG still images, `avatar.vrm`, the motion
library, the legacy companion VRMA, freedesktop face files, and opt-in debug
assets. Existing avatar trees are watched recursively. Parent guards remain
watched non-recursively so deleted or not-yet-created trees can be rearmed.

Filesystem callbacks filter unrelated and access-only events, then set an
atomic dirty flag. They do not decode files or touch GPU state. Consumers poll
that flag from their established render loop. The existing maintenance wake
bounds idle detection latency; once an event is observed, a 180-millisecond
trailing-edge debounce keeps the loop active until the source settles.

The lock screen and command panel rebuild the entire avatar on their render
thread with `Avatar::load_transactional`. They publish the replacement only
after decoding, motion discovery, scene construction, and GPU upload all
succeed. An existing but temporarily unusable source retains the current
avatar and receives up to three retries spaced 350 milliseconds apart. A
genuine deletion is distinct and deliberately installs the normal procedural
fallback. Watch targets are reconciled after each settled change.

A successful reload restores the current motion by stable name when the new
library still contains it. Otherwise the lock screen requests `greeting` with
its random-action fallback, while an open command panel requests a new random
action.

## Alternatives

- **Poll source metadata every frame or on a fixed timer.** Rejected because
  unchanged appearance data should consume no steady-state render or I/O work.
- **Decode directly in the filesystem callback.** Rejected because the
  callback thread cannot safely own Flux resources, and a long callback can
  delay later filesystem events.
- **Mutate the current avatar in place.** Rejected because a failure halfway
  through motion or GPU setup would leave a mixed resource graph. Building a
  replacement makes publication atomic at the Rust ownership boundary.
- **Immediately replace malformed input with the fallback.** Rejected because
  common save sequences temporarily expose incomplete data and would produce
  a visible flash despite a valid last-known-good avatar.

## Consequences

- Avatar, VRM, and VRMA changes become visible in the lock screen and command
  panel without restarting them, with up to one maintenance interval plus the
  debounce before a dormant surface notices a change.
- Filesystem observation remains CPU-only and cross-thread; all decode and GPU
  work stays serialized on each consumer's render thread.
- Reload briefly holds both old and new resources. This increases peak memory
  during replacement but guarantees last-known-good behavior.
- Consumers must include watcher deadlines in their animation-pending signal
  so debounce and retry attempts complete without permanent polling.
