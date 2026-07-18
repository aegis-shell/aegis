# ass-server

`ass-server` is the hand-rolled Wayland server and window-management mechanism
for ass.

## Responsibilities

- Create the Wayland display socket and advertise supported globals.
- Own protocol objects, surface commits, buffers, seats, clipboard state, and
  extension lifecycles.
- Own Realm authority routing, private launch listeners, virtual outputs, and
  read-only observation filtering.
- Maintain window, focus, workspace, output, and interactive move/resize state.
- Route backend-neutral input to clients and apply compositor actions.
- Expose renderer- and shell-friendly snapshots built from `ass-core` models.

## Boundaries

The server does not create the host presentation target, issue GPU draw calls,
draw chrome, parse configuration files, or expose the external JSON protocol.
Those responsibilities belong to the backend, renderer, shell, configuration,
and IPC crates.

## Runtime Effect

Creating a `Server` allocates a libwayland display and an automatically named
Wayland socket. Dispatching it accepts clients and updates the surface tree;
window-management methods translate compositor intents into protocol state and
configure events. Activated Realm portals have no host pathname; dispatch
accepts their sandbox-only connections and assigns Realm identity before
registry enumeration.

Clipboard selections are scoped to one logical seat. Client-owned selections
forward transfers to the owning `wl_data_source`; compositor-owned selections
retain immutable MIME payloads and write them through a bounded background
lane so a paste target cannot block Wayland dispatch. The server intentionally
does not advertise X11-style Primary Selection; standard explicit copy and
paste remains available through `wl_data_device_manager`.

## Use

The executable creates one `Server`, publishes its socket name through
`WAYLAND_DISPLAY`, and repeatedly:

1. Dispatches pending client requests.
2. Forwards input from `ass-backend`.
3. Reads surface and window snapshots for `ass-render` and `ass-shell`.
4. Sends frame callbacks after presentation.

This crate is an integration mechanism, not a standalone server binary.

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [Wayland server decision](../../docs/adr/0002-hand-rolled-wayland-server.md)
- [Workspace layout](../../docs/dev/project-layout.md)
- [Realm and seat decision](../../docs/adr/0040-realms-seats-and-transferable-interaction-authority.md)
- [Explicit clipboard decision](../../docs/adr/0043-explicit-clipboard-only.md)
