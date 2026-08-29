# Platform Adaptation

Status: **Partial**.

Aegis is a Linux Wayland desktop and compositor. Linux desktop behavior is
the only shipped native platform baseline. Windows, macOS, and mobile
conventions are comparison inputs for portable applications, not promises
that the Aegis shell runs on those platforms.

## Platform baseline

| Concern | Aegis contract |
|--------|----------------|
| Windowing | Wayland compositor authority, XWayland compatibility where supported |
| Accessibility | Linux AT-SPI integration with compositor-owned semantic boundaries |
| Applications | XDG desktop entries, icon themes, portals, and freedesktop settings |
| Keyboard | Configurable compositor actions expressed through semantic bindings |
| Pointer | Primary, secondary, middle, scroll, drag, and standard cursor roles |
| Touchpad | Gesture bindings map to compositor actions and respect reduced motion |
| Scaling | Per-output logical geometry with explicit physical-pixel rendering |

## Convention adaptation

Portable first-party applications preserve the task and information
architecture while adapting control conventions to their host. They do not
copy another platform's visual chrome into the Aegis shell.

| Host context | Adaptation rule |
|-------------|-----------------|
| Linux desktop | Follow XDG, Wayland, AT-SPI, and the active toolkit conventions. |
| Windows client | Use Windows shortcut, window-control, and accessibility conventions when a client exists. |
| macOS client | Use macOS menu, Command-key, window-control, and accessibility conventions when a client exists. |
| Mobile client | Redesign for touch targets, safe areas, software keyboard, and navigation history; do not shrink desktop panels. |

These non-Linux rows are design gates for future clients. They have no shared
implementation status today.

## Input modality rules

- Expose semantic actions independently of their pointer, key, gesture, or
  assistive trigger.
- Show hover feedback only on devices that provide hover; required actions
  remain discoverable without it.
- Keep direct manipulation attached to the pointer or touch contact and offer
  a command alternative for consequential operations.
- Treat touchpad gestures as shortcuts, not the only route to a feature.
- Recompute target size and spacing for a touch-first client instead of
  assuming desktop density.

## Output and system integration

Components use usable output bounds and resolved output scale. They do not
assume one primary monitor, fixed DPI, a global menu bar, or a taskbar edge.
System integration uses portals and compositor services rather than importing
foreign platform behavior into the visual layer.

## Adoption work

- Document keyboard and pointer parity for each adopted component.
- Add touch and stylus contracts only when a supported input path exists.
- Define portable application adaptation separately from compositor chrome
  before any non-Linux target is declared supported.

See [Interaction Paradigms](../patterns/interaction-paradigms.md) and the
[Configuration Reference](../../../reference/config.md).
