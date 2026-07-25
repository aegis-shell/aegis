# ADR-0047: Neenee Agent Realm platform bridge (amends ADR-0031)

- Status: Accepted
- Date: 2026-07-22

## Context

[ADR-0031](0031-agent-as-scoped-ipc-client.md) keeps inference, prompts,
tools, and skills outside the compositor. ASS already supplies the authority
model needed by a system agent: named IPC scopes, leases, Agent Realms,
directed capture, independent seats, application sandboxes, optimistic
authority transfer, and fail-closed revocation.

Neenee is the existing product that owns the user-facing agent. It already
owns provider selection, credentials, sessions, TUI, skills, and its MCP
connector runtime. Praxion is the reusable substrate beneath Neenee. Creating
a second `ass-ai` product in this workspace would duplicate Neenee's identity
and product policy, while putting ASS-specific tools in Praxion would make a
generic agent runtime depend on one desktop.

The remaining integration seam is platform-specific: translate Neenee tool
calls into the public ASS IPC without giving the model arbitrary Realm
authority or linking the agent stack into the compositor.

## Decision

Add `ass-neenee`, an out-of-process ASS platform bridge. Its
`ass-neenee-mcp` binary serves newline-delimited Model Context Protocol over
stdio, which Neenee loads through its existing MCP runtime.

The responsibility split is:

| Owner | Responsibility |
|-------|----------------|
| Neenee | Product identity, conversation/session UX, credentials, provider policy, skills, permissions, and MCP client lifecycle |
| Praxion | Reusable model, tool, run, protocol, and orchestration mechanisms used by Neenee |
| `ass-neenee` | MCP transport, ASS tool schemas, named-scope probing, and one bridge-managed Agent Realm |
| `aegis-ipc` and compositor | Authoritative desktop state, scopes, leases, Realm lifecycle, capture, input, sandboxed launch, and mutation execution |

`ass-neenee` depends only on the public ASS model, application catalog, and
IPC crates. It does not depend on Praxion because Neenee already owns that
side of the boundary. It does not depend on compositor implementation crates,
and the compositor binary does not link it. A Praxion change is appropriate
only for a reusable runtime or protocol defect, never for ASS policy.

The bridge requests a configured named scope and advertises only operations
present in the startup grant. Every call opens a fresh scoped connection, so
scope narrowing and lease enforcement remain authoritative. High-risk Realm,
capture, and input operations require explicit operation names; an omitted
operation allowlist never enables them.

The bridge owns exactly one Agent Realm per scope. The model cannot provide a
Realm id. Realm operations resolve the bridge-managed id internally, and
window authority can target only that Realm or the human fallback Realm.
Private state under `$XDG_RUNTIME_DIR/ass-neenee/` records the id for crash
recovery, and an advisory lock prevents two bridge processes from sharing the
same scope. Graceful shutdown revokes the Realm by default; explicit reset
also revokes it and atomically returns controlled groups to the human Realm.

Directed capture returns MCP image content plus placement metadata. Synthetic
input is bounded to 64 self-contained actions, a window id, and the managed
Realm seat. Application launch accepts only desktop ids present in the
current XDG catalog. Ordinary compositor commands remain queue
acknowledgments and require a later journal read or snapshot for verification;
Realm transactions return authoritative receipts.

This amends ADR-0031 only by shipping a first-party platform adapter in the
ASS tree. The adapter remains an ordinary scoped IPC client. The model-free
compositor, shared IPC path, and caller-independent server policy remain in
force.

## Alternatives

- **Keep `ass-ai` as a second standalone agent.** Rejected because Neenee
  already owns the product, provider, session, and skill layers.
- **Link Neenee or Praxion into the compositor.** Rejected because network,
  credentials, prompts, and model cadence would enter the
  stability-sensitive compositor process.
- **Add native ASS tools directly to Neenee.** Rejected as the primary seam
  because it couples release cycles and bypasses Neenee's existing dynamic MCP
  extension boundary.
- **Put ASS tools in Praxion.** Rejected because named scopes, windows, and
  Realms are platform policy rather than reusable agent mechanisms.
- **Let each tool accept a Realm id.** Rejected because it would turn model
  arguments into an authority selector and make cross-Realm mistakes likely.
- **Use full-output screenshots for visual fallback.** Rejected because
  another Realm and compositor chrome must stay outside the agent's visual
  authority.

## Consequences

- Neenee gains a product-native ASS tool surface without a second model
  configuration or credential namespace.
- The ASS workspace no longer needs a source dependency on Praxion; Neenee
  remains the Praxion-powered application.
- Scope configuration is the capability manifest. Broadening it requires a
  bridge restart so `tools/list` can expose newly granted tools; narrowing
  takes effect on the next call.
- Current Neenee clients that flatten MCP results to text can consume all
  structured metadata but do not yet forward image content into the model.
  The bridge therefore also exposes the same directed PNG through an
  owner-only runtime path for Neenee's built-in image reader. It still emits
  standard MCP image content and never weakens capture isolation.
- A bridge killed without a graceful MCP shutdown can leave its Realm active.
  The private recovery record allows the next bridge for that scope to adopt
  it, and `realm_reset` provides explicit cleanup.
