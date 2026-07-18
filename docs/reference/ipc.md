# IPC Reference

The ass IPC is protocol version 2, carried as length-framed JSON over the
owner-only Unix socket at `$XDG_RUNTIME_DIR/ass.sock`. Every connection starts
with `Hello`; commands are accepted only after capability and scope checks.

## Capabilities

| Capability | Authority | Default |
|------------|-----------|---------|
| `query` | Read snapshots and subscribe to events or the journal. | Always granted. |
| `control` | Mutate windows, workspaces, layout, and notifications. | Server policy. |
| `input` | Inject bounded actions into a target window. | Named scope required. |
| `session` | Quit or perform other session-level operations. | Server policy. |

An absent `input` field is `false`, including messages from an older
version-2 peer. An unscoped connection never receives `input`, even when it
requests the capability and server policy otherwise permits it.

## Queries

| Request | Response | Capability |
|---------|----------|------------|
| `GetWindows` | `Windows` | `query` |
| `GetWorkspaces` | `Workspaces` | `query` |
| `GetNotifications` | `Notifications` | `query` |
| `GetOutputs` | `Outputs` | `query` |
| `GetJournal { since }` | `Journal` | `query` |
| `CaptureOutput` | `CaptureOutput` | `control` + explicit scope op |

`Subscribe` enables coarse events. `SubscribeJournal` enables one `Journal`
event per applied main-loop command.

## Commands

| Command | Capability | Operation class | Target |
|---------|------------|-----------------|--------|
| `Focus { id }` | `control` | `Focus` | Window |
| `Minimize { id }` | `control` | `Minimize` | Window |
| `Close { id }` | `control` | `Close` | Window |
| `Move { id }` | `control` | `Move` | Window |
| `SetWindowGeometry { id, rect }` | `control` | `SetWindowGeometry` | Window |
| `InjectInput { id, actions }` | `input` | `InjectInput` | Window |
| `Cycle { forward }` | `control` | `Cycle` | — |
| `SwitchWorkspace { dir }` | `control` | `SwitchWorkspace` | Focused output |
| `SwitchWorkspaceTo { id }` | `control` | `SwitchWorkspaceTo` | Workspace |
| `MoveToWorkspace { window, workspace }` | `control` | `MoveToWorkspace` | Window and workspace |
| `ToggleTiling` | `control` | `ToggleTiling` | Current workspace |
| `ToggleOverview` | `control` | `ToggleOverview` | — |
| `Notify { summary, body, app_id }` | `control` | `Notify` | — |
| `DismissNotification { id }` | `control` | `DismissNotification` | Notification |
| `Screenshot { path }` | `control` | `Screenshot` | Focused output |
| `Quit` | `session` | — | Session |

`Do` returns `Ok` after the command is queued, not after it is applied. Read
the next snapshot or journal entry to observe the result.

## Window Geometry

`SetWindowGeometry` uses compositor-global logical pixels. `rect.size.w` and
`rect.size.h` must each be between `1` and `32768`. The compositor:

- changes the window to floating layout;
- clears maximized and fullscreen state;
- clamps size to the client's minimum and maximum hints;
- preserves the requested origin; and
- exposes the resulting rectangle through `GetWindows`.

```json
{
  "type": "Do",
  "cmd": {
    "type": "SetWindowGeometry",
    "id": 7,
    "rect": {
      "origin": { "x": 120, "y": 80 },
      "size": { "w": 1280, "h": 720 }
    }
  }
}
```

## Synthetic Input

`InjectInput` requires a named scope that contains `InjectInput` and the target
window id. The operation must be listed explicitly: an omitted `ops` field does
not grant synthetic input. Coordinates are logical pixels relative to the
target window's top-left corner.

| Action | Fields | Effect |
|--------|--------|--------|
| `PointerMove` | `position` | Move the logical pointer. |
| `Click` | `position`, `button` | Move, press, and release. |
| `Scroll` | `position`, `dx`, `dy` | Move and deliver a smooth scroll. |
| `KeyPress` | `code` | Press and release one evdev key. |

Validation limits:

- `actions` contains 1–64 entries.
- Pointer positions must be inside the live, visible target and must hit that
  target rather than an overlapping window.
- Shell chrome must not cover any pointer position; keyboard-owning chrome
  rejects key input.
- Click buttons are Linux codes `0x110` through `0x117`.
- Key codes are at most `0x2ff`.
- Scroll deltas are finite and have an absolute value no greater than `1000`.
- Injected keys are refused while a physical modifier is held.
- All input is refused while a physical modifier, button grab, drag, or window
  move/resize is active.

```json
{
  "type": "Do",
  "cmd": {
    "type": "InjectInput",
    "id": 7,
    "actions": [
      {
        "type": "Click",
        "position": { "x": 40, "y": 32 },
        "button": 272
      }
    ]
  }
}
```

Input commands bypass compositor global key bindings. A live-state rejection
after queuing appears as `Effect::Refused` in the mutation journal.

## Capture

ass exposes pixel capture through two fail-closed operations that share one
same-frame presentation readback path (ADR-0037). Both are refused while the
session is locked or the seat is inactive. The request copies the exact frame
being submitted; later client commits, animations, or wallpaper frames cannot
change the detached snapshot. Captures include the overview grid while
overview mode is active.

`Command::Screenshot { path }` is a journaled `control` command that writes
the focused output as a PNG file; `ass-ctl screenshot` is its reference
frontend. `Request::CaptureOutput` is a synchronous query returning
`Response::CaptureOutput { width, height, png_base64 }` — the frame as a
base64-encoded PNG. The request requires the `control` capability and an
explicit `CaptureOutput` entry in the connection's scope `ops`; like
`InjectInput`, the operation is never inherited through the `None`-means-all
default. PNG payloads are bounded by the codec's 16 MiB frame limit.

Capture regions use compositor logical pixels. The returned PNG and
`width`/`height` use physical output pixels, so a region captured at 200%
scale has twice the logical width and height.

Continuous frame streaming (screencast, `xdg-desktop-portal`) is future
work and can reuse the same scale-aware frame-copy path.

## Named Scopes

Named scopes are configured with `[[agent.scope]]`. Every command resolves the
named scope again, so a configuration reload can narrow or revoke authority
without reconnecting. An explicit unknown or removed scope fails closed.

See the [Configuration Reference](config.md#agent-scopes) for fields and
operation names.
