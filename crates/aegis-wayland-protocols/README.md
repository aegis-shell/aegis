# aegis-wayland-protocols

`aegis-wayland-protocols` provides the canonical Wayland C ABI types and
generated extension-protocol interface tables shared by the nested backend
and compositor server.

The explicit `wayland` qualifier matters: this crate is not the owner of IPC,
MCP, authority policy, or other Aegis protocols.

## Responsibilities

- Define ABI-compatible `wl_interface`, `wl_message`, and `wl_array` types.
- Resolve protocol XML from the installed `wayland-protocols` package.
- Run `wayland-scanner` and compile generated interface tables once.
- Give client-side and server-side FFI the same Rust type identities.

## Boundaries

This crate contains immutable protocol metadata, not a Wayland client or
server. It does not connect sockets, dispatch objects, validate requests, or
manage compositor state.

## Related Documentation

- [Wayland server decision](../../docs/adr/0002-hand-rolled-wayland-server.md)
- [FFI soundness discipline](../../docs/adr/0006-ffi-soundness-discipline.md)
- [Workspace layout](../../docs/dev/project-layout.md)
