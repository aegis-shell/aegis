# ADR-0027: IPC and introspection seam

- Status: Accepted
- Date: 2026-06-23

## Context

[Vision and Scope](../explanation/vision.md#design-principles) commits ass
to one model read by many consumers and to "configuration is data, not
code". The [comparative survey](../explanation/comparative-survey.md#extension-and-automation)
records the durable pattern behind those principles: out-of-process IPC is
the versionable extension surface; in-process scripting is powerful but
brittle. sway and river configure and inspect entirely through an IPC;
niri has no in-process scripting and exposes state to external tools; GNOME
and KDE pay ongoing maintenance cost for in-shell JavaScript and QML
scripting. macOS routes automation through AppleScript, Shortcuts, and the
Accessibility API, all out of process.

The agent phase in [Vision and Scope](../explanation/vision.md#the-agent-phase)
needs the same structured, queryable surface a power-user tool needs: stable
identifiers for windows, workspaces, and outputs; a way to subscribe to
changes; and a way to issue mutations with bounded effect. Building that
surface twice — once for power users, once for the agent — is the failure
mode this decision exists to avoid.

## Decision

ass exposes a **versioned, schema-driven IPC over a unix domain socket** at
`$XDG_RUNTIME_DIR/aegis.sock`. The IPC is the sole extension and automation
surface. There is no in-process scripting.

The wire format is a fixed, schema-versioned request/response envelope with
a separate event stream. Requests and responses are framed messages; events
are server-pushed subscriptions. Every message is described by an explicit
schema with a major version, so a client written against version N continues
to work against N.x and fails loudly against (N+1). Schema and reference
client bindings live in a new `aegis-ipc` crate that depends on `aegis-core` for
the model types and nothing else.

The IPC exposes the **same model the chrome reads**. A `Window`, a
`Workspace`, and an `Output` are the same types in the IPC as in the
renderer; the IPC does not reconstruct them. Operations the user can perform
through the chrome — focus, close, move, change workspace, set layout role
([ADR-0024](0024-layout-model.md)) — are the same operations the IPC exposes.

Three capability classes bound what a client may do: **query** (read state
and subscribe to events, always allowed), **control** (mutate windows,
workspaces, and input focus), and **session** (quit, reload configuration,
change outputs). A client declares the classes it needs at connect time and
the compositor may refuse or downgrade. The agent in M10 connects as a
`control`-class client under a user-approved scope; a status bar connects as
`query`.

The configuration system ([ADR-0026](0026-configuration-system.md)) is the
**persistent** source of truth; the IPC is the **live** surface. An IPC
mutation updates live state and, where it makes sense, is reflected back to
the configuration file; transient state that is not part of the
configuration is not persisted.

## Alternatives

- **D-Bus.** Rejected as the transport: it is the freedesktop.org default
  but pulls in a large dependency and a system bus model that does not match
  a single-user compositor. A custom unix socket with a schema-versioned
  envelope gives the same properties at lower cost. A D-Bus bridge is
  implementable later as a separate process over this IPC if a desktop
  integration ever needs it.
- **The i3 IPC protocol.** Rejected as the wire format: it is widely tooled
  but JSON-over-socket with a small fixed message set does not version
  cleanly, and adopting it inherits its limitations as permanent
  constraints. The i3 IPC remains a useful study for message shape.
- **In-process scripting (JavaScript, QML, Lua).** Rejected outright: it
  contradicts [Vision and Scope](../explanation/vision.md#design-principles),
  repeats the GNOME/KDE maintenance pattern, and gives an agent an unsafe
  surface when a bounded IPC capability is exactly what an agent needs.
- **Accessibility-API-style out-of-process automation (macOS).** Rejected as
  the primary surface: it is observability-heavy and control-light, and the
  agent needs both. Accessibility is addressed separately as an output path
  in M9, not as the extension seam.
- **Reusing the Wayland protocol itself as the IPC.** Rejected: Wayland
  objects describe client surfaces and input, not compositor state, and
  stretching them to carry introspection inverts the protocol's direction.

## Consequences

- A new `aegis-ipc` crate owns the schema, the codec, and the reference
  client; the binary owns the server. The crate depends on `aegis-core` and
  not on flux, lens, or Wayland, so it is unit-testable and reusable.
- The chrome, the IPC, and the agent all read the same `aegis-core` snapshot,
  which is the one-model principle made concrete; no consumer reconstructs
  state.
- Every window-manager operation the chrome can trigger must also be
  expressible as an IPC request, which becomes a design constraint on new
  chrome intents: if it cannot be expressed over the IPC, it does not belong
  in the chrome either.
- The capability model is the security boundary for M10; designing it now,
  before the agent exists, avoids retrofitting a boundary onto a permissive
  surface.
- Versioning is a first-class concern: a schema bump is a real event with a
  migration note, not a silent change, because external tools and the agent
  both depend on it.
