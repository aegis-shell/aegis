# ADR-0044: Dock and Control Center as component crates

- Status: Accepted
- Date: 2026-07-19

## Context

[ADR-0021](0021-chrome-component-trait.md) split `aegis-shell` into a core
host and pluggable `Chrome` components, and deferred one crate per
component until a component gained "a dependency or lifecycle the core
should not own". Two components now meet that trigger:

- The dock (~1270 lines) keeps per-tile spring animation state, a
  pinned-app model with window-folding match keys, decoded icon texture
  dependencies, and a context menu with pin actions.
- The Control Center (~1400 lines) keeps paged navigation state, consumes
  the `SystemStatus` model, emits display/touchpad/network/volume
  `SystemAction` mutations, drives Realm management through
  `RealmIntent`, and depends on the application catalog and icons.

Meanwhile the shared trait had grown component-specific leaks.
`Chrome::update_app_catalog` named the dock-private `DockApp` type, so
every component's catalog path referenced dock internals. Decoded GPU
textures crossed the seam as a raw `HashMap<String, *mut c_void>`, an
ownership invariant carried only by comments. App match-key logic lived
in the binary even though both the binary (pin matching) and the dock
(icon lookup, folding running windows into tiles) need it.

A crate boundary is not a process boundary. This decision concerns code
organization only; both components keep rendering in the compositor
process either way.

## Decision

### 1. Promote the dock and the Control Center to their own crates

New crates `aegis-dock` and `aegis-ctl-center`, each depending on the
`aegis-shell` contract (`Chrome`, `ChromeEvents`, `AppCatalog`, `AppMenu`)
and registered by the binary. `aegis-shell` keeps the `Shell` host, the
`Chrome` trait, `ChromeEvents`, shared building blocks (`AppMenu`), and
the remaining components (launcher, workspace bar and HUD, overview,
toast, screenshot selector, decorations). The dependency direction is
`aegis-core` ← `aegis-shell` ← {`aegis-dock`, `aegis-ctl-center`} ← `ass`;
`aegis-shell` depends on no component crate, and `ass` remains the
composition root that decides which components exist.

### 2. `AppCatalog` and `IconSet` replace the leaky signature

`IconSet` is a named, cloneable handle map for borrowed icon textures.
It documents the ownership invariant in one place: the composition
root's icon cache owns the GPU textures and must keep them alive until
it has pushed a replacement catalog; components dereference only handles
from the most recently pushed set.

`AppCatalog` is the one immutable snapshot pushed to chrome:

- `apps` — every launchable entry: enumerated XDG applications plus
  compositor-owned built-ins.
- `pinned` — the user's pinned favorites, resolved against `apps` by the
  composition root from the `[dock] pinned` configuration (or
  auto-populated when unconfigured).
- `icons` — the `IconSet`, keyed by every lowercase id an entry might
  run as.

`Chrome::update_app_catalog` now takes `&AppCatalog` and names no
component-specific type. `Shell` owns the current catalog, seeds it into
every component added later via `Shell::add`, and fans a replacement out
through `Shell::set_app_catalog` — the same push pattern as
`set_system_status` and `set_realms`. Components no longer take catalog
data through constructors.

### 3. App match keys move onto `Entry`

`Entry::match_keys` in `aegis-core` is the single source for the
lowercased ids an entry may be matched by (`StartupWMClass`,
desktop-file stem, icon name). The binary uses it for pin resolution and
pin-toggle matching; `aegis-dock` uses it for icon lookup and for folding
running windows into pinned tiles.

### 4. A crate split is not a process split

Both components still render in the compositor process each frame. The
dock remains compositor chrome indefinitely: its reserved edges,
backdrop-blur regions, modal interplay, pointer capture, and per-frame
animation ticks are compositor-internal paths that a layer-shell-style
protocol would not cover, and no such protocol path exists in the
project. The Control Center is the candidate to eventually become an
ordinary `xdg_toplevel` client — but only after
[ADR-0027](0027-ipc-and-introspection.md)'s IPC grows a `SystemStatus`
subscription and typed system commands (volume, brightness, Wi-Fi,
display configuration). Moving it out today would force it to bypass
the state-in/intent-out boundary and operate on the host directly.

## Alternatives

- **Keep both as `aegis-shell` modules.** Rejected: [ADR-0021](0021-chrome-component-trait.md)'s
  own promotion trigger is met, the core was accumulating
  component-specific state, and the trait leak was spreading — `DockApp`
  had already appeared in three unrelated components' update signatures.
- **Run the dock or the Control Center as external processes now.**
  Rejected: the IPC can subscribe windows, workspaces, and Realms but
  has no `SystemStatus` event and no typed system mutation commands, and
  the dock's reserved-edge, backdrop-capture, modal, and frame-tick
  integration has no protocol path at all.
- **A dynamic plugin ABI (`.so` components).** Rejected: premature. The
  binary stays the composition root with static registration; if
  configurability is needed later, a static registry or a config flag in
  `ass` covers it without an ABI.
- **Move system probing out of `aegis-shell` now.** Deferred: the probing
  code is host-only, but the placement of the `SystemStatus` /
  `SystemAction` types is a separate decision tied to the future IPC
  surface and is better made with it.

## Consequences

- `aegis-shell`'s public API names no component-specific type; a component
  can leave the core without trait changes, and `DockApp` is private to
  `aegis-dock` again.
- The icon-texture lifetime invariant has a named home (`IconSet`)
  instead of a comment on the binary's cache.
- Pin resolution stays in the composition root, which owns the
  configuration; the dock derives its tile model from the pushed
  catalog, keeping the state-in/intent-out direction.
- `Entry::match_keys` replaces a binary-local helper both the binary and
  the dock used.
- Follow-up work this decision creates: converge `ChromeEvents` toward
  component-agnostic intents (remove cross-component addressing such as
  `toggle_launcher` and dock-specific `dock_pin_toggles`); move system
  probing and its types out of `aegis-shell`; extend the IPC with a
  `SystemStatus` subscription and typed system commands; only then can
  the Control Center become an external client whose crash cannot take
  down the compositor.
