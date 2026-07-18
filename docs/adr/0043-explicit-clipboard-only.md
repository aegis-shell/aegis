# ADR-0043: Explicit clipboard only; reject Primary Selection

- Status: Accepted
- Date: 2026-07-18

## Context

ass already exposes the standard Wayland clipboard through
`wl_data_device_manager`. Primary Selection adds a second, implicit channel:
merely selecting text publishes it, and middle-click conventionally pastes
it. The protocol exists to reproduce X11 behavior and remains an unstable,
optional Wayland extension. In ass's interaction model this is an
**anti-pattern**: it duplicates clipboard state, turns selection into a global
side effect, and makes accidental replacement and paste harder to reason
about. The protocol's own description frames it as matching X server
behavior and a middle-mouse convention in the
[wayland-protocols source](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/unstable/primary-selection/primary-selection-unstable-v1.xml).

Omitting the extension is compatible with normal modern application
operation. GTK locally emulates a primary clipboard on backends without a
native one ([GTK documentation](https://docs.gtk.org/gdk4/method.Display.get_primary_clipboard.html));
Qt reports Selection mode as unsupported when neither a primary selection
device nor data-control device exists
([Qt Wayland source](https://codebrowser.dev/qt6/qtbase/src/plugins/platforms/wayland/qwaylandclipboard.cpp.html));
and Chromium does not list the primary-selection manager among the Wayland
globals required to initialize a connection
([Chromium source](https://chromium.googlesource.com/chromium/src/+/ac0160a4ae8b8432a2041992415ceb1c1b0597e0/ui/ozone/platform/wayland/host/wayland_connection.cc)).
All three continue to use the standard clipboard.

This decision narrows the clipboard and Primary Selection provisions in
[ADR-0030](0030-xwayland-strategy.md) and
[ADR-0040](0040-realms-seats-and-transferable-interaction-authority.md). Their
other XWayland, Realm, seat, and interaction-authority decisions remain in
force.

## Decision

ass supports one explicit clipboard per seat through
`wl_data_device_manager`. It does not implement or advertise
`zwp_primary_selection_device_manager_v1`, and it does not preserve an empty
or no-op protocol placeholder.

The unused `ext-data-control-v1` placeholder is also removed. It was never
advertised, did not implement standard clipboard management, and its device
shape includes Primary Selection. Any future clipboard-manager protocol must
be proposed separately, implement real end-to-end clipboard semantics, and
preserve the explicit-clipboard-only policy.

Interactive screenshots may publish an immutable compositor-owned payload to
the physical seat's standard clipboard. IPC and agent Realm captures remain
side-effect-free. Clipboard contents and screenshot publication stay per-seat
so this decision does not weaken Realm isolation.

If XWayland is implemented, its `CLIPBOARD` selection is bridged to the
standard Wayland clipboard. X11 `PRIMARY` may remain internal to XWayland and
X11 clients, but ass does not bridge it into a Wayland Primary Selection.

## Alternatives

- **Advertise a no-op Primary Selection global.** Rejected because registry
  advertisement is a capability promise. Applications would take a protocol
  path that silently loses offers or transfers instead of detecting an
  unsupported capability.
- **Stop advertising the global but retain the implementation.** Rejected
  because dead protocol state and unsafe FFI remain maintenance and security
  surface, while making the product policy less explicit.
- **Keep a fully functional second selection channel.** Rejected because it
  preserves X11 compatibility at the cost of duplicate state and implicit
  user-visible side effects that conflict with ass's interaction model.
- **Remove clipboard support altogether.** Rejected because explicit
  copy/cut/paste is a core application requirement and is not coupled to
  Primary Selection.

## Consequences

- Standard copy, cut, paste, drag-and-drop, and compositor-owned screenshot
  clipboard publication continue through `wl_data_device_manager`.
- Wayland clients cannot publish selected text implicitly or request global
  middle-click paste. Toolkits either expose the capability as unsupported or
  keep any emulation local to their own process.
- The server no longer compiles or owns Primary Selection globals, resources,
  offers, sources, focus notifications, migration state, or no-op
  data-control objects.
- Compatibility testing must treat absence from the Wayland registry as the
  expected capability signal. Reintroducing the global is an architecture
  change and requires a superseding ADR.
