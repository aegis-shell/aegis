//! Chrome components built on the [`crate::Chrome`] seam.
//!
//! Each module is one piece of compositor chrome: it reads the shared window
//! snapshot and input, draws itself through a `lens` frame, and pushes user
//! intents into [`crate::ChromeEvents`]. The core [`crate::Shell`] host is
//! unaware of these; the binary registers whichever set it wants.
//!
//! Current components:
//!
//! - [`WindowList`] — the top-left side panel: Quit button plus a per-window
//!   row with click-to-focus and close.
//! - [`Decorations`] — per-window server-side title bars drawn as `lens`
//!   overlays, with click-to-move and a close gadget.
//! - [`Dock`] — a macOS-style bottom-center dock with one icon tile per
//!   mapped toplevel; click to focus, activated window highlighted.
//! - [`Launcher`] — a top-center toggle that expands into a centered list of
//!   every enumerated `.desktop` entry; click a row to launch it (ADR-0022).

mod decorations;
mod dock;
mod launcher;
mod window_list;

pub use decorations::Decorations;
pub use dock::Dock;
pub use launcher::Launcher;
pub use window_list::WindowList;
