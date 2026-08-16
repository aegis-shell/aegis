# ADR-0128: Peer-identity-bound built-in scopes and the capture indicator

- Status: Accepted
- Date: 2026-08-16

## Context

Built-in IPC scopes (`aegis-portal`, `aegis-owner-admin`,
`aegis-agent-admin`, `aegis-interaction-domain-admin`) are hardcoded
trusted-component grants
([ADR-0090](0090-native-capability-broker-and-stateless-mcp-edge.md)):
they never pair, and claiming one is a name lookup at `Hello`. The socket
is owner-only `0600` in `$XDG_RUNTIME_DIR`, so every same-uid process sits
behind the same file permission — and could claim any built-in scope by
name. Concretely, any local process could present `aegis-portal` and open
a `StreamOutput` frame stream of the whole screen with no consent, or
claim `aegis-owner-admin` and drive ordinary desktop mutations. The
`builtin` flag that exempts platform components from `[agent] lockdown`
was computed from the name alone, so the exemption shared the same hole.

[ADR-0088](0088-agent-capability-borrowing-and-runtime-grants.md) already
recorded the identity boundary this decision respects: unsandboxed local
agents have no verifiable OS identity — `SO_PEERCRED` would identify the
shared bridge binary, never the agent behind it — so agents authenticate
through the compositor-held principal registry and interactive pairing.
Peer identity can bind *platform components*, never agents; the agent path
is untouched.

While capture streams run, nothing in the shell says so. Consent for
ScreenCast is a one-time portal prompt; after that the user has no
persistent signal that the screen is still being recorded. GNOME shows an
always-on indicator for exactly this reason.

## Decision

Built-in scopes bind to kernel-verified process identity, and live capture
streams raise a persistent, non-interactive shell indicator. Protocol
version stays 29: the handshake vocabulary does not change — a refused
claim is the existing `Response::Error` — and the picked-output connector
below rides the additive-field convention already used for stream
renegotiation.

1. **Accept-time peer credentials, defense in depth.** The IPC accept loop
   reads `SO_PEERCRED` (pid/uid/gid) for every connection and refuses any
   peer whose uid differs from the compositor's effective uid, closing it
   before a byte is read. The `0600` socket already enforces this in
   practice; the kernel check keeps the property if the socket is ever
   relocated or its permissions weakened, and it yields the peer pid the
   scope gate needs.
2. **A built-in scope resolves only for its executables.** When `Hello`
   names a built-in scope, the dispatcher resolves the peer's identity —
   the canonicalized `/proc/<pid>/exe` (the kernel resolves every symlink),
   with any ` (deleted)` suffix stripped so a package upgrade mid-run does
   not sever a platform component — and the compositor admits the claim
   only when that path appears in the scope's allowlist. Any read failure
   fails closed: no identity, no built-in scope. The refusal answers
   `scope 'X' is not available to this process`, is logged, and is
   journaled before the reply like every other security decision — as a
   privacy-minimized `ScopeClaim { scope }` mutation that retains the
   scope name and never the peer's path or pid. The check runs before the
   scope resolves and before any session starts, and the `builtin`
   lockdown exemption is only evaluated afterwards, so the exemption can
   never again be name-only. Compiled-in allowlists: `aegis-portal` →
   `xdg-desktop-portal-aegis` in `/usr/bin`, `/usr/libexec`, `/usr/lib`,
   `/usr/local/bin`; the three admin scopes → the `aegis` CLI in
   `/usr/bin` and `/usr/local/bin`. Anonymous connections (no declared
   scope) and paired agents are unaffected — the identity gate exists for
   built-in scope claims only.
3. **`[ipc.scope_executables]` replaces the defaults per scope.** The new
   additive optional config table maps a built-in scope name to the exact
   executable paths that may claim it; a scope named there replaces its
   compiled-in defaults (an empty list refuses every claim — the table is
   fail-closed by construction), and a scope absent from the table keeps
   the compiled-in defaults. It needs no schema-version bump and follows
   the existing live reload: resolution reads the current map, so the next
   handshake after a reload sees the fresh table. Distribution-specific
   layouts (a portal backend installed under `/opt`, a renamed binary) are
   a configuration concern, not a rebuild.
