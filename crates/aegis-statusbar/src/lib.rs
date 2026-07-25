//! Status bar chrome component for the ass compositor.
//!
//! A compact top bar with integrated workspace state, active-window context,
//! clock, application tray, and system status, built on the [`aegis_shell`]
//! `Chrome` contract. The component owns presentation and interaction state
//! only; window, workspace, notification, and system snapshots arrive through
//! the shell each frame, and user intents leave through
//! `aegis_shell::ChromeEvents`.
//!
//! Like the dock (`ass-dock`) and the Control Center (`aegis-ctl-center`),
//! the status bar graduated out of `ass-shell` into its own crate on top of
//! the same component seam (ADR-0021), following the ADR-0044 precedent;
//! ADR-0045 will cover this split. The composition root (the `ass` binary)
//! registers it conditionally from the `[statusbar]` configuration.
//!
//! The crate also hosts a StatusNotifierItem system tray ([`tray`]): the
//! compositor runs as the session's StatusNotifierWatcher + Host on a pair of
//! worker threads and renders the registered items' icons in the bar's tray
//! row.

mod bar;
pub mod tray;

pub use bar::StatusBar;
