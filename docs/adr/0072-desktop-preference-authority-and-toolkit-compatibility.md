# ADR-0072: Desktop preference authority and toolkit compatibility

- Status: Accepted
- Date: 2026-07-29

## Context

Aegis is an independent compositor and desktop session. Applications still
arrive through several integration paths: sandboxed applications read the
freedesktop Settings portal, GTK applications may request selected
`org.gnome.desktop.interface` keys through that portal, and compositor chrome
needs the same icon, cursor, and motion preferences.

GNOME stores desktop policy in GSettings/dconf. KDE and Qt use different
configuration stores and platform integration. Treating any one of those
stores as Aegis policy would create an undeclared dependency on another
desktop and could produce different values for chrome, portal clients, and
toolkit clients. Writing several foreign stores would introduce
bidirectional synchronization, conflict resolution, and version-dependent
schemas.

The Settings portal is a projection of desktop-owned preferences, not a
general settings database. A portal frontend selects one Settings backend for
the interface, so unsupported keys cannot reliably fall through to a second
backend. Aegis therefore needs one authoritative internal model and an
explicit compatibility boundary.

## Decision

Aegis owns one **desktop preference authority**:

1. The versioned Aegis TOML configuration is the persistent source. System
   Settings submits complete, revisioned desktop-preference transactions to
   the compositor; it does not write TOML or toolkit stores directly.
2. The compositor resolves configuration into one concrete
   `DesktopPreferences` snapshot. It applies the snapshot to chrome and
   publishes the same snapshot over IPC. IPC protocol version 10 carries the
   profile and its complete settings action.
3. Explicit process-start overrides remain compatibility inputs only:
   `AEGIS_ICON_THEME`, `XCURSOR_THEME`, and `XCURSOR_SIZE` override their
   corresponding configured values. They are resolved once in the compositor
   pipeline and appear in the effective IPC snapshot. No consumer reads them
   independently. This narrowly amends ADR-0026's environment-variable rule;
   the TOML file remains the only persistent and editable source.
4. `aegis-portal` is a read-only bridge over the effective IPC snapshot. It
   does not read configuration files, dconf, GSettings, or KDE configuration.
   It subscribes to `SettingsChanged`, re-queries the snapshot, and emits one
   `SettingChanged` signal for each affected exported key.
5. The portal backend exports the standardized
   `org.freedesktop.appearance` keys `color-scheme`, `accent-color`,
   `contrast`, and `reduced-motion`. It also exports a curated
   `org.gnome.desktop.interface` compatibility projection for color scheme,
   interface fonts, text scale, icon theme, cursor theme and size, and the
   inverse animation switch. Unknown keys return
   `org.freedesktop.portal.Error.NotFound`.
6. The backend remains `org.freedesktop.impl.portal.Settings` version 1 with
   `Read` and `ReadAll`. `ReadOne` and public interface version 2 belong to
   the `org.freedesktop.portal.Settings` frontend, not to its backend.
7. Aegis does not define a cross-desktop general-purpose settings service.
   Application-owned preferences remain application-owned, and system-owned
   state such as audio or network remains behind its system service. New
   desktop-wide preferences join `DesktopPreferences` only when Aegis can
   define their ownership, persistence, validation, live application, and
   compatibility projection.

The deterministic built-in profile uses no color-scheme or accent
preference, normal contrast, full motion, `Sans 10`, `Monospace 10`, text
scale `1.0`, `hicolor` icons, the `default` cursor theme, and a 24-pixel
cursor.

## Alternatives

- **Adopt GNOME GSettings as the Aegis authority.** Rejected because it makes
  an independent session depend on GNOME schemas and dconf and does not solve
  KDE/Qt integration.
- **Synchronize Aegis, GNOME, and KDE stores.** Rejected because ownership and
  precedence become ambiguous, external edits can loop, and schema changes in
  another desktop become Aegis compatibility breaks.
- **Expose only the standardized appearance namespace.** Rejected because a
  Settings backend is selected at interface granularity and GTK applications
  still request a small set of interface preferences through the portal.
- **Proxy every GNOME interface key.** Rejected because it would make a GNOME
  schema an accidental Aegis public API. The compatibility namespace remains
  intentionally curated.
- **Let chrome, cursor rendering, and the portal resolve their own inputs.**
  Rejected because the same session could advertise and render different
  preferences.
- **Create an Aegis-specific D-Bus settings database.** Rejected because the
  typed configuration and IPC transaction already provide persistence and
  authority, while applications need standard portal and toolkit projections.

## Consequences

- The Appearance page becomes an available, explicit-apply System Settings
  module. One transaction updates color, accessibility, typography, icon, and
  cursor preferences atomically.
- The portal no longer races compositor reloads or duplicates configuration
  parsing. Its initial query is bounded, and its subscription reconnects
  after compositor restarts.
- GTK compatibility is read-only and intentionally incomplete. Adding a key
  requires an Aegis-owned field and tests for its type and change projection;
  it is not copied merely because a GNOME schema contains it.
- Applications receive one coherent effective profile, including explicit
  startup overrides. A complete-profile transaction does not copy overridden
  fields back into TOML; remove the override and relaunch before editing its
  persistent value.
- IPC clients must use protocol version 10.
- The Settings portions of the proposed ADR-0051 and ADR-0053 implementation
  descriptions are replaced by this decision. Their portal process and
  session-service boundaries remain unchanged.
