# How to Run a Wayland Input Method

aegis supports native Wayland input methods on the physical seat. Applications
publish editor state through `zwp_text_input_manager_v3`; an input-method
daemon consumes that state through `zwp_input_method_manager_v2` and may use
`zwp_virtual_keyboard_manager_v1` for unhandled keys.

## Check Protocol Availability

Run the registry inspection inside the aegis session:

```bash
wayland-info | rg \
  'zwp_(text_input_manager_v3|input_method_manager_v2|virtual_keyboard_manager_v1)'
```

The output must contain all three interfaces. If the input-method or
virtual-keyboard manager is absent after updating aegis, restart the compositor.
Wayland globals belong to the running compositor process and do not appear
through live configuration reload.

## Start the Input Method

Start one input-method daemon after the compositor is ready. For Typio with a
Rime engine:

```bash
typio -v -E /path/to/typio-engine-rime
```

Typio's startup log should report that it bound
`zwp_input_method_manager_v2` and `zwp_virtual_keyboard_manager_v1`. Focus a
native Wayland text field and confirm that the log reports activation, a
keyboard keymap, and a popup anchor rectangle.

Only one input method can own a seat. A second daemon receives the protocol's
`unavailable` event. Stop the existing daemon before replacing it.

## Check Application Compatibility

Use a native Wayland application that implements
`zwp_text_input_manager_v3`. X11 applications are outside aegis's supported
scope, and a native application without text-input-v3 continues to receive
ordinary keyboard events but cannot provide caret or surrounding-text state
to the input method.

Interaction Domain applications may use text-input-v3, but Interaction Domain registries do not expose
the privileged input-method or virtual-keyboard manager globals. This prevents
a sandboxed client from claiming the host input-method role or injecting
keyboard events.

The protocol and authority decisions are recorded in
[ADR-0062](../adr/0062-wayland-input-method-v2-host-integration.md).
