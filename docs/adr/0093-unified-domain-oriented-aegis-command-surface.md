# ADR-0093: Unified domain-oriented Aegis command surface

- Status: Accepted
- Date: 2026-08-01

## Context

[ADR-0027](0027-ipc-and-introspection.md) establishes a versioned Unix-socket
IPC as the sole extension and automation seam. The reference client was
installed separately as `aegis-cli`, which made the transport and client
implementation part of the product vocabulary. Users had to choose between
`aegis` and `aegis-cli` before expressing the object they wanted to inspect or
change.

The command surface now spans displays, windows, workspaces, notifications,
live system state, Agent Workspaces, permissions, events, and the mutation
journal. A generic `cli`, `ctl`, or `msg` layer does not add domain meaning to
those typed operations. Flattening every operation at the top level would
remove that layer but would also mix unrelated verbs and make future growth
hard to discover.

The public command should feel like one native Aegis interface without moving
control into the compositor process or coupling the reusable command tests to
the graphics stack.

## Decision

`aegis` is the only installed command for both compositor startup and native
session management. Running `aegis` with no subcommand, or running
`aegis run`, starts the compositor. Resource-oriented subcommands address a
running session through the existing IPC socket:

- `aegis display`
- `aegis window`
- `aegis workspace`
- `aegis notification`
- `aegis journal`
- `aegis realm`
- `aegis permissions`
- `aegis system`

Resource nouns default to their inspect or list operation. Mutations add a
verb below the resource, such as `aegis window focus 42` or
`aegis workspace switch next`. Session-wide operations that are already
unambiguous remain direct commands, including `aegis events`,
`aegis overview`, and `aegis quit`.

`display` is the user-facing term for physical and virtual presentation
targets. The Wayland and model layers retain the precise internal term
`output`; command vocabulary does not rename protocol or schema types.

Domain commands execute as a separate client process and negotiate the same
capabilities as any other IPC client. Sharing the executable does not create
an in-process control path. Argument parsing occurs before logging, graphics,
or backend initialization, so help, completions, and client commands do not
enter compositor runtime code.

A lib-only `aegis-commands` crate owns argument parsing, IPC dispatch, output
formatting, and exit-code mapping. It remains independent of Flux, Lens,
Vulkan, and Wayland server code so its loopback tests stay in the flux-free CI
set. The crate does not install another executable.

Built-in owner-tool scope names use the product identity rather than the
removed client identity: `aegis-owner-admin`, `aegis-realm-admin`, and
`aegis-agent-admin`.

## Alternatives

- **Keep `aegis-cli`.** Rejected because it exposes an implementation role as
  a second product command and duplicates version, help, completion, and
  packaging surfaces.
- **Use `aegis ctl` or `aegis msg`.** Rejected because both name the mechanism
  between the command and compositor rather than the display, window,
  workspace, or Realm the user intends to operate.
- **Flatten all verbs at the top level.** Rejected because commands such as
  `focus`, `dismiss`, `capture`, and `switch` lose their resource context and
  create a crowded namespace.
- **Move the command implementation directly into the compositor crate.**
  Rejected because the parser and IPC client do not require the graphics stack
  and must remain independently testable.
- **Install a lightweight public dispatcher plus a private compositor
  executable.** Deferred because it preserves a small recovery binary at the
  cost of another packaging and process-launch layer. It remains possible
  without changing the domain command grammar.

## Consequences

Scripts and documentation must migrate from `aegis-cli` verbs to the resource
hierarchy. No compatibility alias is installed during the pre-1.0 command
surface change. Shell completions are generated for `aegis` and include both
startup and management commands.

The installed `aegis` executable links the compositor graphics dependencies
even when invoked for a management command. The lib-only command layer stays
small and testable, but a broken dynamic graphics dependency can prevent the
public executable from serving as a recovery client. A future private
compositor executable can address that limitation without reintroducing a
second public command.

IPC framing, capability negotiation, JSON data, and the out-of-process
automation boundary remain unchanged. The renamed built-in scope strings are
a deliberate pre-1.0 compatibility break for clients that named those
first-party recovery scopes directly.
