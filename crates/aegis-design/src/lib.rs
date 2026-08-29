//! Product-specific design policy for aegis chrome.
//!
//! This crate is deliberately data-only: it builds semantic lens themes and
//! material option values, but never receives a `Frame` or `Input`, retains UI
//! state, or emits application intents. Generic UI behavior belongs in lens;
//! component behavior stays in the owning aegis chrome crate.

#![forbid(unsafe_code)]

pub mod colors;
pub mod materials;
pub mod themes;
pub mod tokens;

pub use colors::{CommandPanelColors, DockColors, ProductColors, SceneColors};
pub use tokens::{AvatarRole, Design, GlassRole, PreviewSelectionStyle};
