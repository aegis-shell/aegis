# Command Panel

The command panel is the display-and-control surface for desktop-computer
behavior
([ADR-0115](../../adr/0115-command-panel-desktop-behavior-scope.md)): the
domains daily desktop use involves — sound, displays, network and
Bluetooth, power, the session, notifications, the tray, the user's persona,
machine resources, and desktop preferences. The scope test is the user's
computer, not the compositor: a surface belongs when its subject exists on
any desktop computer regardless of the compositor implementation.
Compositor-mechanism surfaces — window-tree internals, protocol or IPC
state, introspection, developer tooling — stay out and remain CLI/MCP
territory. Domains follow the user's model rather than the implementing
component, so window tiling and Agent Workspaces are in scope while
Interaction Domain lifecycle management is not. Every proposed tab,
section, or status row justifies itself against this scope.

Visually the panel is organized according to a **9-grid spatial anchor
architecture**:

- **Top-Left (左上): Frameless user identity block** (ringed avatar + name + hostname).
- **Top-Right (右上): Floating notification stream.**
- **Right-Middle (右中): Network monitor** (interface, charts, rates).
- **Center (中): Split Main Control Panel (solid nav rail + elevated content view).**
- **Other Anchor Zones**: Peripheral zones remain open for frictionless
  focus. Machine resource telemetry is currently disabled.

## Solid Scheme-Adaptive Background

The whole output uses one opaque grouped background:

- Dark mode uses a near-black canvas with opaque elevated-gray panels.
- Light mode uses a pale grouped-gray canvas with opaque white panels.
- System blue marks active tabs, slider fills, and semantic focus. Quiet
  separators define inactive surfaces without decorative edge lighting.
- The reveal animates the painted canvas's opacity. The command panel does
  not declare backdrop capture, blur, frost, or analytic Liquid Glass.

## 9-Grid Anchor Spatial Geometry

- **Top-Left (Identity Block)**: Frameless user identity (300×84 px) anchored
  at `(margin_x, margin_y)` with a 56 px avatar orb (live 3D VRM rendering
  or fallback initials) inside a **gapped line ring**, the display name at
  title scale, `@username · groups`, and the **hostname** on a third line.
- **Top-Right (Notifications Stream)**: Floating notification stream (260×200 px)
  aligned to the right column's left edge, displaying chronological
  notification rows with click-to-dismiss.
- **Right-Middle (Network Monitor)**: Network surface (300×300 px) below the
  notification stream, sharing the column's right edge.
- **Center (Main Split Control Panel)**: Centered interaction hub (720×460 px at
  full size, narrowed to clear the network column) horizontally split into
  a Left Navigation Rail and a Right Content View.
- Clicking anywhere outside the active interactive controls dismisses the HUD.

## Identity Block (Top-Left)

- The block is **frameless**: no chip body, painted background, or border;
  content sits directly on the grouped canvas.
- A 56 px avatar orb floats with a blue **line ring separated from the avatar
  edge by a deliberate 3 px gap**, reading as a stroke of the identity mark
  rather than a container border. Every avatar texture is circle-masked in
  its alpha channel — still photos, live-rendered 3D VRM models with VRMA
  animation playback, and the initials fallback all render cleanly. The orb
  follows the canonical persona contract
  (`$XDG_DATA_HOME/aegis/avatars/avatar.vrm`).
- Beside the orb, the display name sets at title scale in the display
  typography; below it `@username · groups`, then the machine's **hostname**,
  so the block reads as who-on-which-machine.

## Notifications Stream (Top-Right)

- The top-right notifications panel presents the active notification queue in
  a frameless scroll view.
- Notifications list newest first as recessed solid cards (summary plus
  body). Clicking a row dismisses it immediately.

## Network Monitor (Right-Middle)

- The identity header names the **live interface** (`wlan0`, `enp3s0`) and,
  when the link is wireless, the **Wi-Fi network name**: `wlan0 · Homelab-5G`.
  Wired links show the interface alone; no link shows a muted "Offline".
  The interface comes from `/sys/class/net`; the SSID from the forked
  `iwgetid -r` probe carried on the status poller.
- Below it, **two framed line charts** — upload (top-left) and download
  (top-right) — plot the shared throughput histories from
  `ChromeUpdate::ResourceStats` (48-sample rings, normalized to the series'
  own peak, newest sample at the right).
- Under each chart, the **current rate as text**: `Up 1.2M / s`,
  `Down 340K / s`.

## Main Split Control Panel (Screen Center)

The main control panel sits in the center of the display as the primary interaction hub,
split into two functional columns:

### 1. Left Column: Individual Solid Semicircular Capsules

- Vertically stacked 100% semicircular ($r = h/2 = 22\text{px}$) capsule
  pills have no outer navigation container.
- Each tab features **Icon + Text** layout with horizontal padding that
  prevents subpixel viewport clipping.
- The selected tab uses an opaque blue-tinted surface, primary text, and a
  system-blue icon.
- Inactive tabs use the elevated surface, primary text, muted icons, and a
  quiet separator border. Hover uses a low-strength blue tint.

### 2. Right Column: Elevated Content View

- An opaque elevated page surface hosts the active tab content.
- Header row renders the active section icon and headline.
- Scrollable body renders the **Quick Controls** tab or modular settings
  pages (`aegis-settings` registry).

## Tabs

- **Quick Controls (first tab)** — the daily toggles users reach for without
  thinking: **volume** (+ mute), **brightness**, **always-on** (session idle
  inhibitor), and **do-not-disturb**, emitted as `SystemAction`s. The
  remaining quick settings (Wi-Fi/Bluetooth radios, tiling, Agent
  Workspaces status, Lock Now) are held out of the panel for now per the
  scope review; they return through the settings module tabs.
- **Settings module tabs** — one tab per available `aegis-settings` module
  (display, input, appearance, power), rendered from the registry inside
  the main panel; their `SettingsAction`s leave through
  `ChromeEvents::settings_actions` with the current snapshot revision.

## Display Typography

All panel text uses the **bold sans-serif display voice**: label widgets
render through a panel-owned lens skin that stamps weight 700 (the theme's
bold range) on every label while the panel renders, and sizes step up from
the shared type scale — nav and section headers at `title`/`headline`,
control rows at `body`, secondary lines at `label`/`footnote`. The engine's
default family is already sans-serif; no serif voice enters the panel.

## Rendering Budget

The command panel paints through Lens only. Its `Chrome` implementation uses
the default zero blur and empty backdrop-effect declarations even while open.
This keeps the modal presentation to the normal chrome composition pass and
avoids paying for both a full-screen blur and per-surface Liquid Glass.

## Motion

The cluster reveals with clean HUD motion:
- The top-left identity block slides in from the left.
- The top-right notifications panel slides in from the right.
- The network monitor slides in from the right, staggered behind the main panel.
- The center main control panel rises upward into place.
- The solid background canvas's opacity rides the same reveal.
- Reduced motion resolves directly to the end state.

## Material Rules

- Background, panel, and recessed-surface roles are opaque in both schemes.
- Borders are separators, not simulated material rims. The identity block's
  gapped ring remains content rather than a container edge.
- Interactive emphasis uses the semantic system-blue accent. Destructive
  actions use the design system's scheme-specific critical color.
- New command-panel surfaces extend the solid semantic hierarchy. They do
  not add a backdrop effect unless the complete presentation policy changes.
