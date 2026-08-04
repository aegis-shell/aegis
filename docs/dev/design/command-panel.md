# Command Panel

The command panel is a three-surface cluster — a header band, an icon
rail, and a content panel — presented as one centered group above the
dark blurred scrim. All three surfaces are the SAO light island: the
frosted white [SAO panel](surfaces.md) material with the amber accent.
The cluster is one island per surface, not one island stretched across
three roles; seams between surfaces are deliberate.

## Cluster Geometry

- The cluster measures 716×550 logical px at full size and centers on
  the output. On displays too small to host it, the cluster shrinks
  rather than overflowing; the proportions hold.
- The scrim is unchanged: dark, blurred, click-to-dismiss. The cluster
  is the only light element in the presentation.

## Header Band

The header band spans the full cluster width at 118 logical px and
carries two zones separated by a vertical divider: persona on the
left, machine state on the right.

### Persona Zone

- A 72 px avatar orb sits in an amber ring. Every avatar texture is
  circle-masked in its alpha channel — still photos, animated VRM
  models, and the gradient orb fallback all render as discs. The orb
  follows the same user-configured avatar sources as the lock screen.
- Beside the orb, the display name sets in the primary text role; a
  secondary line below reads `@username · group, group` in muted text.

### Machine Zone

- A thin-line vector chassis pictogram — laptop or desktop — opens the
  zone, captioned in muted text. The pictogram reflects the detected
  chassis, with battery presence as the fallback signal.
- Below it, live gauge rows report machine state: CPU, GPU, RAM,
  network, disk, and battery.

### Gauge Grammar

- A gauge is a 4 px rounded track with an amber fill, a right-aligned
  value, and a 9.5 pt muted label. CPU leads with a 48-sample
  history-bar sparkline alongside the current percent.
- Rows are honest: a row appears only when backed by real data. No GPU
  busy percent, no GPU row; no battery, no battery row. A charging
  battery tints its row with the accent. Gauges sample on a two-second
  cadence — live, not animated.

## Icon Rail

The rail runs 64 px wide down the cluster's left edge and holds the
section buttons — System, Tray, Messages — plus a circular close
button pinned to its bottom.

- Section buttons are icon-only circles in the SAO ring idiom:
  unselected is an amber ring with an accent icon; selected is a solid
  amber disc with an on-accent icon; hover is a soft accent wash.
- The close button shares the ring grammar. It duplicates the scrim
  click and `Escape`, never replaces them.

## Content Panel

The content panel shows one section at a time. Its header carries only
the section title — no breadcrumbs, no controls.

- **System** groups quick settings under muted group headers (Sound,
  Brightness, Connectivity, Desktop, Agent Workspaces, Session) inside
  a scroll view. Bare separator lines do not appear; group headers do
  the dividing.
- **Tray** lays StatusNotifierItem icons in a scroll-view grid whose
  column count derives from the panel width, never a hard-coded count.
  Activation and the host-rendered dbusmenu popover are unchanged.
- **Messages** lists notifications as SAO quest-item cards — summary
  plus body — in a scroll view. Clicking a card dismisses it. The list
  has no row cap.

## Motion

The cluster reveals with a stagger: the header band slides in from the
left, the rail fades, and the content panel rises. Reduced motion
resolves directly to the end state.

## Material Rules

- Every surface in the cluster uses the SAO panel material; no surface
  introduces a new material, tint, or border treatment.
- The amber accent belongs to rings, fills, and selected discs. It
  does not tint text outside the gauge values' fill role.
