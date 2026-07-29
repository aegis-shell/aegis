# aegis-statusbar

`aegis-statusbar` is the top status bar chrome component for the aegis
compositor, built on the `Chrome` contract from `aegis-shell` and shared
materials from `aegis-design`. It hosts the session HUD (workspace state,
active-window title, clock, live system controls, and notifications) and the
compositor's StatusNotifierItem (SNI) system tray.

## Responsibilities

- Draw a compact translucent bar across the top edge of the output.
- Show one workspace dot per workspace (click to switch), the active
  window's icon and title, a polled clock, and an application tray of
  running app icons.
- Present compact system status as icon buttons, with direct volume
  mute/step actions on click and scroll.
- Open one status-and-controls panel for volume, brightness, Wi-Fi,
  Bluetooth, Do Not Disturb, current-workspace layout, and recent
  notifications.
- Show a persistent, state-colored Agent Workspaces entry. It reports no live
  Realm, one Realm's own label, or a multi-Realm aggregate without inferring
  agent process state; clicking it opens Agent Workspaces directly.
- Run the session's StatusNotifierWatcher + Host on the session D-Bus,
  render registered SNI items' icons in the tray row, and forward
  left-click (`Activate`), right-click (`SecondaryActivate`, or a
  host-rendered dbusmenu popover when the item exposes a `Menu` object
  path) to each item. See [`tray`](src/tray/mod.rs).
- Fold open-window cells and SNI cells into a fixed slot budget with a
  `+N` overflow indicator.
- Emit workspace-switch, focus, notification-dismiss, and system-action
  intents through `ChromeEvents`.

## Boundaries

The status bar owns presentation and interaction state only. Window,
workspace, notification, Realm, and system snapshots arrive through the shell
each frame; the decoded application icon textures are borrowed from the
composition root's icon cache and pushed through
`Chrome::update_app_catalog`. It never mutates Wayland state or writes
configuration.

The SNI tray is the workspace's first D-Bus surface. `zbus` serves watcher
methods on its internal executor, while signals and outgoing commands run on
two dedicated `std::thread`s (`sni-tray-signals`, `sni-tray-commands`).
Registration publishes a placeholder and replies before an executor task
reads the item's properties, so a synchronous item never waits on property
requests that it cannot serve until registration returns.

The integration uses `zbus` v5 with `default-features = false` and the
`async-io` + `blocking-api` features — pure Rust, no libdbus system dependency,
and no `tokio`/`async-std` in the dependency graph. State crosses thread
boundaries only as plain values through `Arc<Mutex<_>>` and
`std::sync::mpsc`; the compositor's render thread reads a `TraySnapshot` once
per frame and never blocks on D-Bus. SNI icon pixmaps are uploaded through a
borrowed `flux::Device` (refcounted in C, held non-owning on the render thread,
matching `Shell::new`'s pattern).

Failure stays silent: without a session bus, or when another watcher
already owns the `org.kde.StatusNotifierWatcher` name, the tray simply
shows no SNI icons and startup is unaffected.

## Runtime Effect

Each frame the status bar reports its reserved top edge
(`aegis_shell::HUD_HEIGHT`), its backdrop-blur regions (the bar plus the
open panel or dbusmenu popover), and captures pointer input over its
own surfaces. The composition root registers it conditionally from the
`[statusbar] enabled` configuration; the default is `true`.

## Use

Register one status bar with the shell, sharing the flux device and the
notification queue:

```rust
shell.add(Box::new(aegis_statusbar::StatusBar::with_notifications(
    &device,
    std::sync::Arc::clone(&notif_queue),
)));
```

`Shell::add` seeds newly registered components with the current
application catalog and system status.

## Related Documentation

- [Status bar crate and SNI tray decision](../../docs/adr/0045-statusbar-crate-and-sni-tray.md)
- [Status bar system controls](../../docs/adr/0060-statusbar-system-controls-and-live-system-ipc.md)
- [Generic Agent Workspaces status surface](../../docs/adr/0074-generic-agent-workspaces-status-surface.md)
- [Chrome component decision](../../docs/adr/0021-chrome-component-trait.md)
- [Design system decision](../../docs/adr/0046-design-system-crate.md)
- [Configuration reference: `[statusbar]`](../../docs/reference/config.md#status-bar)
