# aegis-hud

`aegis-hud` is the display-only session HUD for the aegis compositor
(ADR-0080, ADR-0083), built on the `Chrome` contract from `aegis-shell` and
shared materials from `aegis-design`. What used to be the interactive top
status bar is now two floating frosted chips in minimal FPS-HUD style;
every interaction the bar once hosted moved to the command panel
(`aegis-command-panel`).

## Responsibilities

- Draw two compact frosted chips composited over the desktop: system
  status (network, Bluetooth, battery), the StatusNotifierItem tray row,
  the clock, and the notification count on the left; workspace dots in the
  center. The top-right belongs to the frameless notification toast strip
  (ADR-0083).
- Reserve no space: `Chrome::reserved()` stays at the default, so tiled and
  maximized windows run underneath the chips.
- Accept no pointer input: `Chrome::captures_pointer()` stays `false`, so
  clicks fall through to the windows below — including on tray icons.
- Fade each chip out when the cursor approaches it (a per-chip eased fade
  with a proximity margin), honoring the reduced-motion policy; raster tray
  and themed icons draw only above an alpha floor because lens cannot fade
  images.
- Read the session's StatusNotifierItem tray snapshot from the shared
  `aegis-tray` service and render registered items' icons in the tray row.
  The HUD never sends tray commands; tray interaction (including the
  host-rendered dbusmenu popover) lives in the command panel.
- Fold SNI cells into a fixed slot budget with a `+N` overflow indicator.
- Hide entirely while a fullscreen window owns the output.

## Boundaries

The HUD owns presentation state only. Window, workspace, notification, and
system snapshots arrive through the shell each frame; the decoded
application icon textures are borrowed from the composition root's icon
cache and pushed through `Chrome::update_app_catalog`. It emits no
`ChromeEvents` intents and never mutates Wayland state or writes
configuration. The Agent Workspaces status the right chip once showed moved
to the command panel's System section (ADR-0083).

The SNI tray service lives in the `aegis-tray` crate (re-exported here as
`tray`); the composition root spawns it once and shares the snapshot with
both this HUD (read-only) and the command panel (read + command). SNI icon
pixmaps are uploaded through a borrowed `flux::Device` (refcounted in C,
held non-owning on the render thread, matching `Shell::new`'s pattern).

## Runtime Effect

Each frame the HUD reports its backdrop-blur regions (the visible chips) and
keeps the frame loop ticking while a proximity fade is in flight. The
composition root registers it conditionally from the `[hud] enabled`
configuration; the default is `true`.

## Use

Register one HUD with the shell, sharing the flux device, the tray snapshot,
and the notification queue:

```rust
shell.add(Box::new(aegis_hud::Hud::with_sources(
    &device,
    tray_snapshot,
    std::sync::Arc::clone(&notif_queue),
)));
```

`Shell::add` seeds newly registered components with the current
application catalog and system status.

## Related Documentation

- [HUD status chips and SAO command panel decision](../../docs/adr/0080-hud-status-chips-and-sao-command-panel.md)
- [Frameless transient toasts and HUD consolidation decision](../../docs/adr/0083-frameless-transient-toasts-and-hud-consolidation.md)
- [Status bar crate and SNI tray decision](../../docs/adr/0045-statusbar-crate-and-sni-tray.md)
- [Status bar system controls](../../docs/adr/0060-statusbar-system-controls-and-live-system-ipc.md)
- [Generic Agent Workspaces status surface](../../docs/adr/0074-generic-agent-workspaces-status-surface.md)
- [Chrome component decision](../../docs/adr/0021-chrome-component-trait.md)
- [Design system decision](../../docs/adr/0046-design-system-crate.md)
- [Configuration reference: `[hud]`](../../docs/reference/config.md)
