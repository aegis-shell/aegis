# ADR-0088: Agent capability borrowing and runtime grants (amends ADR-0031, ADR-0034, ADR-0047, ADR-0087)

- Status: Superseded by [ADR-0090](0090-native-capability-broker-and-stateless-mcp-edge.md)
- Date: 2026-07-31

## Context

Agents reach the desktop as IPC clients of the `aegis-mcp` bridge
(ADR-0047, ADR-0087), with authority bounded by named scopes declared in
the compositor configuration (ADR-0034). Three properties of that model do
not hold up:

- **Provisioning does not fit an open MCP ecosystem.** Every agent
  deployment needs a hand-written `[[agent.scope]]` TOML entry before first
  use. The set of MCP clients (fuji, Codex, OpenCode, anything) cannot be
  assumed in advance, and asking users to edit compositor configuration per
  agent is unfriendly.
- **Scope names authenticate nothing.** Any local process can present any
  declared name at the handshake. A name is self-asserted, so it cannot key
  anything durable such as remembered user consent.
- **There is no runtime consent.** A scope is an all-or-nothing static
  allowlist: destructive and privacy-sensitive operations have the same
  silent grant as benign ones, and the user has no per-agent memory of what
  was allowed.

MCP itself has no authorization concept, so enforcement must live in the
compositor, the only authority. Unsandboxed local processes have no
verifiable OS identity: the socket is owner-only `0600`, and `SO_PEERCRED`
would identify the shared `aegis-mcp` bridge binary, never the agent behind
it. File and cmdline hashes bind to copyable, update-fragile content and
remain invisible to the compositor across the bridge. The project already
ships a trusted-path consent pipeline (`PickConfirm` → compositor chrome,
ADR-0086) that agents cannot see or manipulate.

## Decision

Adopt a three-layer model: **ceiling, identity, grant** — the Android
install-time/package/runtime structure, adapted to a topology where the
connecting process is a shared bridge and no OS-level agent identity
exists.

**Ceilings come from a compositor-held principal registry, not from
configuration.** Config-declared `[[agent.scope]]` entries are removed
(amends ADR-0034). A borrowing agent self-declares at `Hello`: a cosmetic
display label plus the operation families it wants. First contact opens the
pairing prompt — a capability checklist in compositor chrome — and the set
the user approves becomes the principal's ceiling, stored in
`$XDG_DATA_HOME/aegis/principals.json` (owner-only, atomic, fail-closed on
corruption). Administrator pre-provisioning uses `aegis-cli permissions`
against the registry over IPC, not TOML. Built-in scopes (`aegis-portal`,
`aegis-cli-realm-admin`) stay hardcoded for platform components. Labels are
cosmetic: they authenticate nothing, the user may rename any principal, and
impersonating a label yields only a fresh principal with empty grants.

**Identity is platform-issued at pairing.** On approval the compositor
issues a random principal id and credential; the registry keeps the SHA-256
digest, and the agent persists the credential in durable owner-only storage
and presents it at every handshake. Unrecognized credentials re-pair. A
label colliding with a different credential triggers a "different
installation" warning in the prompt (TOFU continuity). Built-in scopes
never pair; anonymous connections (`aegis-cli`) are unchanged; a new
`[agent] lockdown` (default `false`) strips privileged capabilities from
unpaired connections so the anonymous channel can be closed deliberately.

**Runtime grants bind (principal, operation).** A platform-owned dangerous
set — closing windows, Realm capture, Realm input injection, Realm
lifecycle, sandboxed launch — always requires an interactive grant on first
use, however the ceiling was approved: *Deny*, *Allow once*, *Allow
session*, *Always allow*. Persisted decisions live in
`$XDG_DATA_HOME/aegis/grants.json`; session grants live in memory and die
with the compositor or the managed Realm. Denials are cached per session to
prevent prompt spam. `tools/list` advertises the ceiling including gated
operations, because each bridge call reconnects and re-checks the grant.
Pairings, grants, revocations, renames, and ceiling changes are recorded in
the mutation journal.

The bridge drops `AEGIS_MCP_SCOPE` (the scope concept no longer exists for
agents) and gains `AEGIS_MCP_LABEL` and `AEGIS_MCP_DATA_DIR`. Mutation
calls use a 360-second read bound because a grant may block on the
compositor's 300-second interaction timeout.

**Threat model, stated plainly.** This protects the user from model and
agent behavior — prompt injection, hallucinated actions, over-eager tools.
A same-uid malicious process can steal a credential file and impersonate a
principal; that is the physical limit of unsandboxed desktop Linux, and the
mitigations are visibility (journal, permission manager) and one-click
revocation. The anonymous privileged channel remains for owner tools
unless `lockdown` is set. Sandbox-issued identity through the bwrap launch
path is follow-up work, not claimed here.

## Alternatives

- **Keep config-declared scopes as the only ceiling.** Rejected: per-agent
  TOML provisioning is unfriendly and cannot fit an open MCP ecosystem. The
  registry gives administrators the same control through a scriptable CLI
  without configuration edits.
- **Identity by scope name, executable hash, or cmdline hash.** Rejected:
  names are forgeable by one configuration line; hashes bind to file
  content that copies preserve and edits break, and the compositor never
  sees the agent process behind the bridge anyway.
- **Android-style app identity via peer credentials or desktop ids.**
  Rejected: terminal-spawned agents have no app id, and peer credentials
  identify the shared bridge binary for every agent alike.
- **polkit or a TCC-like external authority.** Not chosen: polkit's
  identity is uid/process-based with the same limits and adds D-Bus
  machinery without solving agent identity; the compositor is already the
  authority for every affected object.
- **Grants persisted in the agent product.** Rejected as an authority:
  agent-side permission configuration is a UX guardrail, not enforcement;
  only compositor-held grants make revocation authoritative.

## Consequences

- Protocol version 18: `Hello` carries the agent declaration and pairing
  reply, `Scope` gains `ask_ops`, management requests and responses are
  added, `JournalMutation::AgentAuth` records the authorization lifecycle.
  All in-tree clients upgrade together; pre-1.0, no compatibility aliases
  (ADR-0066).
- `[[agent.scope]]` is removed from configuration; old files fail to parse
  (`deny_unknown_fields`), a loud breaking change recorded in the
  changelog. The default-scope rename in ADR-0087's wake (`desktop-operator`)
  becomes moot: agents no longer present scope names.
- The zero-config path is: connect → capability-checklist pairing →
  approved ceiling → dangerous operations prompt at first use with
  once/session/always options. `tools/list` shrinks with the ceiling, which
  also trims model context and injection surface.
- New management surfaces: `aegis-cli permissions`
  (list/revoke/forget/rename/set-ceiling/register) and a Permissions
  section in the Agent Workspaces application.
- The bridge's Realm recovery lock and state move from per-scope to
  per-label, so two agents hold independent managed Realms concurrently.
- Follow-up work: credential rotation/repair, parent-process context as
  display metadata at pairing, sandbox-issued identity for agents launched
  through the Realm bwrap path.
