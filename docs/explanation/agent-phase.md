# The Agent Phase

aegis is built in two phases. The first is a desktop for human users. The
second adapts the same compositor so an agent can understand and operate
the machine through it. This page is the blueprint for the second phase:
how the pieces fit, why the shape is what it is, and how the result meets
the current AI ecosystem. The intent is fixed in
[Vision and Scope](vision.md#the-agent-phase); the decisions are recorded
in [ADR-0031](../adr/0031-agent-as-scoped-ipc-client.md) and its
follow-ons; the milestone sequence is in
[Roadmap](roadmap.md#m10-the-agent-phase).

## Where the Compositor Stops

The compositor's job in the agent phase is the same as in the desktop
phase: own the desktop model, own the IPC, present frames. It does not gain
an inference model, a prompt, a tool runtime, or a skill layer. Everything
AI-specific lives out of process, on the other side of the introspection
IPC. The `aegis-mcp` integration follows this boundary while shipping
the platform adapter in the same distribution
([ADR-0047](../adr/0047-neenee-agent-realm-platform-bridge.md),
[ADR-0050](../adr/0050-fuji-agent-product-and-bridge-rename.md),
[ADR-0087](../adr/0087-aegis-mcp-standalone-platform-bridge-crate.md)).

The stack, from the rendering layer up:

| Layer | Owner | What lives here |
|-------|-------|-----------------|
| Rendering and UI | flux, lens (out of tree) | Vulkan presentation; immediate-mode chrome drawing |
| The model | `aegis-model` | Windows, workspaces, outputs, Interaction Domains, seats, semantics, and layout — the one truth |
| Security kernel | `aegis-security` | Actor authority policy and privacy-minimized, hash-chained audit mechanisms without a transport dependency |
| Semantic trust seam | `aegis-semantic` | Bounded application accessibility trees, provider ownership, window-namespaced node identities, and action routing |
| The compositor | `aegis-compositor`, `aegis-backend`, `aegis-render`, `aegis-shell` | Wayland, per-Interaction Domain input and output, the chrome host |
| The seam | `aegis-ipc` | Versioned JSON and sealed descriptors over a Unix socket; transport admission, scopes, capture, and the journal |
| Accessibility adapter | `aegis-atspi` (supervised separate process) | AT-SPI discovery, tree publication, live precondition recheck, and toolkit action dispatch |
| IPC clients | any number, all equal | Native `aegis` commands, the agent, future bridges |
| Platform adapter | `aegis-mcp` (separate process and crate) | Scoped Aegis tools and one bridge-managed Agent Interaction Domain over MCP |
| Agent product | `aegis-agent` (in-tree `aegis-agent`) | Providers, credentials, sessions, skills, permissions, and the CLI |
| Other skill and tool layers | external projects | Other model-specific adapters, prompts, and schemas |

The line that matters is between the seam and the clients. Above that line,
every consumer is equal: a status bar holding `query`, a CLI tool holding
`control`, an agent borrowing capabilities under its approved ceiling. There
is no model-runtime code path inside the compositor. An Agent connects as an
Actor: the compositor recognizes its credential-bound principal, connection
lifetime, capabilities, and owned resources, but knows nothing about its
prompt, model, or plan
([ADR-0102](../adr/0102-actor-scoped-semantic-observation-and-transactional-actions.md)).
The canonical boundary and vocabulary are recorded in
[ADR-0103](../adr/0103-actor-authority-and-interaction-domain-architecture.md).

This is the deliberate inversion of most "AI desktop" projects, which bake
the model into the shell. aegis bets the other way: the compositor is the
slowest-moving, most stability-critical layer; models and tool protocols
are the fastest-moving; coupling them either freezes the agent or
destabilizes the compositor.

## What the Agent Reads and Does

The contract the agent connects to has four parts, each the subject of its
own ADR.

**Durable identifiers.** Every window, workspace, and output has an id that
is never reused within the compositor's lifetime. The agent can cite a
window in a journal entry, a scope, or its own memory without the id later
referring to something else. See
[ADR-0032](../adr/0032-durable-window-identifiers.md).

**The structured model.** The agent reads the same window, workspace, and
output snapshot the chrome renders. It does not reconstruct state from
pixels unless it has to. The model is typed, versioned, and self-describing
on the wire. Authenticated Actors receive only observation families and
resources named in their ceiling. Inside an Interaction Domain, the compositor additionally
publishes stable semantic window roots with state, bounds, declared actions,
and revisions. Pixels are a separate fallback, not a source of semantic
authority. See [ADR-0027](../adr/0027-ipc-and-introspection.md) and
[ADR-0102](../adr/0102-actor-scoped-semantic-observation-and-transactional-actions.md).

**The mutation journal.** Every command and Interaction Domain authority decision — from
chrome, keybinding, IPC, or internal cleanup — is recorded with its real
Actor principal and outcome in an append-only log. Interaction Domain entries carry
before/after authority revisions; Actor-action entries carry the semantic
target and committed action id without the observation bearer token. An
Actor's journal query is principal- and resource-filtered, so it reconstructs
its own recent history without learning another Actor's events. The bounded
projection is backed by an owner-only, append-and-sync SHA-256 chained store;
action text, values, coordinates, key codes, bearer ids, and exact resource
details never enter it. See
[ADR-0033](../adr/0033-mutation-journal.md).

**Scoped capabilities and Interaction Domains.** The Agent's authority is bounded by its
credential-bound, user-approved capability ceiling: which observation and
action families, and which resources. Observation and action are independent.
The compositor refreshes and enforces the ceiling on every operation and
records refusals in the journal. An independent Interaction Domain adds a virtual output,
seat, focus, selection, grabs, and transferable interaction authority without
creating another compositor. See
[ADR-0035](../adr/0035-fail-closed-named-ipc-scopes.md) and
[ADR-0040](../adr/0040-realms-seats-and-transferable-interaction-authority.md).

**Observation-bound actions.** A semantic observation produces a random,
connection-bound, 15-second token. `ActInInteractionDomain` consumes it once and checks
that the principal, Interaction Domain authority revision, complete target state, declared
actions, active seat, and local coordinates still agree. A mismatch aborts
the complete bounded batch; success returns a main-loop commit receipt. This
is optimistic concurrency for GUI dispatch, not a promise to roll back state
inside an application after it handles an event.

Together these close the gap [Vision and Scope](vision.md#the-agent-phase)
names: the implicit state a human-only compositor accumulates — focus
heuristics, unlogged mutations, hidden shortcuts — is made explicit and
queryable. Every mutation exposed to an Actor is attributable and scoped,
and no Actor action bypasses the capability broker.

## Actor Context and Isolation

An Agent is not modeled as a pointer or keyboard. It is an Actor whose GUI
namespace is assembled from several independently enforceable contexts:

| Actor context | Compositor authority |
|---------------|----------------------|
| Identity | Principal id plus a TTL/idle-bounded Actor session tied to the live IPC connection; labels are cosmetic. |
| Capability | Separate observation and action operations, exact session-bound resource grants, and a connection-bound lease. |
| View | One directed Interaction Domain output and a filtered semantic tree; framebuffer capture is a separate capability. |
| Input | One Interaction Domain seat, focus, selection, grabs, event queue, and interaction-group authority. |
| Observation | Single-use semantic leases bound to the Actor, Interaction Domain, revision, and target state. |
| Storage and network | No host network or user-file mounts; exact access requires a grant-consuming broker. |
| Lifecycle | Actor-session TTL/idle expiry, disconnect cascade, pause, lock/VT suspension, and permanent Interaction Domain revocation. |
| Memory | A bounded compositor event journal; plans, prompts, indexes, and long-term memory remain in the Agent runtime. |

This is a microkernel boundary. The compositor owns identity, routing,
isolation, optimistic locking, and the audit log. The out-of-process runtime
owns reasoning and planning. Per-Actor seats remove device-input races;
authority and semantic revisions detect the remaining human/Agent operation
races without freezing unrelated Actors behind a global GUI lock.

## Meeting the Current AI Ecosystem

The introspection contract is stable and model-free; the AI ecosystem above
it is neither. The bridge is one out-of-process adapter per integration
pattern, not a reimplementation per pattern. The compositor stays put.

| Current pattern | Representatives | Bridge shape |
|-----------------|-----------------|--------------|
| Function calling / tool use | Claude, GPT, Gemini, Qwen, Mistral | Each IPC request becomes a tool; the adapter translates between the model's tool-call schema and aegis's JSON. |
| Model Context Protocol | fuji, Claude Desktop, Cline, Cursor | `aegis-mcp` exposes snapshots, journals, and operations as scoped tools, with Interaction Domain pixels as MCP image content. |
| Vision-based computer use | Claude Computer Use, OpenAI Operator | Damage-driven Interaction Domain capture supplies separately authorized pixels plus a semantic observation; bounded actions must consume its precondition token. |
| Agent SDKs | Claude Agent SDK, LangGraph, custom | The agent process uses an SDK; tools call through the IPC. The SDK is indifferent to the transport. |
| Local models | Ollama, llama.cpp, MLX | Same tool-calling interface, routed to a local endpoint. Smaller models benefit most from the structured path. |
| Multi-agent orchestration | CrewAI, AutoGen, sub-agents | Each Agent has a separate principal, connection, Interaction Domain, capability context, and filtered journal. Deliberate cooperation uses an explicit higher-level channel. |

The fit with the Model Context Protocol is unusually clean. aegis's
introspection surface and MCP converged independently on the same shape:
the versioned schema against tool schemas; capabilities and scope against
authorization; and the typed model and journal against structured tool
results. The current fuji adapter uses tools and image content rather than
MCP resources or subscriptions. It is still a thin translation, not a
re-architecture. This is not coincidence: both are answers to the same
question — how does an out-of-process agent address a system it did not
build? — and the answer is the same shape both times.

The vision-based computer-use pattern is the exception. Scoped, target-local
clicks, scrolls, pointer moves, and key presses cover bounded interaction
without granting a client arbitrary physical-desktop input. A directed Interaction Domain
capture correlates pixels, placements, semantic roots, scale, and authority
revision, but the later action must still consume the observation token and
pass a main-loop state check. This remains a measured fallback: the semantic
model is cheaper and more stable, while pixels require a separate capability,
live lease, and fail-closed lock and lifecycle checks.

The physical user observes those actions through a separate trusted feedback
layer ([ADR-0048](../adr/0048-compositor-owned-agent-operation-feedback.md)).
An applied Agent pointer appears as a labeled circular crosshair, movement
trail, and click pulse over the human's read-only mirror; keyboard or hidden-
target activity becomes a background-operation pill. This is not the user's
XDG cursor and never changes it. The compositor emits the feedback only after
the Interaction Domain seat accepts the input, omits key contents, hides it on lock, and
keeps it out of directed Interaction Domain capture so the Agent cannot observe its own
feedback loop.

## The Strategic Bet

**Structured introspection plus bounded mutation is the durable contract;
models and tool protocols churn above it.**

This is the same bet Wayland made against X11 — stable protocol, competing
compositors — applied to agents. If the Model Context Protocol becomes the
standard, aegis is already the right shape. If something replaces it, the
adapter changes; the compositor does not. If vision models become good
enough that structured APIs look obsolete, the journal, the scope, and the
durable identifiers still matter: a vision model also needs to know what
it did and to be bounded while doing it.

The agent phase is not a model feature. It is the claim that a compositor built
for humans — with its state made explicit, its mutations journaled, and
its surface bounded — is most of what an agent needs. Model adapters,
semantic element bridges, and streaming integrations remain on the other side
of the seam.

## What aegis Does Not Do

The shape is defined as much by what it refuses as by what it adds.

- **No model inside the compositor.** The compositor never calls a model.
  Inference, prompt assembly, and tool selection live out of process.
- **No prompt storage in the compositor.** Product prompts live in fuji or
  another out-of-process skill layer. Aegis ships only the platform tool
  contract and an optional fuji skill.
- **No retrieval index inside the compositor.** If the agent needs semantic
  search over its history, the agent indexes the journal; the compositor
  provides the data, not the index.
- **No model-driven chrome.** Agent Workspace controls expose human-owned
  security and authority state; they do not run inference, choose actions, or
  embed an agent in the shell.
- **No privileged input-device shortcut.** The Agent connects through the
  same IPC seam, but as an authenticated Actor with explicit observation and
  action capabilities. It is refused when its identity, lease, resource,
  observation, or action precondition does not hold.

One door is left unlocked. The chrome is a pluggable component
([ADR-0021](../adr/0021-chrome-component-trait.md)), so an agent could in
principle become the desktop surface — an agent-driven shell. This is not
the default composition; it is an option the architecture leaves open
without committing to.

## See Also

- [Vision and Scope — The Agent Phase](vision.md#the-agent-phase) — the
  intent this page makes concrete.
- [Roadmap — M10](roadmap.md#m10-the-agent-phase) — the milestone that
  delivers it.
- [ADR-0031](../adr/0031-agent-as-scoped-ipc-client.md) — the framing
  decision.
- [ADR-0047](../adr/0047-neenee-agent-realm-platform-bridge.md) — the MCP
  platform bridge that remains outside the compositor.
- [ADR-0050](../adr/0050-fuji-agent-product-and-bridge-rename.md) — the fuji
  rename and the in-tree, self-contained agent runtime.
- [ADR-0087](../adr/0087-aegis-mcp-standalone-platform-bridge-crate.md) —
  the bridge as the standalone `aegis-mcp` platform crate.
- [ADR-0048](../adr/0048-compositor-owned-agent-operation-feedback.md) — the
  trusted visual distinction between human and Agent input.
- [ADR-0102](../adr/0102-actor-scoped-semantic-observation-and-transactional-actions.md)
  — Actor contexts, semantic observations, and optimistic GUI transactions.
- [ADR-0103](../adr/0103-actor-authority-and-interaction-domain-architecture.md)
  — canonical Actor capabilities, Interaction Domain language, and crate ownership.
- [ADR-0032](../adr/0032-durable-window-identifiers.md),
  [ADR-0033](../adr/0033-mutation-journal.md),
  [ADR-0040](../adr/0040-realms-seats-and-transferable-interaction-authority.md),
  [ADR-0041](../adr/0041-sealed-file-descriptor-pixel-transport.md), and
  [ADR-0042](../adr/0042-mount-scoped-realm-portals-and-cgroup-sandboxes.md)
  — the authority, observation, and isolation follow-ons.
- [Comparative Survey — Extension and Automation](comparative-survey.md#extension-and-automation)
  — the systems whose patterns aegis borrows from and rejects.
