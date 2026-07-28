# ADR-0051: xdg-desktop-portal backend as an out-of-process D-Bus bridge

- Status: Proposed
- Date: 2026-07-24

## Context

Sandboxed (Flatpak) and portal-aware applications resolve desktop services
through `xdg-desktop-portal`, which delegates to a per-desktop backend
selected by `XDG_CURRENT_DESKTOP`. aegis now exports `XDG_CURRENT_DESKTOP=aegis`
into the D-Bus activation environment, so the frontend looks for an aegis
backend and, finding none, either fails the call or misroutes it to another
desktop's backend.

The pixels a Screenshot portal needs already exist behind a security
boundary aegis deliberately designed: scoped `CaptureOutput` over the IPC with
sealed-memfd transport
([ADR-0037](0037-scoped-pixel-capture-over-ipc.md),
[ADR-0041](0041-sealed-file-descriptor-pixel-transport.md)). That boundary
is fail-closed — the operation requires the `control` capability, an
explicit `CaptureOutput` entry in a named scope's `ops`, and a live lease —
and it is gated on session lock and seat activity. ADR-0027 already
anticipated this shape: "a D-Bus bridge is implementable later as a separate
process over this IPC if a desktop integration ever needs it."

A portal backend is a long-lived, bus-activated daemon that must not share
fate with the compositor, must not hold Wayland capture privileges, and must
not pull an async runtime into the compositor's process.

## Decision

aegis ships a portal backend as a **standalone process, `aegis-portal`** (new
`crates/aegis-portal` crate), that is a pure bridge: outward it speaks D-Bus
(zbus blocking API on the session bus, plain `std::thread` workers, no
tokio — the same dependency red line as the SNI tray); inward it is an
ordinary scoped IPC client of the compositor. No Wayland capture protocol is
added anywhere.

The backend serves at `/org/freedesktop/portal/desktop` under
`org.freedesktop.impl.portal.desktop.aegis`, D-Bus-activated:

- **`org.freedesktop.impl.portal.Settings` v1.** `Read`/`ReadAll` answer
  `org.freedesktop.appearance` `color-scheme` from the compositor
  configuration's new `[appearance] color_scheme` key, re-read per call so
  the compositor's live reload is honored without a watcher. `SettingChanged`
  is declared for introspection but not emitted until the setting joins the
  revisioned IPC settings snapshot.
- **`org.freedesktop.impl.portal.Screenshot` v1, non-interactive only.**
  `Screenshot` registers an `org.freedesktop.impl.portal.Request` object at
  the `handle_token`-derived path, captures the focused output through
  `CaptureOutput` on a dedicated worker thread, writes the PNG mode `0600`
  under `$XDG_CACHE_HOME/aegis-portal` (falling back to `$XDG_RUNTIME_DIR`),
  emits `Response` with a `file://` URI, and removes the request object.
  `interactive = true` answers with response code 2; `Request.Close` races a
  capture into response code 1.

The IPC grant uses a **built-in owner-only named scope** `aegis-portal`
(`aegis_ipc::LOCAL_PORTAL_SCOPE`) resolved by the compositor to exactly one
operation, `CaptureOutput` — mirroring the `aegis-ctl-realm-admin`
precedent. The user configures nothing; the socket's owner-only `0600`
boundary stays the real perimeter, and the fail-closed rule (explicit op,
never `None`-means-all) is preserved.

Distribution ships the standard three files: `aegis.portal` under
`/usr/share/xdg-desktop-portal/portals/`, `aegis-portals.conf` (preferring
`aegis;gtk` so UI-driven portals fall back to the GTK backend) under
`/usr/share/xdg-desktop-portal/`, and a D-Bus activation file under
`/usr/share/dbus-1/services/`.

**Roadmap.** Phase 1 is Settings + non-interactive Screenshot. Phase 2 adds
ScreenCast streaming, reusing the same readback path with continuous
frame/damage delivery (ADR-0041 already notes streaming reuses it). Phase 3
adds Background, Inhibit, and an interactive Screenshot dialog, presumably
through the control-center chrome.

**Phase 3A (landed, [ADR-0053](0053-portal-session-services-and-grants.md)).**
The non-interactive half of Phase 3: `Background` v1 and `Inhibit` v1
(idle flag only, over a new scoped `SetIdleInhibit` IPC op), ScreenCast v2
(`persist_mode` / `restore_token`), and `SettingChanged` emission from a
config-file mtime watcher — the Settings bullet above is superseded on
that point. The interactive halves (screenshot dialog, source selection)
remain open.

## Alternatives

- **Wayland capture protocols (`wlr-screencopy`, ext-image-copy) for portal
  use.** Rejected again, per ADR-0037: any client that can bind the global
  bypasses the named-scope model. The portal backend gets pixels through the
  same scoped IPC as every other trusted client.
- **Portal interfaces served in-process by the compositor.** Rejected: it
  would link zbus and a D-Bus dispatch loop into the compositor's process,
  against ADR-0027's rejection of D-Bus as the compositor's own transport,
  and a bus-level fault would threaten the display server.
- **`xdg-desktop-portal-gtk` (or -wlr) as the whole backend.** Rejected as
  the screenshot path: the GTK backend cannot capture pixels from a
  compositor that exposes no capture global, and shipping no backend breaks
  portal resolution for sandboxed apps. The GTK backend remains the
  configured fallback for UI-driven interfaces (`aegis-portals.conf`).
- **A user-configured `[[agent.scope]]` for the portal instead of a built-in
  scope.** Rejected: the portal backend is session infrastructure, not a
  third-party agent; requiring every user to hand-author its scope would make
  a stock install silently fail screenshots. The built-in scope is the
  narrowest grant that works out of the box.

## Consequences

- `crates/aegis-portal` is flux-free and joins the CI clippy/doc/test
  allowlists; zbus stays confined to bridge-style crates (statusbar, portal).
- `aegis-ipc` gains `LOCAL_PORTAL_SCOPE`; the compositor registers the built-in
  scope alongside `aegis-ctl-realm-admin`, so every `CaptureOutput` from
  the portal is journaled and revocable by the same machinery as any scoped
  client.
- `aegis-config` gains `[appearance] color_scheme`, documented in
  `docs/reference/config.md`; it is the first config key consumed by a
  satellite process rather than the compositor itself.
- Portal screenshots are cache files, not user pictures: they go to the
  portal cache directory, separate from `[screenshot] save_dir`, and the
  owning application is expected to move them.
- Phase 2 (ScreenCast) needs a streaming transport decision the current
  one-shot memfd does not answer; that is a new ADR when the work starts.
