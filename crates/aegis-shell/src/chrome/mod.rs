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
//! - [`Launcher`] — a top-center toggle that expands into a centered list of
//!   every enumerated `.desktop` entry; click a row to launch it (ADR-0022).
//!
//! Larger components have graduated to their own crates on top of the same
//! contract (ADR-0021): the dock lives in `aegis-dock`, AI Workspaces in
//! `aegis-ai-workspaces`, and status plus immediate system controls in
//! `aegis-statusbar`.

mod agent_feedback;
mod app_menu;
mod launcher;
mod overview;
mod screenshot;
mod toast;
mod window_switcher;

pub use agent_feedback::AgentFeedback;
pub use app_menu::{AppMenu, PinAction};
pub use launcher::Launcher;
pub use overview::Overview;
pub use screenshot::{PickerMode, ScreenshotSelector};
pub use toast::Toast;
pub use window_switcher::WindowSwitcher;
