# Comparative Survey of Compositors and Shells

aegis targets the feature richness of a full graphical shell while keeping the
compositor small and the chrome pluggable. This page surveys the systems aegis
borrows from, isolates the ideas worth carrying over, and names the ones aegis
deliberately leaves behind. It grounds the product direction in
[Vision and Scope](vision.md) and the concrete milestones in
[Roadmap](roadmap.md); specific decisions are recorded in the
[Architecture Decision Records](../adr/index.md).

The survey is selective, not exhaustive. It covers the seven systems the
project takes inspiration from and groups findings by the concerns a
compositor must own: architecture, layout, workspaces, configuration,
extension and automation, chrome, and input/output.

## Systems at a Glance

| System | Lineage | Layout | Config | Extensibility | Notable idea |
|--------|---------|--------|--------|---------------|--------------|
| GNOME Shell | GTK / Mutter (C + JS) | Floating + manual snap; dynamic workspaces | `gsettings` schemas, `dconf`; ad hoc file | JavaScript extensions on a live shell | The Activities overview unifies search, windows, workspaces, and app launch |
| KDE Plasma | Qt / QML / KWin (C++) | Floating; KWin tiling scripts; Activities | `~/.config` (INI-like), `kwriteconfig5` | Plasmoids (QML/C++), KRunner plugins, KWin scripts | Data engine / visualization split; KRunner as a universal command surface |
| sway | wlroots (C), i3 lineage | Manual tree tiling | Plain-text file in i3 syntax | i3 IPC; external bars and helpers | The compositor ships no chrome; bars, lock, wallpaper, launcher are separate programs |
| river | wlroots (Zig), bspwm/dwm lineage | Dynamic tiling, external layout generator | None; runtime IPC only | `riverctl` IPC; layout generator protocol | Layout policy is an out-of-process program; config is a script that drives IPC |
| niri | wlroots (Rust), PaperWM lineage | Scrollable tiling columns | KDL file, live reload | Config-driven window rules; no in-process scripting | Per-monitor independent window strips; opening a window never resizes others |
| macOS (WindowServer / Quartz Compositor) | Proprietary | Floating; Spaces; Stage Manager; window tabs | `defaults` / plist | AppleScript, Shortcuts, Accessibility API | The window server is the only process that touches the framebuffer; chrome is global and per-screen |
| Xfce | GTK (C), traditional | Floating (Xfwm4) | `xfconf` channels; GUI dialogs | Panel plugins; external helper processes | Traditional desktop metaphor at low resource cost; strict modularity across processes |

The next sections walk through each concern and record what aegis borrows and
what it rejects.

## Architecture: Where the Seams Are

A compositor can split responsibilities at several seams. The reference
systems cluster into three shapes.

**Monolithic shell over a compositing window manager.** GNOME Shell is a
Mutter plugin written largely in JavaScript; it owns the Activities overview,
the top bar, the dash, search, notifications, and the window picker. KWin
plays the same role for KDE Plasma, with QML plasmoids rendering the panels,
desktop, and widgets. macOS goes furthest: WindowServer is the single process
that may write the framebuffer, and it also routes the input event queue to
the owning process. The strength is coherence — the shell and the compositor
share one scene and one event loop. The cost is coupling: GNOME Shell cannot
swap Mutter without losing its extensions, and KDE's chrome is bound to KWin.

**Compositor core with out-of-process chrome.** sway and river ship no
chrome of their own. Bars, lock screens, wallpapers, and launchers are
separate programs that talk to the compositor over protocols (layer-shell,
`riverctl` IPC, the i3 IPC). The strength is modularity and replaceability;
the cost is that a working desktop is an integration project, and the
out-of-the-box experience is bare.

**Library core with a co-developed renderer.** niri is written in Rust on
wlroots and draws its own animations, overview, and screenshot UI inside the
compositor, but leaves the bar and launcher to external tools. It is the
closest precedent to aegis's shape: a Rust compositor that owns more than sway
but less than GNOME.

aegis adopts the library-core shape. The compositor, renderer, and shell split
is fixed in [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md);
the chrome is already a pluggable `Chrome` trait
([ADR-0021](../adr/0021-chrome-component-trait.md)). From GNOME and KDE aegis
keeps the ambition of a coherent first-party shell; from sway and river it
keeps the discipline that chrome is a component, not a privilege.

## Layout Model

Layout is the most consequential design axis. The field splits between
**floating** (macOS, GNOME, KDE, Xfce), **manual tiling** (sway/i3), and
**dynamic or scrollable tiling** (river, niri).

- **Floating** treats each toplevel as a free rectangle the user positions
  and resizes. It is the most forgiving model and the only one that handles
  arbitrary application windows (dialogs, toolbars, popups) without special
  cases. Its weakness is window management at scale: the user does the
  layout work.
- **Manual tiling** (sway) arranges windows into a tree of containers the
  user edits with the keyboard. It is precise and keyboard-first but rejects
  applications that assume free placement, and it imposes a workflow.
