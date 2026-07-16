# ADR-0036: Scoped semantic geometry and target-local input

- Status: Accepted
- Date: 2026-07-16

## Context

The structured agent path can focus, close, minimize, and move windows between
workspaces, but it cannot place a floating window at an exact rectangle. The
existing `Move` command starts a pointer-dependent interactive grab, so it is
not deterministic automation.

Some application tasks also require a click, scroll, pointer move, or key
press. Reusing the broad `control` capability for synthetic input would grant
more authority than semantic window management. Accepting raw press and
release events across independent requests could also leave a key or button
held after a client failure.

[ADR-0031](0031-agent-as-scoped-ipc-client.md) keeps pixel capture behind a
separate perceptual-path decision. The current launcher backdrop is a private
GPU render target, not a capture API. Creating a capture crate before there is
an accepted privacy model, transport, or real implementation would establish
an unused boundary.

## Decision

Add two scoped IPC operations to the existing automation seam.

`SetWindowGeometry` accepts a durable window id and a rectangle in compositor
logical coordinates. It detaches the window from layout policy, clears
maximized and fullscreen state, applies client size hints, and posts the
resulting configure. The operation uses the existing `control` capability and
a dedicated `SetWindowGeometry` operation class.

`InjectInput` accepts a durable target window id and at most 64 self-contained
actions. Pointer coordinates are local to the target window. The first action
set contains pointer move, click, scroll, and key press; every click and key
press includes its release in the same request. The compositor validates the
whole batch before applying any event, rejects hidden, stale, covered, or
shell-occluded targets, and bypasses compositor global key bindings.

Add a separate `input` capability. It is never granted to an unscoped
connection. A client must request a configured named scope whose operation and
window allowlists both permit `InjectInput`. Scope hot reload and fail-closed
resolution continue to follow
[ADR-0035](0035-fail-closed-named-ipc-scopes.md).
Unlike ordinary operations, omitting `ops` does not implicitly grant input;
the scope must list `InjectInput` explicitly.

Keep protocol major version 2. The new capability field defaults to false when
it is absent, so older version-2 peers negotiate input off. The new tagged
commands are additive.

Do not create `ass-ai`, `ass-automation`, or an empty capture crate. Semantic
types remain in `ass-core`, wire types remain in `ass-ipc`, and execution
remains in the server and main-loop input router. If the perceptual-path ADR
later approves capture, its GPU readback, `memfd`, portal, and PipeWire
dependencies form a coherent `ass-capture` crate at that time.

## Alternatives

- **Simulate window drag and resize.** Rejected because the result depends on
  pointer position, button lifetime, timing, and shell hit testing.
- **Put synthetic input under `control`.** Rejected because application input
  is materially broader than semantic window management.
- **Expose raw input event press and release commands.** Rejected because a
  disconnect can strand held state and separate requests are not atomic.
- **Allow unscoped input for compatibility.** Rejected because input injection
  has no legacy client that requires compatibility and must fail closed.
- **Create an automation or AI crate.** Rejected because it would split one
  protocol and execution path by caller identity, contrary to ADR-0031.
- **Create `ass-capture` now.** Rejected until a real capture implementation
  and its privacy-sensitive capability gate are accepted.

## Consequences

- External clients can place and resize floating windows without a pointer
  grab and observe the applied rectangle in the window snapshot.
- Scoped clients can perform bounded clicks, scrolls, pointer moves, and key
  presses without acquiring semantic control or session authority.
- Input injection reports queuing through the normal command response. Live
  target or shell-occlusion refusal is recorded in the mutation journal.
- Dragging inside client content, text entry, chords, pixel capture, and frame
  streaming remain follow-up work. Drag needs a main-loop action scheduler so
  the client can process the press before motion and release.
- The nested backend cannot move the host compositor's hardware pointer.
  Synthetic pointer state remains logical and is realigned before the next
  physical button or scroll event.
