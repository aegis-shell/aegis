# ADR-0062: Wayland input-method-v2 host integration

- Status: Accepted
- Date: 2026-07-28

## Context

ass advertised `zwp_text_input_manager_v3` so native applications could
publish editor state. The nested backend could relay that state to a host
compositor, but a direct session had no compositor-side input-method role.
Daemons such as Typio require `zwp_input_method_manager_v2`, a hardware
keyboard grab, `zwp_virtual_keyboard_manager_v1`, and positioned popup
surfaces to provide CJK composition and candidate selection.

Input methods and virtual keyboards carry more authority than ordinary
application text input. A host input method can observe editor context and
intercept keyboard events, while a virtual keyboard can inject unhandled
events into the focused application. Realm clients must not gain either
authority through ambient registry discovery.

## Decision

ass implements the unstable Wayland input-method-v2 and virtual-keyboard-v1
protocols from their canonical XML definitions.

Application text-input-v3 state is routed to the active local input method.
When no local input method owns the seat, the existing backend relay remains
the fallback. One input-method object is available per seat; additional
objects receive `unavailable`.

The active input method may grab the physical keyboard after compositor-owned
shortcuts have been evaluated. A virtual keyboard object can affect a seat
only while its client owns that seat's active input-method object. Candidate
popup surfaces receive the input-popup role, are rendered above application
surfaces, and are positioned near the caret while remaining within the output.

The physical registry advertises the input-method and virtual-keyboard manager
globals. Realm registries hide both globals while retaining application-side
text-input-v3.

## Alternatives

- **Relay every session to an outer compositor.** Rejected because direct
  DRM/KMS sessions have no outer compositor and therefore no input method.
- **Run the input method as compositor-internal code.** Rejected because it
  couples engine lifecycle and failures to the compositor process and prevents
  ordinary protocol-compatible daemons from competing on implementation.
- **Expose virtual-keyboard authority to every Realm client.** Rejected
  because text-entry capability does not imply host keyboard-injection
  authority.
- **Implement only the manager globals.** Rejected because registry
  compatibility without activation, serial batching, grabs, and popup
  lifecycle does not provide a usable input method.

## Consequences

- Native Wayland input methods work in nested and direct sessions.
- Keyboard grabs, serial validation, virtual-keyboard forwarding, popup
  placement, and object destruction become compositor-owned protocol state.
- The local input method takes precedence over nested host relay while it is
  connected.
- Realm applications can request text input without claiming the privileged
  host input-method role.
- The protocols remain unstable upstream, so their vendored definitions and
  ABI tables must be reviewed when adopting a newer interface version.
