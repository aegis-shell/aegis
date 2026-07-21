# ass-statusbar

`ass-statusbar` is the top status bar chrome component for the ass
compositor, built on the `Chrome` contract from `ass-shell`. It hosts
the session HUD (workspace state, active-window title, clock, system
status, notification panel) and the compositor's StatusNotifierItem
(SNI) system tray.

## Responsibilities

- Draw a compact translucent bar across the top edge of the output.
- Show one workspace dot per workspace (click to switch), the active
  window's icon and title, a polled clock, and an application tray of
  running app icons.
- Present compact system status (volume, network, battery, notifications)
  as icon buttons, with volume mute/step actions on click and scroll.
- Open a small notification panel from the bell button.
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
workspace, notification, and system snapshots arrive through the shell
each frame; the decoded application icon textures are borrowed from the
composition root's icon cache and pushed through
`Chrome::update_app_catalog`. It never mutates Wayland state or writes
configuration.

The SNI tray is the workspace's first D-Bus surface. It runs on two
dedicated `std::thread`s (`sni-tray-signals`, `sni-tray-commands`) using
`zbus` v5 with `default-features = false` and the `async-io` +
`blocking-api` features — pure Rust, no libdbus system dependency, no
`tokio`/`async-std` in the dependency graph. State crosses the thread
boundary only as plain values through `Arc<Mutex<_>>` and
`std::sync::mpsc`; the compositor's render thread reads a `TraySnapshot`
once per frame and never blocks on D-Bus. SNI icon pixmaps are uploaded
through a borrowed `flux::Device` (refcounted in C, held non-owning on
the render thread, matching `Shell::new`'s pattern).

Failure stays silent: without a session bus, or when another watcher
already owns the `org.kde.StatusNotifierWatcher` name, the tray simply
shows no SNI icons and startup is unaffected.

## Runtime Effect

Each frame the status bar reports its reserved top edge
(`ass_shell::HUD_HEIGHT`), its backdrop-blur regions (the bar plus the
open panel or dbusmenu popover), and captures pointer input over its
own surfaces. The composition root registers it conditionally from the
`[statusbar] enabled` configuration; the default is `true`.

## Use

Register one status bar with the shell, sharing the flux device and the
notification queue:

```rust
shell.add(Box::new(ass_statusbar::StatusBar::with_notifications(
    &device,
    std::sync::Arc::clone(&notif_queue),
)));
```

`Shell::add` seeds newly registered components with the current
application catalog and system status.

## Related Documentation

- [Status bar crate and SNI tray decision](../../docs/adr/0045-statusbar-crate-and-sni-tray.md)
- [Component crate split (dock + Control Center precedent)](../../docs/adr/0044-dock-and-control-center-crates.md)
- [Chrome component decision](../../docs/adr/0021-chrome-component-trait.md)
- [Configuration reference: `[statusbar]`](../../docs/reference/config.md#status-bar)
