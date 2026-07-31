# ADR-0084: Session-scoped always-on-top window band

- Status: Accepted
- Date: 2026-07-31

## Context

Users routinely need one window — a video call, a reference document, a
terminal tailing a build — to stay visible while they work in other
windows. The `xdg_toplevel` protocol has no always-on-top state, so clients
cannot request it; the compositor owns the stacking order and the chrome
overlays that render above client surfaces. Adding the feature requires
deciding where such windows sit in the stacking order, who may set the
flag, and whether the setting persists.

Compositor chrome must stay reachable and visually above every client
window, so a pin cannot float a window above the dock, launcher, or command
panel. A pin is also a transient working intent: restoring it across
sessions would resurrect a stacking exception for a window the user no
longer remembers pinning.

## Decision

1. **Always-on-top is a compositor-internal, session-scoped flag.** Each
   toplevel carries an `always_on_top` flag that is not persisted, has no
   configuration key or `window_rule` action, and is not exposed as an
   `xdg_toplevel` state. It clears when the session ends.

2. **Flagged windows form a band at the top of the surface stacking order,
   below compositor chrome.** After every raise and each input dispatch
   batch the compositor restacks so that no normal window can stack above
   a flagged one, keeping each flagged window's surface tree contiguous
   and preserving relative order inside and outside the band. Chrome
   overlays continue to render above the band.

3. **The Dock context menu exposes the toggle.** A lifecycle row next to
   Maximize/Restore flips its label between "Always on Top" and
   "Not Always on Top" and targets the same window as the Maximize row:
   the application's activated window, excluding read-only mirrors and
   minimized or fullscreen windows.

4. **External control uses one new IPC command.** `SetAlwaysOnTop
   { id, on_top }` sits in the `control` capability under the
   `SetWindowGeometry` operation class, and `aegis-ctl always-on-top
   <id> <on|off>` wraps it. There is no default keybind.

## Alternatives

- **Persist the flag** through the configuration or a `window_rule`
  action. A pin expresses a transient intent; restoring it on the next
  session would surprise users with a stacking exception they no longer
  remember setting. A `window_rule` action can still be added later
  without reversing this decision.
- **Extend the Wayland protocol so clients can request topmost.** No
  standard `xdg_toplevel` state exists, and a private request interface
  would let applications compete for a stacking policy that belongs to
  the user.
- **Stack flagged windows above compositor chrome.** Chrome overlays are
  the user's control surfaces; letting a client window cover the dock or
  command panel breaks their reachability.

## Consequences

- The IPC seam gains a durable command: renaming or removing
  `SetAlwaysOnTop` would break `aegis-ctl` and other scoped clients, and
  its authorization reuses the `SetWindowGeometry` operation class rather
  than adding a new one.
- Every raise and dispatch batch pays a restack pass, which returns early
  when no flagged window exists.
- There is no configuration, keybind, or persistence surface to maintain;
  the feature ships as the flag, the Dock row, and the IPC command.
- The state is observable to IPC clients only through the window snapshot;
  scripting reads it back with `aegis-ctl windows`.