- **Dynamic tiling** (river) computes layout from rules each time the window
  set changes. river externalizes the policy to a separate layout-generator
  process over an IPC protocol, so users swap algorithms without rebuilding
  the compositor.
- **Scrollable tiling** (niri, PaperWM) lines windows up in columns on an
  infinite horizontal strip per monitor, so a new window never resizes its
  neighbors. niri's per-monitor independent strips solve a real problem
  PaperWM inherits from GNOME's global coordinate space.

GNOME and KDE both layer window snapping (edge-halves, quarters, maximize)
on top of floating. macOS layers Spaces, Stage Manager, and window tabbing
on top of floating. Xfwm4 is floating only.

aegis takes **floating as the universal base**, with window snapping and an
optional, policy-driven tiling layer applied on top, never as a replacement.
The reasoning and the rejection of tiling-only are recorded in
[ADR-0024](../adr/0024-layout-model.md). The tiling policy will be a
first-class part of the window manager rather than an external process, so
that the chrome, the focus model, and the introspection surface all see one
consistent scene. river's external layout generator is attractive but is
deferred: it is a natural extension point once the IPC seam
([ADR-0027](../adr/0027-ipc-and-introspection.md)) exists.

## Workspaces and Outputs

Workspaces are the second axis where the field diverges sharply.

- **GNOME** uses dynamic workspaces: a new empty one is always available, and
  the set grows and shrinks with use. Workspaces live on a single global
  vertical list, shown in the overview.
- **KDE Plasma** adds **Activities**: separate, named working contexts, each
  with its own virtual desktops, panels, and recently used apps. Activities
  are a heavier concept than workspaces.
- **sway/i3** use numbered static workspaces, one per output by default, with
  explicit move and assignment.
- **river** uses **tags** in the dwm/bspwm tradition: windows carry tag bits
  and a workspace shows the union of its tags.
- **niri** gives each monitor its own set of dynamic workspaces arranged
  vertically, and the workspace arrangement survives monitor unplug and
  replug where it makes sense. This per-monitor independence is a deliberate
  correction to GNOME's global workspace list.
- **macOS** Spaces are per-display virtual desktops managed through Mission
  Control, with drag-to-reorder and full-screen apps each becoming a Space.

aegis takes **dynamic, per-output workspaces** in the niri lineage, without
niri's scrollable-tiling coupling. The model is recorded in
[ADR-0025](../adr/0025-workspace-model.md). aegis rejects KDE's Activities as a
separate axis (the same effect is reachable with more workspaces and window
rules) and river's tag bits (dynamic workspaces are easier to reason about
and to expose to the AI-adaptation phase).

The **output model** is treated as its own concern in
[ADR-0028](../adr/0028-output-and-monitor-model.md): per-output independent
geometry, mixed DPI, fractional scaling through `wp_fractional_scale_v1`,
and workspace relocation on unplug. niri and macOS both prove that
per-output independence is worth the implementation cost.

## Configuration

Configuration is where the field is least disciplined.

- **sway** uses a plain-text file in i3 syntax at
  `~/.config/sway/config`, with a `set`/`variable` preprocessor. It is
  readable but inconsistent: some settings are expressions, some are
  commands, and live reload is partial.
- **GNOME** spreads configuration across `gsettings` schemas stored in
  `dconf`, plus per-extension folders under `~/.local/share/gnome-shell/
  extensions`. It is thorough but fragmented and hard to version as a unit.
- **KDE Plasma** uses `~/.config` INI-like files, edited through
  `kwriteconfig5` or the GUI. It is closer to a single tree but still spread
  across many files.
- **river** has no config file at all: the user writes a shell script that
  issues `riverctl` IPC calls at login. It is maximally expressive but
  unstructured, and there is no schema to validate against.
- **niri** uses a KDL file with full live reload and rich window-rule
  matching. It is the current high-water mark for compositor configuration.
- **Xfce** uses `xfconf` channels, editable through GUI dialogs, with hidden
  settings reachable only by direct channel edits.

aegis standardizes on a **single declarative TOML file** with a versioned
schema, full live reload, and validation errors reported back to the user
rather than silently ignored. The decision, and the rejection of an embedded
scripting language and of ad hoc INI, is in
[ADR-0026](../adr/0026-configuration-system.md). TOML is chosen over KDL for
tooling maturity and over YAML for explicitness. The current `$AEGIS_KEYBINDS`
environment variable (see [CHANGELOG](../../CHANGELOG.md)) is a placeholder
that the config file subsumes.

## Extension and Automation

Every mature shell grows an extension surface; the question is where the
seam sits and how much it can do.

- **GNOME Shell extensions** are JavaScript that runs inside the shell
  process, with full access to the scene. They are powerful and fragile:
  an extension upgrade often breaks on a GNOME version bump.
- **KDE Plasma** offers QML plasmoids, C++ plugins, KWin effects scripts,
  and KRunner plugins. The breadth is a strength; the number of seams is a
  maintenance cost.
