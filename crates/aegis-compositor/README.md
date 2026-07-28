# aegis-compositor

`aegis-compositor` is the hand-rolled Wayland server and window-management mechanism
for ass.

## Responsibilities

- Create the Wayland display socket and advertise supported globals.
- Own protocol objects, surface commits, buffers, seats, clipboard state, and
  extension lifecycles.
- Own Realm authority routing, private launch listeners, virtual outputs, and
  read-only observation filtering.
- Maintain window, focus, workspace, output, and interactive move/resize state.
- Route backend-neutral input to clients and apply compositor actions.
- Expose renderer- and shell-friendly snapshots built from `aegis-core` models.

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

Decoration-aware toplevels use compositor-owned, borderless frames by default:
clients omit their own title bars while move, resize, close, and state controls
remain available through compositor input and shell surfaces. The runtime may
switch to client-side decorations through the shared decoration policy, and
existing clients receive the required configure sequence.

Clipboard selections are scoped to one logical seat. Client-owned selections
forward transfers to the owning `wl_data_source`; compositor-owned selections
retain immutable MIME payloads and write them through a bounded background
lane so a paste target cannot block Wayland dispatch. The server intentionally
does not advertise X11-style Primary Selection; standard explicit copy and
paste remains available through `wl_data_device_manager`.

Native editors publish text context through `zwp_text_input_v3`. On the
physical seat, one host input method may consume that context through
`zwp_input_method_v2`, grab the hardware keyboard, forward unhandled keys
through its paired `zwp_virtual_keyboard_v1`, and present candidate surfaces
as compositor-positioned overlays. Realm registries do not expose the
input-method or virtual-keyboard manager globals. Destroyed keyboard resources
are removed from forwarding without dereferencing their lifecycle tombstones.

The server deliberately does not advertise `wp_presentation` until the backend
can provide commit-correlated presentation timestamps. Clients use
`wl_surface.frame` callbacks instead; the runtime completes those callbacks
after output presentation.

## Use

The executable creates one `Server`, publishes its socket name through
`WAYLAND_DISPLAY`, and repeatedly:

1. Dispatches pending client requests.
2. Forwards input from `aegis-backend`.
3. Reads surface and window snapshots for `aegis-render` and `aegis-shell`.
   Client rendering pairs `client_surface_frames` and
   `client_surface_dmabuf_frames` with the authoritative
   `client_surface_frame_order` so each toplevel, popup, and subsurface tree
   stays in desktop z-order.
4. Sends frame callbacks after presentation.

This crate is an integration mechanism, not a standalone server binary.

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [Wayland server decision](../../docs/adr/0002-hand-rolled-wayland-server.md)
- [Wayland input-method decision](../../docs/adr/0062-wayland-input-method-v2-host-integration.md)
- [Client surface compositing decision](../../docs/adr/0061-window-tree-atomic-client-surface-compositing.md)
- [Workspace layout](../../docs/dev/project-layout.md)
- [Realm and seat decision](../../docs/adr/0040-realms-seats-and-transferable-interaction-authority.md)
- [Explicit clipboard decision](../../docs/adr/0043-explicit-clipboard-only.md)
