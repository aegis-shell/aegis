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
| The model | `aegis-core` | Windows, workspaces, outputs, Realms, seats, authority, layout — the one truth |
| The compositor | `aegis-compositor`, `aegis-backend`, `aegis-render`, `aegis-shell` | Wayland, per-Realm input and output, the chrome host |
| The seam | `aegis-ipc` | Versioned JSON and sealed descriptors over a Unix socket; leases, scope, capture, and the journal |
| IPC clients | any number, all equal | Status bars, `aegis-ctl`, the agent, future bridges |
| Platform adapter | `aegis-mcp` (separate process and crate) | Named-scope Aegis tools and one bridge-managed Agent Realm over MCP |
| Agent product | fuji (in-tree `aegis-fuji`) | Providers, credentials, sessions, skills, permissions, and the CLI |
| Other skill and tool layers | external projects | Other model-specific adapters, prompts, and schemas |

The line that matters is between the seam and the clients. Above that line,
every consumer is equal: a status bar holding `query`, a CLI tool holding
`control`, an agent holding `control` under a named scope. There is no
"agent" code path inside the compositor. An agent connecting to the socket
is indistinguishable from any other client except by the capabilities and
scope it negotiated at the handshake.

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
on the wire. See [ADR-0027](../adr/0027-ipc-and-introspection.md).

**The mutation journal.** Every command and Realm authority decision — from
chrome, keybinding, IPC, or internal cleanup — is recorded with its real
connection origin and outcome in an append-only, subscribable log. Realm
entries also carry before/after authority revisions. The agent reconstructs
recent history, filters its own echoes, and distinguishes "the user did this"
from "I did this". See
[ADR-0033](../adr/0033-mutation-journal.md).

**Scoped capabilities and Realms.** The agent's authority is bounded by a
user-approved scope declared in the configuration file: which resources,
which operations. The compositor enforces the scope on every mutation and
capture and records refusals in the journal. An independent Realm adds a virtual output,
seat, focus, selection, grabs, and transferable interaction authority without
creating another compositor. The agent is bounded without becoming an
in-process special case. See
[ADR-0035](../adr/0035-fail-closed-named-ipc-scopes.md) and
[ADR-0040](../adr/0040-realms-seats-and-transferable-interaction-authority.md).

Together these close the gap [Vision and Scope](vision.md#the-agent-phase)
names: the implicit state a human-only compositor accumulates — focus
heuristics, unlogged mutations, hidden shortcuts — is made explicit and
queryable. There are no mutations the agent cannot see, and no actions the
agent can take that bypass the scope.

## Meeting the Current AI Ecosystem

The introspection contract is stable and model-free; the AI ecosystem above
it is neither. The bridge is one out-of-process adapter per integration
pattern, not a reimplementation per pattern. The compositor stays put.

| Current pattern | Representatives | Bridge shape |
|-----------------|-----------------|--------------|
| Function calling / tool use | Claude, GPT, Gemini, Qwen, Mistral | Each IPC request becomes a tool; the adapter translates between the model's tool-call schema and aegis's JSON. |
| Model Context Protocol | fuji, Claude Desktop, Cline, Cursor | `aegis-mcp` exposes snapshots, journals, and operations as scoped tools, with Realm pixels as MCP image content. |
| Vision-based computer use | Claude Computer Use, OpenAI Operator | Damage-driven Realm capture supplies correlated pixels and window-to-input mappings; bounded target-local actions enter the Realm's independent seat. |
| Agent SDKs | Claude Agent SDK, LangGraph, custom | The agent process uses an SDK; tools call through the IPC. The SDK is indifferent to the transport. |
| Local models | Ollama, llama.cpp, MLX | Same tool-calling interface, routed to a local endpoint. Smaller models benefit most from the structured path. |
| Multi-agent orchestration | CrewAI, AutoGen, sub-agents | Each agent is a separate connection with its own scope; the journal lets them observe each other. Scoped capabilities are what make this safe. |

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
without granting a client arbitrary physical-desktop input. A Realm observer
receives damage notifications and requests an atomic directed capture whose
pixels, layout placements, surface sizes, scale, and authority revision agree.
This remains a measured fallback: the structured model is cheaper and more
stable, while pixels require an explicit capability, live lease, and
fail-closed lock and lifecycle checks.

The physical user observes those actions through a separate trusted feedback
layer ([ADR-0048](../adr/0048-compositor-owned-agent-operation-feedback.md)).
An applied Agent pointer appears as a labeled circular crosshair, movement
trail, and click pulse over the human's read-only mirror; keyboard or hidden-
target activity becomes a background-operation pill. This is not the user's
XDG cursor and never changes it. The compositor emits the feedback only after
the Realm seat accepts the input, omits key contents, hides it on lock, and
keeps it out of directed Realm capture so the Agent cannot observe its own
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
- **No special agent client.** The agent connects as `control` under a
  scope, dispatches through the same main-loop handler as a status bar,
  and is refused the same way when it steps outside the scope.

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
- [ADR-0032](../adr/0032-durable-window-identifiers.md),
  [ADR-0033](../adr/0033-mutation-journal.md),
  [ADR-0040](../adr/0040-realms-seats-and-transferable-interaction-authority.md),
  [ADR-0041](../adr/0041-sealed-file-descriptor-pixel-transport.md), and
  [ADR-0042](../adr/0042-mount-scoped-realm-portals-and-cgroup-sandboxes.md)
  — the authority, observation, and isolation follow-ons.
- [Comparative Survey — Extension and Automation](comparative-survey.md#extension-and-automation)
  — the systems whose patterns aegis borrows from and rejects.