- **sway/i3** expose an **IPC** (a unix socket speaking a JSON protocol)
  that external programs use to query and mutate state. i3status, i3blocks,
  rofi, and waybar all build on it. The IPC is the extension surface.
- **river** is configured entirely through its IPC; there is no in-process
  extension.
- **niri** deliberately has no in-process scripting; everything is a config
  window rule. Power users add waybar or fuzzel.
- **macOS** offers AppleScript, Shortcuts, and the Accessibility API as
  out-of-process automation, plus Automator. None run inside WindowServer.
- **Xfce** panel plugins are shared libraries loaded by the panel process,
  so a crashing plugin does not bring down the session.

The pattern is clear: out-of-process IPC is the durable, versionable
extension surface; in-process scripting is powerful but brittle. aegis chooses
the IPC-first path and rejects in-process scripting for the core. The seam
is recorded in [ADR-0027](../adr/0027-ipc-and-introspection.md): a versioned,
schema-driven protocol over a unix socket that exposes the same model the
shell reads. This seam is also the foundation of the AI-adaptation phase
described in [Vision and Scope](vision.md): an agent that can drive the
machine needs the same structured, queryable surface a power-user script
needs.

## Chrome

Chrome is where aegis is most explicit about borrowing.

| Chrome surface | aegis borrows from | Notes |
|----------------|------------------|-------|
| Top bar / panel | GNOME, macOS | A single status area, not a forest of panels |
| Dock | macOS, GNOME Dash | Bottom-center overlay, already shipped ([ADR-0019](../adr/0019-dock-as-bottom-center-overlay.md)) |
| Application launcher | GNOME, KDE KRunner, macOS Spotlight | Centered list with search, already shipped ([ADR-0022](../adr/0022-application-launcher.md)) |
| Overview | GNOME, niri | A unified window-and-workspace picker; planned, not shipped |
| Borderless window controls | sway, macOS | Compositor-owned gestures, invisible resize borders, and shell controls; users may opt into client-drawn frames ([ADR-0063](../adr/0063-compositor-owned-borderless-decoration-policy.md)) |
| Window list | Xfce, KDE | A fallback chrome component; already shipped |
| Notifications | freedesktop.org spec | Planned, served by the same `Chrome` trait |
| Wallpaper | sway (`swaybg`), Xfce | Already shipped as its own crate ([ADR-0018](../adr/0018-wallpaper-crate.md)) |

aegis rejects GNOME's tight binding between the shell and the compositor: the
chrome is registered into the core at startup and can be replaced or omitted.
aegis also rejects sway's "no chrome at all" default: a coherent first-run
experience is part of the product, not an integration exercise. The
mechanism that makes both positions hold is the `Chrome` trait
([ADR-0021](../adr/0021-chrome-component-trait.md)).

## Input and Rendering

- **GNOME** and **KDE** both ship multi-touch gestures, HiDPI, fractional
  scaling, color management, and (in KDE 6) HDR and ICC profiles. This is
  the floor a modern compositor is measured against.
- **niri** treats input as a first-class concern: touchpad gestures, tablet
  mapping to a specific monitor, and (planned) touch gestures, plus
  fractional scaling that keeps the compositor's own UI pixel-perfect.
- **sway/river** leave gesture handling minimal and rely on the wlroots
  input backend; fractional scaling arrived later through
  `wp_fractional_scale_v1`.
- **macOS** owns input routing in WindowServer and runs all final
  composition on the GPU through Metal.

aegis builds on Vulkan through flux rather than OpenGL/GLES, so HDR, color
management, and explicit sync land as flux capabilities rather than
compositor workarounds, following [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md).
HiDPI and fractional scale are output-model concerns
([ADR-0028](../adr/0028-output-and-monitor-model.md)); touchpad gestures and
tablet support arrive with the libinput backend in the DRM/KMS milestone.

## What aegis Rejects

To stay small, aegis deliberately leaves the following to other layers or to
later phases:

- **An X11 server.** aegis is Wayland-only; X11 applications are unsupported
  (XWayland descoped; the strategy remains in
  [ADR-0030](../adr/0030-xwayland-strategy.md) should it be revisited). aegis
  will not grow an X11 session like the KDE `kwin_x11` split.
- **In-process shell scripting.** No JavaScript, no QML, no Lua inside the
  compositor. Power and automation go through the IPC
  ([ADR-0027](../adr/0027-ipc-and-introspection.md)).
- **A bundled application suite.** No file manager, terminal, text editor,
  or image viewer. Xfce and KDE ship these; aegis does not, and the launcher
  discovers whatever the host installed.
- **Fragmented configuration stores.** One TOML file, one schema
  ([ADR-0026](../adr/0026-configuration-system.md)).
- **Tag-bit workspaces and Activities.** Dynamic per-output workspaces cover
  the same ground with a simpler model
  ([ADR-0025](../adr/0025-workspace-model.md)).

## See Also

- [Vision and Scope](vision.md) — how the survey turns into a product
  direction.
- [Roadmap](roadmap.md) — the milestone sequence that delivers it.
- [Architecture](architecture.md) — the component boundaries the survey
  assumes.
