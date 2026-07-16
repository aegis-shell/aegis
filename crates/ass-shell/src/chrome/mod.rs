//! Chrome components built on the [`crate::Chrome`] seam.
//!
//! Each module is one piece of compositor chrome: it reads the shared window
//! snapshot and input, draws itself through a `lens` frame, and pushes user
//! intents into [`crate::ChromeEvents`]. The core [`crate::Shell`] host is
//! unaware of these; the binary registers whichever set it wants.
//!
//! Current components:
//!
//! - [`Decorations`] — per-window server-side title bars drawn as `lens`
//!   overlays, with click-to-move and a close gadget.
//! - [`Dock`] — a macOS-style bottom-center dock showing a persistent strip of
//!   pinned `.desktop` app icons (plus running windows folded in); click a tile
//!   to focus its window or launch the app, running apps marked with a dot.
//! - [`Launcher`] — a top-center toggle that expands into a centered list of
//!   every enumerated `.desktop` entry; click a row to launch it (ADR-0022).

mod app_menu;
mod control_center;
mod decorations;
mod dock;
mod launcher;
mod toast;
mod workspace_bar;

pub use control_center::ControlCenter;
pub use decorations::Decorations;
pub use dock::{Dock, DockApp};
pub use launcher::Launcher;
pub use toast::Toast;
pub use workspace_bar::{HudBar, WorkspaceBar};
