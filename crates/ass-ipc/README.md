# ass-ipc

`ass-ipc` is the versioned query, control, event, and introspection protocol
for ass.

## Responsibilities

- Define the JSON request, response, command, event, capability, and scope
  schemas.
- Frame messages over a Unix-domain socket.
- Provide synchronous client and threaded server adapters.
- Record and stream the bounded mutation journal.
- Carry the same serializable `ass-core` models used inside the compositor.

## Boundaries

This crate defines and transports operations. It does not implement
window-management policy, persist compositor state, or decide whether a
command is successful beyond protocol and capability checks. The executable's
handler owns those effects.

## Runtime Effect

The compositor listens at `$XDG_RUNTIME_DIR/ass.sock`. Clients negotiate the
current protocol version and capabilities before issuing requests. State
changes can be observed through coarse events or the ordered mutation journal.
The socket is owner-only (`0600`) and a second server cannot replace an active
socket.

## Use

```rust
use std::path::PathBuf;

let socket = PathBuf::from(
    std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR is set"),
)
.join("ass.sock");
let mut client = ass_ipc::Client::connect(&socket)?;
let windows = client.windows()?;
```

Use `Client::connect_with` when a client needs explicit control or session
capabilities. Use `Client::connect_scoped` to request a named scope from the
compositor configuration; `Client::scope` returns the granted allowlist. The
separate `input` capability is available only on a named connection whose
scope grants `InjectInput` and the target window. An unknown explicit name is
refused, and hot-reloaded scope restrictions apply to existing named
connections on their next command. Prefer `ass-ctl` for shell scripts and
interactive inspection.

## Related Documentation

- [Command-line reference](../../docs/reference/cli.md)
- [IPC reference](../../docs/reference/ipc.md)
- [IPC and introspection decision](../../docs/adr/0027-ipc-and-introspection.md)
- [Fail-closed named scopes](../../docs/adr/0035-fail-closed-named-ipc-scopes.md)
- [Scoped semantic automation](../../docs/adr/0036-scoped-semantic-automation.md)
- [Workspace layout](../../docs/dev/project-layout.md)
