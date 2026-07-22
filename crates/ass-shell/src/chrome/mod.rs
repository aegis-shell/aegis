//! Chrome components built on the [`crate::Chrome`] seam.
//!
//! Each module is one piece of compositor chrome: it reads the shared window
//! snapshot and input, draws itself through a `lens` frame, and pushes user
//! intents into [`crate::ChromeEvents`]. The core [`crate::Shell`] host is
//! unaware of these; the binary registers whichever set it wants.
//!
//! Current components:
//!
//! - [`AgentFeedback`] — trusted, non-interactive visual feedback for input
//!   applied by an Agent Realm's independent seat.
//! - [`Decorations`] — per-window server-side title bars drawn as `lens`
//!   overlays, with click-to-move and a close gadget.
//! - [`Launcher`] — a top-center toggle that expands into a centered list of
//!   every enumerated `.desktop` entry; click a row to launch it (ADR-0022).
//!
//! Larger components have graduated to their own crates on top of the same
//! contract (ADR-0021): the dock lives in `ass-dock`, the Control Center in
//! `ass-control-center`, and the status bar in `ass-statusbar`.

mod agent_feedback;
mod app_menu;
mod decorations;
mod launcher;
mod overview;
mod screenshot;
mod toast;

pub use agent_feedback::AgentFeedback;
pub use app_menu::{AppMenu, PinAction};
pub use decorations::Decorations;
pub use launcher::Launcher;
pub use overview::Overview;
pub use screenshot::ScreenshotSelector;
pub use toast::Toast;
