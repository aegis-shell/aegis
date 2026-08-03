//! HUD status component for the aegis compositor (ADR-0080, ADR-0083).
//!
//! Two floating, display-only status chips in minimal FPS-HUD style:
//! system status (network, Bluetooth, battery), the StatusNotifierItem
//! tray row, the clock, and the notification count on the left; workspace
//! position markers in the center. The top-right belongs to the frameless notification
//! toast strip, and the Agent Workspaces status lives in the command panel
//! (ADR-0083). The chips
//! reserve no space, accept no pointer input, and fade out when the cursor
//! approaches; window, workspace, notification, and system snapshots arrive
//! through the shell each frame. Every interaction the old status bar
//! hosted (quick settings, tray activation, notification dismissal) lives
//! in the command panel (`aegis-command-panel`).
//!
//! Like the dock (`aegis-dock`) and the modal compositor applications,
//! the HUD graduated out of `aegis-shell` into its own crate on top of
//! the same component seam (ADR-0021), following the ADR-0044 precedent.
//! The composition root (the `aegis` binary)
//! registers it conditionally from the `[hud]` configuration.
//!
//! The StatusNotifierItem system tray lives in the shared [`aegis_tray`]
//! crate (re-exported here as [`tray`]); the composition root spawns it once
//! and shares the handle with the command panel.

mod hud;

pub use aegis_tray as tray;
pub use hud::Hud;
