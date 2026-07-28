//! Aegis platform integration for the fuji (宓姬) agent.
//!
//! This module is deliberately not an agent runtime. The [`crate::agent`]
//! module owns product identity, provider policy, and conversation state.
//! `bridge` is the independently launched, named-scope MCP adapter that
//! translates agent tool calls into the public Aegis IPC contract.

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
