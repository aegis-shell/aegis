# ass-protocols

`ass-protocols` provides the canonical Wayland C ABI types and generated
extension-protocol interface tables shared by the nested backend and Wayland
server.

## Responsibilities

- Define ABI-compatible `wl_interface`, `wl_message`, and `wl_array` types.
- Resolve protocol XML from the installed `wayland-protocols` package.
- Run `wayland-scanner` and compile the generated interface tables once.
- Ensure client-side and server-side FFI use the same Rust type identities.

## Boundaries

This crate contains protocol metadata, not a Wayland client or server. It does
not connect sockets, dispatch objects, validate requests, or manage compositor
state. Core `wl_*_interface` symbols continue to come from the libwayland
library linked by each consumer.

## Runtime Effect

The generated tables are immutable link-time data. They let `ass-backend` and
`ass-server` marshal core shell and extension messages without duplicating
symbols or incompatible FFI definitions.

## Use

This is an internal FFI support crate. Add it as a dependency only when a
backend or protocol implementation must reference the shared Wayland ABI or
generated extension tables. It has no standalone runtime entry point. Building
it requires `pkg-config`, `wayland-protocols`, and `wayland-scanner`.

## Related Documentation

- [Wayland server decision](../../docs/adr/0002-hand-rolled-wayland-server.md)
- [FFI soundness discipline](../../docs/adr/0006-ffi-soundness-discipline.md)
- [Workspace layout](../../docs/dev/project-layout.md)
