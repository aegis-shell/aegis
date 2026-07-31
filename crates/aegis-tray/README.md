# aegis-tray

StatusNotifierItem (SNI) system tray service for the aegis compositor.

The crate runs as both the StatusNotifierWatcher and a StatusNotifierHost on
the session bus: a worker setup owns the `org.kde.StatusNotifierWatcher`
name, tracks every registered item, and mirrors them into a plain
`TraySnapshot` the render thread reads. No async runtime is involved — zbus
runs its blocking API on two `std::thread`s (signal tracking and command
dispatch) and shares state through `Arc<Mutex<_>>` + `std::sync::mpsc`.

`spawn()` returns the shared snapshot and the command channel, or `None`
when the tray is unavailable (no session bus, or another watcher owns the
name). Shell chrome components consume it read-only for display (the HUD)
or send `TrayCommand`s for interaction (the command panel's tray
section, including host-rendered dbusmenu context menus).

## Boundaries

This crate owns D-Bus protocol machinery and icon pixmap production only. It
has no GPU, lens, or Wayland dependency; uploading pixmaps to textures and
presenting items is the consuming chrome component's job.
