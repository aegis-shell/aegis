# The Agent Phase

ass is built in two phases. The first is a desktop for human users. The
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
phase: own the model, own the IPC, present frames. It does not gain a
model, a prompt, a tool runtime, or a skill layer. Everything AI-specific
lives out of process, on the other side of the introspection IPC.

The stack, from the rendering layer up:

| Layer | Owner | What lives here |
|-------|-------|-----------------|
| Rendering and UI | flux, lens (out of tree) | Vulkan presentation; immediate-mode chrome drawing |
| The model | `ass-core` | Windows, workspaces, outputs, focus, layout — the one truth |
| The compositor | `ass-server`, `ass-backend`, `ass-render`, `ass-shell` | Wayland, input, output, the chrome host |
| The seam | `ass-ipc` | Versioned JSON over a unix socket; capabilities and scope; the journal |
| IPC clients | any number, all equal | Status bars, `ass-ctl`, the agent, future bridges |
| Skill and tool layer | out of tree, many projects | Model-specific adapters, prompts, schemas |

The line that matters is between the seam and the clients. Above that line,
every consumer is equal: a status bar holding `query`, a CLI tool holding
`control`, an agent holding `control` under a named scope. There is no
"agent" code path inside the compositor. An agent connecting to the socket
is indistinguishable from any other client except by the capabilities and
scope it negotiated at the handshake.

This is the deliberate inversion of most "AI desktop" projects, which bake
the model into the shell. ass bets the other way: the compositor is the
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

**The mutation journal.** Every mutation the compositor applies — from
chrome, keybinding, IPC, or internal cleanup — is recorded with origin and
outcome in an append-only, subscribable log. The agent reconstructs recent
history, filters its own echoes, and distinguishes "the user did this"
from "I did this". See
[ADR-0033](../adr/0033-mutation-journal.md).

**Scoped capabilities.** The agent's authority is bounded by a
user-approved scope declared in the configuration file: which resources,
which operations. The compositor enforces the scope on every command and
records refusals in the journal. The agent is bounded without being
special. See [ADR-0034](../adr/0034-scoped-capabilities.md).

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
| Function calling / tool use | Claude, GPT, Gemini, Qwen, Mistral | Each IPC request becomes a tool; the adapter translates between the model's tool-call schema and ass's JSON. |
| Model Context Protocol | Claude Desktop, Cline, Cursor | One adapter exposes IPC operations as MCP tools, the model snapshot as MCP resources, and the journal as MCP subscriptions. |
| Vision-based computer use | Claude Computer Use, OpenAI Operator | Requires pixel capture and input injection — the deferred perceptual path; the fallback for applications the structured model cannot reach. |
| Agent SDKs | Claude Agent SDK, LangGraph, custom | The agent process uses an SDK; tools call through the IPC. The SDK is indifferent to the transport. |
| Local models | Ollama, llama.cpp, MLX | Same tool-calling interface, routed to a local endpoint. Smaller models benefit most from the structured path. |
| Multi-agent orchestration | CrewAI, AutoGen, sub-agents | Each agent is a separate connection with its own scope; the journal lets them observe each other. Scoped capabilities are what make this safe. |

The fit with the Model Context Protocol is unusually clean. ass's
introspection surface and MCP converged independently on the same shape:
the versioned schema against tool schemas; capabilities and scope against
authorization; the journal against resource subscriptions; the typed model
against resources. An adapter between them is a thin translation, not a
re-architecture. This is not coincidence: both are answers to the same
question — how does an out-of-process agent address a system it did not
build? — and the answer is the same shape both times.

The vision-based computer-use pattern is the exception. It needs what the
structured path deliberately does not provide: raw pixels and arbitrary
input injection. ass treats this as a measured fallback, opened only when
the structured path plus the M9 accessibility output demonstrably cannot
cover a class of agent tasks. The default bet is that structure plus
accessibility is enough; pixels earn their own capability gate and their
own ADR if and when the bet fails.

## The Strategic Bet

**Structured introspection plus bounded mutation is the durable contract;
models and tool protocols churn above it.**

This is the same bet Wayland made against X11 — stable protocol, competing
compositors — applied to agents. If the Model Context Protocol becomes the
standard, ass is already the right shape. If something replaces it, the
adapter changes; the compositor does not. If vision models become good
enough that structured APIs look obsolete, the journal, the scope, and the
durable identifiers still matter: a vision model also needs to know what
it did and to be bounded while doing it.

The agent phase is not a feature. It is the claim that a compositor built
for humans — with its state made explicit, its mutations journaled, and
its surface bounded — is most of what an agent needs. The remaining work
is small, structural, and on the right side of the seam.

## What ass Does Not Do

The shape is defined as much by what it refuses as by what it adds.

- **No model inside the compositor.** The compositor never calls a model.
  Inference, prompt assembly, and tool selection live out of process.
- **No prompt storage in tree.** Prompts live in the skill layer,
  version-controlled and model-specific.
- **No retrieval index inside the compositor.** If the agent needs semantic
  search over its history, the agent indexes the journal; the compositor
  provides the data, not the index.
- **No "AI features" in the chrome.** No smart placement, no AI launcher.
  The chrome is for humans; the agent is another client.
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
- [ADR-0032](../adr/0032-durable-window-identifiers.md),
  [ADR-0033](../adr/0033-mutation-journal.md),
  [ADR-0034](../adr/0034-scoped-capabilities.md) — the three follow-ons.
- [Comparative Survey — Extension and Automation](comparative-survey.md#extension-and-automation)
  — the systems whose patterns ass borrows from and rejects.
