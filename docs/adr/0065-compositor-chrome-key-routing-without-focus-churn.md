# ADR-0065: Compositor chrome key routing without focus churn

- Status: Accepted
- Date: 2026-07-28

## Context

[ADR-0022](0022-application-launcher.md) introduced keyboard capture for
compositor-owned chrome. It routed keys to the launcher while open, but also
cleared the focused Wayland surface and restored it when the launcher closed.
Screenshot selection, overview, and other keyboard-capturing chrome later
reused the same mechanism.

Clearing focus sends `wl_keyboard.leave`. Because text-input focus follows
keyboard focus, it also sends `zwp_text_input_v3.leave`, invalidates the
client's committed text-input state, and deactivates the input method. Sending
`enter` to the same surface after a transient overlay closes cannot restore
that state; the client must rebuild and commit it. Clients differ in when they
do so, which caused preedit and commit delivery to stop in applications such
as foot after using compositor chrome.

The old mechanism also treated capture as an overlay-level boolean. If chrome
closed after receiving a key press, the matching release could reach the
client. If chrome opened between a client-owned press and release, it could
swallow the release. A focus leave hid some of this imbalance by resetting the
client's logical keyboard state, but it did not model key ownership.

## Decision

Wayland keyboard focus changes only when the seat's focused `wl_surface`
actually changes. Compositor-owned overlays have no client surface and
therefore do not clear or restore keyboard focus merely to intercept keys.
Text-input focus continues to follow real keyboard focus through the existing
focus-change path.

Keyboard capture is a routing policy with per-key sequence ownership:

1. The capture state when a physical key is pressed selects either the focused
   client route or the compositor chrome route.
2. Repeated presses and the matching release retain that owner even if chrome
   opens or closes in the meantime.
3. Both routes advance the compositor's xkb state. Chrome-owned sequences are
   resolved for shell handling and withheld from the Wayland seat.
4. Real focus transitions still send the protocol leave/enter sequence. These
   include focusing another client surface, hiding or minimizing the focused
   surface, session locking, and surface destruction. Text input, data-device
   selection, and shortcut-inhibition focus update through one shared
   dependency hook.
5. A `wl_keyboard` resource's lifetime does not determine the seat's surface
   focus. Destroying one resource leaves focus unchanged. A newly bound
   resource for the focused client immediately receives `enter` followed by
   the current modifiers, as required by the core protocol.

This decision replaces only the keyboard-focus grab and restore behavior in
[ADR-0022](0022-application-launcher.md). Its launcher discovery, spawning,
chrome, and shortcut decisions remain in effect.

## Alternatives

- **Clear and restore focus for every compositor overlay.** Rejected because
  it invalidates `zwp_text_input_v3` state even though the focused surface did
  not change, makes transient chrome dependent on client-specific re-enable
  behavior, and requires retaining a raw surface pointer for restoration.
- **Preserve focus only for the screenshot selector.** Rejected because the
  same protocol and key-pairing problems apply to the launcher, overview,
  menus, and future compositor-owned chrome.
- **Replay the previous text-input state after a synthetic re-entry.**
  Rejected because the protocol makes state after `enter` client-owned; the
  compositor must not invent a replacement enable transaction.
- **Send all releases according to the current overlay state.** Rejected
  because a release without its press creates inconsistent client or chrome
  key state.

## Consequences

- Opening and closing compositor chrome no longer interrupts active preedit or
  commit delivery in the focused application.
- Screenshot selection, overview, launcher, menus, and future chrome share one
  capture model rather than component-specific focus exceptions.
- Key sequences remain balanced across overlay transitions.
- The saved raw keyboard-focus pointer and its grab/release API are removed.
- Focused-window minimization and late `wl_keyboard` binding now keep the
  keyboard, text-input, data-device, and shortcut-inhibition views aligned.
- Compositor chrome still receives xkb-resolved key input rather than
  text-input protocol events. A future chrome surface that needs full input
  method editing must acquire real keyboard focus or add a separate
  compositor-internal text-input integration.
