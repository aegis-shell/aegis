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
capabilities. Prefer `ass-ctl` for shell scripts and interactive inspection.

## Related Documentation

- [Command-line reference](../../docs/reference/cli.md)
- [IPC and introspection decision](../../docs/adr/0027-ipc-and-introspection.md)
- [Workspace layout](../../docs/dev/project-layout.md)