4. **A live capture stream shows a recording indicator.** The runtime
   mirrors the live stream count into `SystemStatus::capture_streams`
   after every registry mutation — start, stop, disconnect, geometry or
   blit endings — and pushes it through the ordinary status channel. A
   trusted, non-interactive chrome pill (the Agent feedback pill's
   conventions, not the HUD's: it must not hide behind a fullscreen
   window) paints a recording marker in the design's critical emphasis
   color plus the localized live count while the count is at least one,
   and nothing at zero. It renders into the ordinary desktop composite, so
   it is visible inside the recording itself — the same self-reference
   GNOME's indicator has.
5. **`PickKind::Output` closes the output-pick loop.** The v29 projection
   already modeled an output pick, but the compositor had neither the kind
   nor a way to name the result. `PickKind::Output` drives the existing
   picker chrome in an output mode: the output under the cursor is
   highlighted (hit-tested against the model's output logical rects), a
   click or Enter answers
   `PickResult::Output { connector: Some(connector) }`, and Escape cancels
   like every picker. The window-mode whole-output path (Enter, or a click
   on empty desktop) keeps answering the bare `{"type":"Output"}` — the
   new `connector` field is `default` + `skip_serializing_if`, so the
   legacy shape round-trips in both directions and old clients parse new
   replies.

## Alternatives

- **Keep name-only built-in scopes** leaves any same-uid process one
  `Hello` away from streaming the screen. The socket's file mode is a
  perimeter, not an identity; defense in depth at the scope layer is the
  point of the built-in names.
- **Re-check the peer executable at every effect boundary** (stream
  frames, event delivery) instead of once at `Hello` would catch a process
  that re-`exec`s mid-connection, at the cost of a `/proc` read per
  boundary and a new failure mode (a legitimate `exec` silently killing a
  consented stream). The connection is the authority boundary: identity is
  verified when the claim is made, and the granted scope then lives and
  dies with the connection like every other binding.
- **Recording state in the HUD chips** would inherit the HUD's deliberate
  blind spots: the chips fade near the cursor and hide entirely behind a
  visible fullscreen window — exactly where a full-screen video or game
  runs while being captured. A security indicator must not step aside, so
  it follows the always-on Agent feedback pill instead.
- **A distinct `PickResult::OutputPicked` variant** for the connector
  answer would avoid the optional field but fork the wire vocabulary for
  one datum and break the portal's existing `Output` handling; extending
  the variant with an additive optional field is the protocol's own
  convention (protocol 29's stream selector).
- **Including the streaming client's scope name in the indicator** was
  considered and dropped: the scope name lives at the connection, not the
  stream registry, and plumbing it through `StreamOutputStart` buys a
  label the user cannot act on — every production stream today belongs to
  the one portal scope. The count is the actionable signal.

## Consequences

- A same-uid process can no longer claim a built-in scope by name: the
  portal scope belongs to the portal backend binary, the admin scopes to
  the `aegis` CLI, and every other claimant is refused at the handshake
  with a journaled `ScopeClaim` refusal. This closes the ambient
  screen-streaming hole without touching the agent pairing model —
  [ADR-0088](0088-agent-capability-borrowing-and-runtime-grants.md)'s
  boundary holds: peer identity binds platform components, agents still
  authenticate by credential and pairing.
- The lockdown exemption now means "name *and* identity matched"; a
  refused built-in claim is a closed connection, not a silently stripped
  anonymous session.
- Deployments with non-standard install layouts configure
  `[ipc.scope_executables]` instead of patching the compositor; a
  misconfigured or emptied allowlist fails closed (every claim refused),
  never open.
- The user sees a persistent, localized recording pill with the live
  stream count whenever anything streams the screen or a window — during
  fullscreen content too — and the compositor's own captures and one-shot
  screenshots stay unannounced (they are single user-initiated acts, not
  standing streams).
- `PickKind::Output` lets the portal drive a real per-output picker and
  receive the connector, while every pre-existing window-mode whole-output
  answer keeps its exact legacy wire shape.
- Follow-up work this decision creates: none required; a distribution
  package only needs to ship its `[ipc.scope_executables]` stanza if its
  layout differs from the compiled-in defaults.
