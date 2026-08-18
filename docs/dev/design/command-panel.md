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

Visually the panel is organized according to a **9-grid spatial anchor architecture**:
- **Top-Left (左上)**: Compact user profile chip (48px avatar + name).
- **Top-Right (右上)**: Floating notification stream.
- **Center (中)**: Split Main Control Panel (Left Liquid Glass Nav Rail + Right Gaussian Blur Content View).
- **Other Anchor Zones**: Peripheral zones remain open for frictionless focus. Machine resource telemetry is currently disabled.

## 9-Grid Anchor Spatial Geometry

- **Top-Left (Profile Chip)**: Compact user identity badge (280×72 px) anchored at `(margin_x, margin_y)`
  with a 48 px avatar orb (live 3D VRM rendering or fallback initials) and `@username` tagline.
- **Top-Right (Notifications Stream)**: Floating notification stream (260×200 px) anchored at
  `(display.w - margin_x - 260, margin_y)` displaying chronological notification rows with click-to-dismiss.
- **Center (Main Split Control Panel)**: Centered interaction hub (720×460 px at full size)
  horizontally split into a Left Navigation Rail and a Right Content View.
- Clicking anywhere outside the active interactive controls dismisses the HUD.

## Profile Chip (Top-Left)

- A compact 48 px avatar orb floats in the top-left corner.
  Every avatar texture is circle-masked in its alpha channel — still photos,
  live-rendered 3D VRM models with VRMA animation playback, and the
  initials fallback all render cleanly. The orb follows the canonical
  persona contract (`$XDG_DATA_HOME/aegis/avatars/avatar.vrm`).
- Beside the orb, the display name sets in the headline role; a
  secondary footnote line below reads `@username · group, group` in muted text.

## Notifications Stream (Top-Right)

- The top-right notifications panel presents the active notification queue in
  a frameless scroll view.
- Notifications list newest first as pure floating text rows (summary plus body).
  Clicking a row dismisses it immediately.

## Main Split Control Panel (Screen Center)

The main control panel sits in the center of the display as the primary interaction hub,
split into two functional columns:

### 1. Left Column: Individual Floating Semicircular Liquid Glass Capsules
- Vertically stacked 100% semicircular ($r = h/2 = 22\text{px}$) **Liquid Glass** capsule pills with zero outer container boundaries and zero 2D stroke outlines.
- Each tab features **Icon + Text** layout with horizontal padding preventing any subpixel viewport clipping.
- Selected tab receives an intense **optical spotlight caustic focus beam** (`LiquidGlassFocus` with `strength: 1.25`), crystal clear illuminated refraction core (`frost_strength: 0.55`, `tint_strength: 1.35`), and brilliant white illuminated text.
- Inactive tabs float in pure optical refraction without artificial 2D borders.

### 2. Right Column: Gaussian Blur Content View
- A frosted **Glass Page View** (`GlassRole::ProminentPanel`, radius: 20px) hosting the active tab content.
- Header row renders the active section icon and headline.
- Scrollable body renders System quick settings (Sound, Brightness, Connectivity, Desktop) or modular settings pages (`aegis-settings` registry).

## Liquid Glass Physical Pipeline

The command panel connects directly to the compositor's analytic **Liquid Glass** pipeline (`prism` / `flux`), emitting `LiquidGlassRegion` descriptors with unified `GlassRole` tokens:
- **Profile Chip (Top-Left)**: `GlassRole::Chip` — physical refraction, edge highlights, and adaptive backdrop scattering aligned with the HUD chips.
- **Notifications Stream (Top-Right)**: `GlassRole::FloatingPanel` — physical glass body covering the notification stream.
- **Left Navigation Capsules**: `GlassRole::Chip` — each capsule pill emits its own discrete `LiquidGlassRegion` ($r = 22\text{px}$) with targeted `capture_bounds`. Active pill attaches `LiquidGlassFocus` for spotlight beam projection.
- **Right Content View**: `GlassRole::ProminentPanel` with `materials::glass_panel` minimal foreground over the physical liquid-glass body.

## Motion

The cluster reveals with clean HUD motion:
- The top-left profile panel slides in from the left.
- The top-right notifications panel slides in from the right.
- The center main control panel rises upward into place.
- Reduced motion resolves directly to the end state.

## Material Rules

- Physical refraction, edge lighting, and chromatic dispersion are provided by `prism` via `LiquidGlassRegion`.
- Painted foregrounds stay deliberately minimal with **zero painted strokes/borders** to preserve raw optical refraction.
- Background scrim quad is removed completely to guarantee seamless, boundless presentation and eliminate stencil invalidation flash on cursor movement.
