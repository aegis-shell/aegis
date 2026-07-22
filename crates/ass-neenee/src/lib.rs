//! ASS platform integration for the Neenee agent product.
//!
//! This crate is deliberately not an agent runtime. Neenee owns product
//! identity, provider policy, conversation state, and Praxion composition.
//! `ass-neenee` is the independently launched, named-scope MCP bridge that
//! translates Neenee tool calls into the public ASS IPC contract.

mod config;
mod mcp;
mod realm;
mod tools;

pub use config::{BridgeConfig, ConfigError};
pub use mcp::{McpError, serve};
pub use tools::{
    AssPlatform, PlatformError, SmokeNotificationReport, SmokeRealmReport, SmokeReport,
    SmokeVisualReport, ToolGrant,
};
