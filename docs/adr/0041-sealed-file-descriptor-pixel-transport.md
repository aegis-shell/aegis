# ADR-0041: Sealed file-descriptor pixel transport

- Status: Accepted
- Date: 2026-07-18
- Supersedes: ADR-0037

## Context

ADR-0037 placed a base64 PNG inside the length-framed JSON response. That
kept the first capture API simple, but base64 expands every frame, requires
another full-size allocation on both sides, and makes the JSON frame limit
the pixel-transport limit. Realm capture also needs layout metadata that is
atomically correlated with pixels and authority revision.

The IPC already runs over an owner-only Unix-domain stream. Linux can transfer
an immutable file descriptor on that stream without adding another ambient
endpoint.

## Decision

Capture responses contain bounded JSON metadata followed by one sealed
`memfd` transferred with `SCM_RIGHTS`. The metadata records the exact byte
length. The sender applies the seal, shrink, grow, and write seals before
delivery. The receiver checks the descriptor type, byte length, required
seals, and global payload bound before allocating or reading.

Output capture metadata contains the physical dimensions and PNG length.
Realm capture additionally contains the Realm, logical capture region,
virtual-output scale, window placements, target-local surface sizes, and
authority revision captured with the pixels.

The connection-bound lease and live scope must remain valid through final
descriptor delivery. Session lock, inactive seat, Realm pause or revocation,
and security-generation changes invalidate work that was already encoding.
The compositor thread performs the final authorization check.

The JSON codec retains its independent small-message limit. Pixel descriptors
have a 288 MiB bound, covering the Realm model's 256 MiB raw-frame limit plus
PNG overhead.

## Alternatives

- **Keep base64 JSON.** Rejected because it inflates memory and wire size and
  couples large pixels to the control-message framing limit.
- **Write a temporary file and return its path.** Rejected because pathname
  access is ambient, cleanup is racy, and it does not bind delivery to the
  authorized connection.
- **Send raw bytes after the JSON frame.** Rejected because the stream then
  needs another framing state and cannot make the received payload immutable
  before validation.
- **Start continuous PipeWire streaming.** Deferred. Streaming has different
  pacing and consent semantics; damage-driven Realm observers need bounded
  atomic snapshots first.

## Consequences

- Capture avoids base64 expansion and giant JSON allocations.
- IPC implementations must support ancillary file descriptors and Linux
  `memfd` seals.
- A response is complete only after both its JSON metadata and descriptor
  arrive.
- Realm observers can map captured pixels to target-local input without
  racing a later layout or authority revision.
- ADR-0037 remains the history of introducing scoped capture, but its payload
  transport and size limit no longer describe the protocol.
