# ADR-0078: Out-of-process idle policy and session lock

- Status: Accepted
- Date: 2026-07-29

## Context

Aegis needs inactivity handling, lock-before-sleep coordination, display
power management, and an authentication surface. These functions cross a
security boundary: normal desktop content must disappear before power or
sleep transitions continue, input must reach only the locker, and a locker
failure after the boundary must never reveal the previous session.

The compositor already owns Wayland protocol state, input routing, output
hardware, idle inhibition, and the final rendered scene. Authentication and
lock-screen presentation do not need that authority. Idle policy changes
more frequently than compositor protocol behavior and must be restartable
without destabilizing the desktop.

## Decision

Keep the irreducible security state in the compositor and implement two
separate first-party session clients:

- `aegis-lock` is an `ext-session-lock-v1` client. It owns one lock surface
  per output, the Aegis lock-screen presentation, bounded credential entry,
  and PAM authentication.
- `aegis-idle` is an `ext-idle-notify-v1` client. It owns the ordered dim,
  lock, display-off, and suspend policy, coordinates logind sleep delay, and
  starts the locker.
- `aegis` supervises the idle client and supplies its validated persistent
  policy. The compositor remains the authority for session-lock protocol
  state, exclusive input routing, idle inhibitors, physical output power,
  and fail-closed rendering.

The locker signals readiness only after the compositor confirms the session
lock. Display power-off, suspend, and release of the sleep delay inhibitor
wait for that signal. If the locker exits after confirmation, the compositor
keeps an opaque fail-closed frame. If the idle coordinator fails while
outputs are off, the compositor wakes them without unlocking.

The default implementation invokes fixed first-party programs and fixed host
interfaces. The persistent policy contains data, not arbitrary commands.

## Alternatives

- **Put idle policy and authentication inside the compositor.** Rejected
  because PAM, credential handling, UI rendering, brightness tools, and
  logind failure modes would expand the compositor process and its trusted
  crash surface.
- **Combine idle policy and the lock screen in one client.** Rejected because
  policy reloads and host power-service failures should not share the
  authentication UI lifecycle. Separate clients also keep lock invocation
  useful when automatic idle actions are disabled.
- **Run configurable shell commands at each idle stage.** Rejected because it
  turns persistent desktop settings into an execution surface and cannot
  enforce lock confirmation before power transitions.
- **Delegate the complete feature to an arbitrary external locker and idle
  daemon.** Rejected as the product default because Aegis could not guarantee
  its presentation, readiness handshake, configuration transaction, or
  fail-closed behavior. The standard Wayland protocols remain available to
  compatible clients.

## Consequences

- Core packages must install `aegis-lock`, `aegis-idle`, and an
  `aegis-lock` PAM service profile with the compositor.
- Lock-screen presentation or policy crashes are isolated and supervised,
  while protocol and output authority remain in one place.
- Direct sessions can dim hardware backlights, power down outputs, and
  suspend through host services. Nested sessions retain locking but leave
  host brightness, output power, and sleep ownership to the outer desktop.
- Authentication follows the host PAM policy. Packaging must validate that
  the selected PAM stack provides both authentication and account policy.
- The lock client remains part of the trusted session path even though it is
  out of process; its dependencies and credential handling require security
  review.

See [Architecture](../explanation/architecture.md) for the component model
and the [Configuration Reference](../reference/config.md#idle-and-locking)
for policy fields.
