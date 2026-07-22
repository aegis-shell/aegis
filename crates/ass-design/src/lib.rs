//! Product-specific design policy for ass chrome.
//!
//! This crate is deliberately data-only: it builds semantic lens themes and
//! material option values, but never receives a `Frame` or `Input`, retains UI
//! state, or emits application intents. Generic UI behavior belongs in lens;
//! component behavior stays in the owning ass chrome crate.

#![forbid(unsafe_code)]

pub mod materials;
pub mod themes;
pub mod tokens;

pub use tokens::Design;
