//! Transport-neutral security policy and audit mechanisms for Aegis.
//!
//! Authority policy and audit persistence remain separate modules so callers
//! can name the exact mechanism they use. The crate contains no IPC framing,
//! Wayland dispatch, rendering, or agent-runtime behavior.

pub mod audit;
pub mod authority;
