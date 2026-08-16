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
//!   applied by an Agent Interaction Domain's independent seat.
//! - [`Launcher`] — a top-center toggle that expands into a centered list of
//!   every enumerated `.desktop` entry; click a row to launch it (ADR-0022).
//!
//! Larger components have graduated to their own crates on top of the same
//! contract (ADR-0021): the dock lives in `aegis-dock`, Prism in
//! `aegis-prism`, the HUD in `aegis-hud`, and the command panel in
//! `aegis-command-panel`.

mod agent_feedback;
mod app_menu;
mod app_picker;
mod battery_alert;
mod capability_prompt;
mod confirm_prompt;
mod controlled_window_guard;
mod launcher;
mod overview;
mod recording_indicator;
mod screenshot;
mod secret_prompt;
mod toast;
mod window_switcher;

pub use agent_feedback::AgentFeedback;
pub use app_menu::{AppMenu, PinAction};
pub use app_picker::{AppPickParams, AppPicker};
pub use battery_alert::{BatteryAlert, BatteryAlertParams};
pub use capability_prompt::{
    CapabilityFamily, CapabilityGroup, CapabilityPickParams, CapabilityPickResult, CapabilityPrompt,
};
pub use confirm_prompt::{ConfirmAnswer, ConfirmPickParams, ConfirmPickStyle, ConfirmPrompt};
pub use controlled_window_guard::ControlledWindowGuard;
pub use launcher::Launcher;
pub use overview::Overview;
pub use recording_indicator::RecordingIndicator;
pub use screenshot::{PickerMode, ScreenshotSelector};
pub use secret_prompt::{SecretPrompt, SecretPromptParams};
pub use toast::Toast;
pub use window_switcher::WindowSwitcher;
