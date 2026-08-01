# aegis-ipc

`aegis-ipc` is the versioned query, control, event, and introspection protocol
for aegis.

## Responsibilities

- Define the JSON request, response, command, event, capability, lease, Realm,
  revisioned settings, live-system control, and scope schemas.
- Frame messages over a Unix-domain socket.
- Transfer large immutable capture PNGs as sealed `memfd` descriptors through
  `SCM_RIGHTS`.
- Provide synchronous client and threaded server adapters.
- Record and stream the bounded command/Realm/settings mutation journal with
  real connection origins and authority revisions.
- Carry the same serializable `aegis-core` models used inside the compositor.

## Boundaries

This crate defines and transports operations. It does not implement
window-management policy, persist compositor state, or decide whether a
command is successful beyond protocol and capability checks. The executable's
handler owns those effects.

## Runtime Effect

The compositor listens at `$XDG_RUNTIME_DIR/aegis.sock`. Clients negotiate the
current protocol version and capabilities before issuing requests. State
changes can be observed through coarse events or the ordered mutation journal.
Capability, lease, validation, scope, and live-state refusals are auditable
alongside successful commands and Realm actions. The socket is owner-only
(`0600`) and a second server cannot replace an active socket.

## Use

```rust
use std::path::PathBuf;

let socket = PathBuf::from(
    std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR is set"),
)
.join("aegis.sock");
let mut client = aegis_ipc::Client::connect(&socket)?;
let windows = client.windows()?;
```

Use `Client::connect_with` when a client needs explicit control or session
capabilities. Use `Client::connect_scoped` to request a named scope from the
compositor configuration; `Client::scope` returns the granted allowlist. The
separate `input` capability is available only on a named connection whose
scope grants the input operation and target window. Realm operations likewise
require a named scope, an explicit operation, and a live connection-bound
lease. An unknown explicit name is refused, and hot-reloaded restrictions
apply to existing named connections on their next operation and again before
capture descriptors are sent. Interaction-group Realm mutations authorize
every affected window, not only the nominated member. Prefer native `aegis`
resource commands for shell scripts and interactive inspection.

Persistent-settings clients call `Client::settings`, then submit a typed
action through `Client::apply_settings` with the revision they observed. The
result is returned only after the compositor main loop applies the action.
Live-system clients call `Client::system_status` and submit immediate controls
through `Client::apply_system_action`; both paths use the same
`aegis-core` model as compositor chrome.

## Related Documentation

- [Command-line reference](../../docs/reference/cli.md)
- [IPC reference](../../docs/reference/ipc.md)
- [IPC and introspection decision](../../docs/adr/0027-ipc-and-introspection.md)
- [Fail-closed named scopes](../../docs/adr/0035-fail-closed-named-ipc-scopes.md)
- [Scoped semantic automation](../../docs/adr/0036-scoped-semantic-automation.md)
- [Sealed pixel transport](../../docs/adr/0041-sealed-file-descriptor-pixel-transport.md)
- [System Settings identity and boundary](../../docs/adr/0056-system-settings-identity-and-boundary.md)
- [Status bar system controls](../../docs/adr/0060-statusbar-system-controls-and-live-system-ipc.md)
- [Workspace layout](../../docs/dev/project-layout.md)
