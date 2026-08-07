# Command Panel

The command panel is a three-surface cluster — a full-width header band,
a main panel, and a side column — presented as one centered group above
the dark blurred scrim. All three surfaces use the HUD material: the
dark glass [HUD panel](surfaces.md) with the cyan accent, thin
hairlines, and corner-bracket accents — the VR/AR personal-info-HUD
language. Seams between surfaces are deliberate.

## Cluster Geometry

- The header band spans the full cluster width at 118 logical px. Below
  it, the main panel (640×420 logical px at full size) sits at the left
  and the 300 px side column at the right, separated by a 12 px gap.
- The cluster centers on the output. On displays too small to host it,
  the side column shrinks to its minimum first, then the main panel;
  past those minima both yield down to their floors, so the cluster
  always fits on-screen.
- The scrim runs slightly deeper than the default blur so the dark
  glass surfaces still separate from the desktop; it remains
  click-to-dismiss.

## Header Band

The header band carries two zones separated by a vertical hairline:
persona on the left, machine state on the right.

### Persona Zone

- A 72 px avatar orb sits in the persona ring. Every avatar texture is
  circle-masked in its alpha channel — still photos, animated VRM
  models, and the gradient orb fallback all render as discs. The orb
  follows the same user-configured avatar sources as the lock screen.
- Beside the orb, the display name sets in the primary text role; a
  secondary line below reads `@username · group, group` in muted text.

### Machine Zone

- A thin-line vector chassis pictogram — laptop or desktop — opens the
  zone. The pictogram reflects the detected chassis, with battery
  presence as the fallback signal.
- Beside it, live gauge rows report machine state: CPU, GPU, RAM,
  network, disk, and battery, capped at five rows.

### Gauge Grammar

- A gauge row is a label cell, a bar or sparkline zone, and a
  right-aligned value cell. CPU, GPU, and RAM lead with a 48-sample
  history sparkline alongside the current percent.
- Rows are honest: a row appears only when backed by real data. No GPU
  busy percent, no GPU row; no battery, no battery row. A charging
  battery tints its row with the accent. Gauges sample on a two-second
  cadence — live, not animated.

## Main Panel

The main panel is a flat tab bar over the active tab's body.

- The tab bar holds the **System** tab plus one tab per available
  settings module, in registry order, with the close button at its
  right end. The active tab gets the accent label and an underline;
  hovered tabs take the soft accent wash. Modules without a working
  backend render no tab.
- The close button duplicates the scrim click and `Escape`, never
  replaces them.
- The **System** tab groups quick settings under muted group headers
  (Sound, Brightness, Connectivity, Desktop, Agent Workspaces, Session)
  inside a scroll view, ending in the display-only Agent Workspaces
  status row.
- Each settings tab renders the module's page from the `aegis-settings`
  registry in place; explicit-apply modules stage edits behind their
  Apply button.
- Tab bodies scroll when they overflow.

## Side Column

The side column is always visible, regardless of the active tab: the
notifications panel fills the flexible height on top, and the tray
panel pins to the bottom at a fixed height. Each carries a small muted
section header.

- **Notifications** lists every retained notification as a card —
  summary plus body — in a scroll view. Clicking a card dismisses it.
  The list has no row cap.
- **Tray** lays StatusNotifierItem icons in a grid whose column count
  derives from the panel width, never a hard-coded count. Left-click
  activates an item; right-click opens the host-rendered dbusmenu
  popover anchored to the live cell rect. Switching tabs closes an open
  popover even though the tray stays visible.

## Motion

The cluster reveals with a stagger: the header band leads, the main
panel lags behind it, and the side column lags furthest, rising into
place. The corner brackets fade with each surface's reveal. Reduced
motion resolves directly to the end state.

## Material Rules

- Every surface in the cluster uses the HUD panel material; no surface
  introduces a new material, tint, or border treatment.
- The cyan accent belongs to active tabs, underlines, corner brackets,
  and slider or gauge fills. It does not tint body text.
