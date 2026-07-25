# ADR-0054: Interactive target picking and window-scoped stream targets

- Status: Proposed
- Date: 2026-07-25

## Context

Portals such as `Screenshot` (with `interactive=true`) and `ScreenCast` (with `source_types` containing `window` or `monitor`) require user consent and interactive target selection via compositor chrome.

## Decision

### 1. IPC `PickTarget` Operation

Add `Request::PickTarget { kind: PickKind }` to `aegis-ipc`:
- `PickKind::Region`: Drag out a screen region.
- `PickKind::Pixel`: Pick a single pixel point (for `PickColor`).
- `PickKind::Window`: Pick a window, or select full output on Enter.

The compositor freezes the screen overlay, lets the user interact, and returns `Response::Picked { result: PickResult }`.

### 2. Window-Scoped Stream Targets

Extend `StreamOutputStart` to take `StreamTarget::Window { window: WindowId }`.
The compositor crops each presented frame to the target window's current visible region.

## Consequences

- Portal Screenshot (interactive & color picker) and ScreenCast (window mode) execute seamlessly through compositor chrome.
