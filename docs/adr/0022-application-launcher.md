# ADR-0022: Application launcher via freedesktop.org desktop entries

- Status: Accepted
- Date: 2026-06-20

## Context

[ADR-0016](0016-shell-server-window-management-bridge.md),
[ADR-0019](0019-dock-as-bottom-center-overlay.md), and
[ADR-0021](0021-chrome-component-trait.md) all defer the launcher: a way for a
user to discover and start application-level programs from the compositor
chrome. Until now ass had no application menu, no `.desktop` parsing, and no
path to spawn an external process in response to a chrome interaction (the only
`Command::spawn` in the tree is `aegis-wallpaper`'s `ffmpeg` decoder).

Three constraints shape the design:

1. **Spec correctness.** A launcher must honor the freedesktop.org
   [Desktop Entry Specification](https://specifications.freedesktop.org/desktop-entry-spec/)
   (search `$XDG_DATA_HOME` and `$XDG_DATA_DIRS` for `applications/*.desktop`,
   filter `Type=Application`, drop `NoDisplay`, honor `TryExec`, resolve
   localized `Name[xx_YY]` by `LC_MESSAGES`, deduplicate by desktop id with the
   user directory overriding the system) and the lookup half of the
   [Icon Theme Specification](https://specifications.freedesktop.org/icon-theme-spec/)
   (walk the theme inheritance chain with `hicolor` as the mandatory final
   fallback).
2. **Modularity.** Per [ADR-0001](0001-scope-and-responsibility-boundary.md)
   and `docs/dev/project-layout.md`, code with no flux / flux-ui / Wayland
   dependency does not belong in `aegis-shell`; and
   [ADR-0021](0021-chrome-component-trait.md) says a chrome component that
   grows a `.desktop`-parsing dependency should leave `aegis-shell` for its own
   crate. So the parsing cannot live in the shell.
3. **The spawn path must detach.** A launched app must outlive the compositor,
   inherit the Wayland / XDG environment it needs to connect back, and never
   share the compositor's stdio or controlling terminal.

## Decision

### 1. Two new leaf crates: `aegis-desktop-entries` and `aegis-launcher`

`aegis-desktop-entries` (depends on `dirs`, `rust-ini`, `aegis-core`) owns desktop-entry
enumeration, locale resolution, `Exec` field-code expansion, and icon-theme
lookup. It has no flux / flux-ui / Wayland dependency.

`aegis-launcher` (depends on `aegis-desktop-entries`, `aegis-core`) owns the spawn path. It is a
leaf: only the binary depends on it.

### 2. The shared model lives in `aegis-core::app`

`Entry` — a parsed, launchable desktop entry — is a plain-`std` struct in
`aegis-core::app`. `aegis-desktop-entries` builds it; `aegis-shell`'s launcher and `aegis-launcher`
both read it. Because the model is in `aegis-core`, the shell chrome renders a
launcher **without depending on `aegis-desktop-entries`**, preserving the seam
[ADR-0021](0021-chrome-component-trait.md) established.

### 3. Detached spawning via the external `setsid` binary

`aegis-launcher::launch` expands the entry's `Exec` (stripping `%`-field codes and
POSIX single-quoting every token via `aegis-desktop-entries`), optionally wraps it in a
terminal emulator when `Terminal=true`, and runs it under
`setsid --fork sh -c '<expanded>'`. The child lands in a new session, detached
from the compositor's process group and controlling terminal, and inherits the
display environment a Wayland / XDG client needs (`WAYLAND_DISPLAY`,
`XDG_RUNTIME_DIR`, `DISPLAY`, `HOME`, `PATH`, locale). No `unsafe` or `libc`
is used: process detachment is delegated to `setsid`, the same pattern
`aegis-wallpaper` uses for `ffmpeg`.

### 4. A `Launcher` chrome component in `aegis-shell`

A new `chrome::Launcher` implementing `Chrome` renders a small top-center
toggle that expands into a centered overlay listing every enumerated entry.
A row click sets a new `ChromeEvents::spawn: Option<Entry>` intent, which the
main loop drains into `aegis-launcher`. The component holds only a single
`ass_core::launcher::Launcher` brain and a bool, so it stays in `aegis-shell`
and adds no dependency to it.

### 5. Keyboard capture while the launcher is open

The launcher's search is keyboard-driven. The `Chrome` trait gains two
default no-op methods — `captures_keyboard(&self) -> bool` and
`key_char(&mut self, &KeyChar, &mut ChromeEvents)` — so adding keyboard
handling touches only the launcher, not the other three components. While any
component reports capture, the main loop:

- calls `Server::key_char(code, pressed)` for each `Key` event, which advances
  the server's xkbcommon state and returns the resolved keysym + printable
  character (a new `Keyboard::update_key` reports the keysym/utf8 alongside the
  modifier mask);
- feeds the resolved `KeyChar` to `Shell::key_char` only on key press (so
  typed characters are not double-counted on release);
- withholds captured `Key` events from `forward_input`, so the focused client
  does not also receive them;
- sends `wl_keyboard.leave` to the focused client when the launcher opens and
  `wl_keyboard.enter` back when it closes, via the server's new
  `grab_keyboard_focus` / `release_keyboard_focus` pair. The release restores
  the pre-grab focus only if nothing else took focus during the session (a
  launcher "focus running app" action, a pointer click, …), so an explicit
  focus change is never overridden.

The brain's query/filter/selection logic lives in `aegis-core::launcher` (pure,
unit-tested, no flux-ui); the `aegis-shell` component is a thin flux-ui adapter
over it. The `KeyChar` → `KeyAction` classification (`ass_core::input::key_action`)
is likewise pure and tested.

### 6. A global Super-tap hotkey opens the launcher

A bare Super tap (press and release with no other key in between) toggles the
launcher, even while a client has keyboard focus. Detection is a pure state
machine in `ass_core::input::TapDetector`, fed every `Key` event by the main
loop in both the captured and uncaptured paths. The tap is **observed, not
intercepted**: Super key events still forward to the focused client, so Super
keeps working as a modifier in every other combo (`Super+letter`,
`Super+drag`, …). Only a clean tap fires. Left and right Meta are treated as
the same logical key. The `Chrome` trait gains a default-no-op `toggle`;
`Shell::toggle` fans out to components, and the launcher overrides it to flip
its brain.

### 7. Running-app awareness via `app_id` ↔ `StartupWMClass`

The launcher does not spawn a second instance of an app that is already
running. Each frame the chrome refreshes the brain with the server's live
`(app_id, surface_id)` pairs (from `xdg_toplevel.set_app_id`, already captured
on `ass_core::window::Window`). An entry matches a running instance when the
client's `app_id` equals the entry's `StartupWMClass`, or — when that is
unset — equals the entry's desktop id with its `.desktop` suffix stripped
(case-insensitively, mirroring how most toolkits derive `app_id` from the
desktop file name). Activating a matched entry emits a focus intent (reusing
the existing `ChromeEvents::clicked` → `Server::focus_surface_by_id` path)
instead of a spawn; the render marks running rows with a leading `●`.

### 8. The binary owns discovery and wiring

The binary calls `ass_apps::enumerate()` once at startup, hands the snapshot to
`Launcher::new`, and in the frame loop drains `Shell::take_spawn()` into
`aegis-launcher::launch`. Neither `aegis-shell` nor `aegis-render` gains a dependency
on `aegis-desktop-entries` or `aegis-launcher`.

## Alternatives

- **Parse `.desktop` files inside `aegis-shell`.** Rejected: would pull `dirs`
   and `rust-ini` into the chrome crate, violating the placement rule in
   [ADR-0001](0001-scope-and-responsibility-boundary.md) and the explicit
   deferral in [ADR-0021](0021-chrome-component-trait.md).
- **Link `gio` / `glib` (`GDesktopAppInfo`).** Rejected: a heavy C dependency
   and a GLib main-loop worldview, neither of which the rest of ass uses. The
   pure-Rust `rust-ini` path keeps the build hermetic.
- **`libc::fork` + `setsid(2)` via `unsafe pre_exec`.** Rejected: avoids no
   real cost (the external `setsid` binary is present on every util-linux
   host) and adds `unsafe` and a `libc` dep for no gain. Delegating to a child
   binary mirrors the accepted `ffmpeg` pattern in
   [ADR-0018](0018-wallpaper-crate.md).
- **Full Icon Theme Spec (`index.theme` `Directories` / `Context` / `MinSize`
   / `MaxSize` / `Threshold`) and the Menu Spec tree.** Deferred: this
   revision reads only `index.theme`'s `Inherits` line and picks the icon size
   closest to a target by directory-name heuristic. Flat enumeration is
   sufficient for a launcher; the full table is tracked as follow-up.
- **Keyboard text-search and the global Super-tap hotkey.** Implemented in
   this revision (Decisions 5 and 6). A *configurable* keybind (e.g.
   `Super+Space`) is deferred; the tap detector covers the discoverable case.
- **Promote `Launcher` to its own `aegis-launcherer` crate.** Deferred: the
   component currently reads only `ass_core::app::Entry`, so it has no
   dependency [ADR-0021](0021-chrome-component-trait.md) says to split on.
   The `Chrome` trait makes that promotion near-zero-cost when it earns it
   (e.g. when it grows a `.desktop`-icon texture loader or an animation loop).

## Consequences

- Two new leaf crates (`aegis-desktop-entries`, `aegis-launcher`) and one new module
  (`aegis-core::app`) join the workspace. `aegis-shell` gains a `Launcher`
  component and one new `ChromeEvents` field; its dependency graph is
  unchanged.
- The main loop gains one enumeration step at startup and one intent-drain arm
  per frame.
- A launched app requires `/usr/bin/setsid` (util-linux) on the host. If
  absent, `launch` returns a spawn error that the main loop logs and ignores;
  the rest of the compositor is unaffected.
- Application icons are resolved to absolute `PathBuf`s (`Entry::icon_path`)
  but are **not yet rendered as textures**: flux-ui's `Icon` is glyph-based,
  and wiring arbitrary image decode into chrome is deferred. The dock's
  placeholder glyph approach ([ADR-0019](0019-dock-as-bottom-center-overlay.md))
  applies to the launcher rows too until that lands.
- Application discovery runs once at process start; a runtime rescan (e.g. when
  a package is installed while ass runs) is not yet supported.
- Keyboard capture sends a proper `wl_keyboard.leave` on grab and
  `wl_keyboard.enter` on release, so the focused client's state stays
  consistent. The release restores the pre-grab focus only if it is still
  vacant; an explicit focus change made during the session (launcher focus
  action or pointer click) wins. Modifier state stays consistent because the
  server's xkbcommon state advances for every captured event and the modifier
  mask is re-sent on the next forwarded key.
- The Super-tap hotkey treats left and right Meta as one key and fires on the
  final release; pressing both supers at once and releasing is an undefined
  corner that may fire once. Bare Super key events still reach the focused
  client, which is standard compositor behavior.
- Running-app matching keys off `StartupWMClass` first and falls back to the
  desktop-id stem. Apps that set neither a matching `app_id` nor a
  `StartupWMClass` (some Electron / Flatpak apps) will not be recognized as
  running and get a fresh spawn; a future heuristic could also match on the
  window title or the binary name.
