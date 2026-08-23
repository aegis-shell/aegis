# ADR-0138: The Input settings domain

- Status: Accepted
- Date: 2026-08-22

## Context

The settings IPC (ADR-0049, ADR-0056) exposed one hardware-input domain:
touchpad. `SettingsSnapshot.touchpad` carried a `TouchpadStatus`, and
`SettingsAction::SetTouchpad` persisted `[input.touchpad]`. Mouse and
keyboard settings existed only as registered-but-unavailable module routes;
the direct DRM backend dropped every non-touchpad libinput device at hotplug
(`add_input_device` returned early), so no per-device mouse configuration
could be applied even in principle. Keyboard repeat was hardcoded: the
`wl_keyboard` bind advertised 25 cps / 250 ms (ADR-0010), and the
input-method keyboard grab advertised a divergent 25 cps / 600 ms pair.

Users asked for the obvious desktop settings: keyboard repeat speed, mouse
scroll and move speed, and touchpad scroll speed. Adding them as three more
single-device domains would multiply routes, snapshot fields, actions, and
journal entries for what is physically one seat's input policy.

## Decision

One `input` settings domain owns the whole seat's device policy.

1. **Module topology.** The Touchpad module becomes the **Input** module
   (route `input`). The placeholder `mouse` and `keyboard` routes are
   removed; keyboard layout, compose keys, and shortcut editing remain
   unexposed rather than behind dead routes.
2. **Model.** `aegis_model::input::InputConfig` groups `KeyboardConfig`,
   `MouseConfig`, and the existing `TouchpadConfig`. `InputStatus` groups
   the touchpad and mouse device statuses plus the keyboard profile.
3. **Wire vocabulary** (IPC protocol version 31): `SettingsSnapshot.touchpad`
   becomes `SettingsSnapshot.input`, and `SettingsAction::SetTouchpad`
   becomes `SettingsAction::SetInput { config: InputConfig }`. The rename is
   a deliberate break; older settings clients fail deserialization rather
   than silently editing a partial profile.
4. **Persistence.** `ConfigEdit::SetTouchpad` becomes
   `ConfigEdit::SetInput`, writing `[input.keyboard]`, `[input.mouse]`, and
   `[input.touchpad]` in one edit.
5. **Backend.** `Host::set_touchpad_config`/`touchpad_status` become
   `set_input_config`/`input_status`. The DRM backend retains plain mice
   (pointer devices that are neither touchpads nor tablet tools) and applies
   libinput acceleration and natural scrolling to them.
6. **Keyboard repeat** (`repeat_rate`, `repeat_delay_ms`) is advertised as
   `wl_keyboard.repeat_info` from the config, both at bind and — since the
   protocol permits it at any time — pushed to already-bound version-4+
   keyboards when the settings change. The input-method grab advertises the
   same policy, removing the 250 ms / 600 ms divergence. The compositor
   still does not repeat keys itself (ADR-0010).
7. **Scroll speed.** Mouse and touchpad `scroll_speed` multipliers are
   applied by the compositor when translating libinput scroll into
   `wl_pointer` axis frames — libinput has no per-device equivalent. Wheel
   motion scales `value`/`value120` and whole detents; a deliberate click
   slowed below one detent still steps once so legacy v5–7 clients keep a
   usable wheel, while high-resolution fractions never inflate to whole
   steps.

## Alternatives

- **Keep `touchpad` and add `mouse`/`keyboard` domains.** Three routes,
  three snapshot fields, three actions, and three journal payload shapes for
  one device seat; coalescing and revision checks would also run three times
  for what is conceptually one edit. Rejected as interface churn without a
  user-visible benefit.
- **Per-device profiles keyed by device name** (as some compositors do).
  The existing single-profile-per-class model is simpler and survives
  hotplug and renaming; per-device profiles can be added later behind the
  same `InputConfig` shape.
- **Server-side key repetition.** Rejected: ADR-0010's client-side repeat
  from `repeat_info` remains correct, and only the advertised values were
  missing configurability.

## Consequences

- Settings clients must adopt protocol version 31's `input` vocabulary; the
  `SetTouchpad` action and the `touchpad` snapshot field no longer exist.
- Existing user configs gain two new tables with defaults that reproduce the
  previous hardcoded behavior (`repeat_rate = 25`, `repeat_delay_ms = 250`,
  neutral mouse settings), so an unedited config behaves identically.
- The DRM backend now retains every pointer device, so its input-status
  probe cost grows with the mouse count as well; the probe remains
  throttled to its existing interval.
- Keyboard layout/compose editing is still unexposed; when it lands it
  extends `KeyboardConfig` and the Input module rather than reopening a
  route.
