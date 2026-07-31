# ADR-0080: HUD status chips and the SAO command panel

- Status: Accepted
- Date: 2026-07-30

## Context

The status bar began as interactive chrome: a full-width 32 px top bar that
reserved its height from the tiling work-area (`Chrome::reserved`), captured
pointer input over its strip and drop-down panels, and hosted quick
settings, StatusNotifierItem (SNI) tray activation with dbusmenu popovers,
notification dismissal, and workspace-dot switching.

The product direction changed: the bar should read as a minimal FPS-style
HUD — pure status display that never competes with windows. Concretely, the
new requirements are: no reserved space (tiled and maximized windows run
underneath), no pointer capture (clicks fall through to windows, even on
tray icons), a fade-out when the cursor approaches, and a richer
Sword-Art-Online-style panel to absorb the displaced interactions.

Compositor chrome is in-process by design (ADR-0021): components render
through the shared lens `Ui`, receive the cursor position every frame, and
express space reservation and pointer capture through the `Chrome` trait, so
all of the above is expressible without protocol work. Touchpad swipe
gestures were previously only forwarded to clients holding
`zwp_pointer_gestures_v1` objects; the compositor itself had no gesture
actions.

## Decision

1. **The status bar becomes display-only HUD chips.** Three floating
   frosted chips replace the full-width bar: system status (network,
   Bluetooth, battery) plus the SNI tray row on the left, workspace dots in
   the center, and the clock, notification count, and Agent Workspaces state
   on the right. `Chrome::reserved` and `Chrome::captures_pointer` keep
   their defaults (none), so windows tile and maximize underneath and every
   click falls through. Each chip eases its visibility toward zero while
   the cursor is inside a proximity-inflated chip rect, honoring the
   reduced-motion policy; backdrop-blur regions track the visible chips,
   and the blur sigma drops to zero when nothing is visible. Fullscreen
   auto-hide is preserved. Raster tray/themed icons draw only above an
   alpha floor because lens cannot fade images; vector content fades
   smoothly.

2. **Interactions migrate to a new modal SAO command panel**
   (`aegis-sao-panel`, `SaoPanel`). One full-screen modal chrome surface
   hosts the displaced functionality: quick settings (volume, brightness,
   Wi-Fi, Bluetooth, do-not-disturb, tiled layout) emitted as
   `SystemAction`s, SNI tray activation with host-rendered dbusmenu
   popovers, and notification dismissal. Its visual language is the SAO
   menu idiom — frosted white floating panels with an amber accent — over
   the product's standard dark blurred scrim; the palette lives in
   `aegis-design` as `Sao` tokens with matching material and theme
   factories.

3. **The tray service is extracted to `aegis-tray`.** The composition root
   spawns the StatusNotifierWatcher/Host once and shares the handle: the
   HUD reads the snapshot; the panel reads it and owns the `TrayCommand`
   channel. No watcher logic is duplicated between components.

4. **The panel is toggled by a keybinding and a compositor gesture.** A new
   `Action::ToggleSaoPanel` keybinding (default `Super+S`, available while
   chrome captures the keyboard so it also closes the panel) and a
   compositor-level four-finger vertical touchpad swipe — down opens, up
   closes — claimed in the main loop and no longer forwarded to clients.

## Alternatives

- **Out-of-process layer-shell HUD clients.** The compositor deliberately
  ships no wlr-layer-shell implementation; shell chrome is in-process.
  Introducing a protocol surface for what the `Chrome` trait already
  expresses would add a process boundary, an IPC contract for cursor
  proximity, and new failure modes for zero behavioral gain.
- **Keep the bar interactive but auto-hiding.** An autohide bar still
  reserves space while visible and still eats clicks near the top edge; it
  cannot deliver the click-through overlay requirement.
- **Leave four-finger swipes for clients.** No client in the target
  sessions binds four-finger gestures, and a workspace-level panel summon
  is a compositor concern; the gesture is claimed at the same point where
  global keybindings dispatch.
- **Drop tray interactivity entirely.** Losing `Activate`/dbusmenu access
  would break real applications (media players, IM clients); the panel's
  Tray section preserves the full interaction set instead.

## Consequences

- Tiled and maximized windows gain the former 32 px top reservation; the
  toast stack still anchors below `HUD_HEIGHT`, which remains the chip
  height.
- Pointer interactions formerly on the bar — workspace-dot switching,
  volume scroll-step, bell/network toggles, tray clicks — no longer exist
  there. Equivalents live in the SAO panel, existing keybindings, the
  overview, and IPC (`aegis-ctl`).
- Four-finger touchpad swipes are compositor-owned and never reach
  `zwp_pointer_gestures_v1` clients; three-finger swipes still forward.
- New crates `aegis-tray` and `aegis-sao-panel`; `aegis-statusbar` shrinks
  to presentation. New default binding `Super+S` (`sao` action name) is
  documented in the configuration reference.
- The HUD renders no text on behalf of pointer hover; all animated state is
  driven by cursor position and system snapshots, keeping the component
  free of `ChromeEvents` emissions.
