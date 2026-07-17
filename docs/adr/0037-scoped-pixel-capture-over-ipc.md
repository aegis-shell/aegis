# ADR-0037: Scoped pixel capture over the IPC

- Status: Accepted
- Date: 2026-07-17

## Context

[ADR-0031](0031-agent-as-scoped-ipc-client.md) left pixel-level capture open:
the structured agent path (ids, journal, scopes) was the default, and raw
pixels needed a privacy model before an API. The desktop phase has since
closed two prerequisites. First, the capture mechanism exists: flux owns
offscreen RGBA8 surfaces with CPU readback (`flux_surface_read_pixels`), and
the compositor re-renders the scene into one on demand, so a capture works
identically on the nested and the DRM/KMS backend. Second, the capability
model exists: named scopes with explicit operation allowlists
([ADR-0034](0034-scoped-capabilities.md),
[ADR-0035](0035-fail-closed-named-ipc-scopes.md)) already gate the
comparably sensitive `InjectInput` operation.

Two distinct callers want pixels. A human wants a screenshot written to a
file. An agent wants the frame delivered to itself, without filesystem
access, under a revocable scope.

## Decision

ass exposes pixel capture through two fail-closed IPC operations, sharing
one offscreen re-render path in the compositor:

1. **`Command::Screenshot { path }`** — a journaled `control` command that
   writes the focused output as a PNG file. It follows ordinary control
   scope rules and exists for the user's own tooling (`ass-ctl screenshot`).
2. **`Request::CaptureOutput` → `Response::CaptureOutput { width, height,
   png_base64 }`** — a synchronous query that returns the PNG over the IPC.
   It requires the `control` capability and an explicit `CaptureOutput`
   entry in the connection's scope `ops`, never inherited through the
   `None`-means-all default — the same fail-closed rule as `InjectInput`.

Both are refused while the session is locked or the seat is inactive: a
locked or VT-switched-away desktop yields no pixels. The PNG payload is
bounded by the codec's 16 MiB frame limit. The capture reflects exactly the
scene the user sees, including the overview grid when overview mode is
active.

## Alternatives

- **No pixel capture ever (ADR-0031's open default).** Rejected: the agent
  phase needs perception to close its loop, and the privacy risk is now
  bounded by the same scope machinery that already guards synthetic input.
- **A separate `ass-capture` crate with PipeWire/portal streaming.** Deferred,
  per [ADR-0036](0036-scoped-semantic-automation.md): frame streaming,
  `memfd`, and portal integration form a coherent crate when continuous
  capture is a real requirement; a single-frame PNG answers the agent's
  current needs without those dependencies.
- **`wlr-screencopy` for third-party tools.** Not chosen: it hands raw
  frames to any client that can bind the global, which bypasses the named
  scope model instead of reusing it.

## Consequences

- The agent phase's perception loop is unblocked: a scoped client can
  observe the desktop on demand and correlate pixels with the structured
  window model in the same protocol.
- `docs/reference/ipc.md` documents both operations; the "Capture" section
  supersedes its earlier "no pixel capture" statement.
- Screen streaming (screencast, `xdg-desktop-portal`) remains future work
  and will reuse this readback path; nothing here precludes it.
