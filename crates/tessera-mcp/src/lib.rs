//! tessera-mcp — the Tessera platform's MCP bridge.
//!
//! This crate is deliberately not an agent runtime. It is the independently
//! launched MCP adapter that pairs for a capability ceiling (ADR-0088) and
//! translates agent tool calls into the public Tessera IPC contract, and it is
//! the standard access point for any agent product, first- or third-party,
//! operating the desktop. Agent products reach the desktop only as MCP
//! clients of this bridge, never through `tessera_*` imports (ADR-0047,
//! ADR-0087).

mod config;
mod mcp;
mod tools;

pub use config::{BridgeConfig, ConfigError};
pub use mcp::{McpError, serve, serve_config};
pub use tools::{
    TesseraPlatform, PlatformError, SmokeInteractionDomainReport, SmokeNotificationReport,
    SmokeReport, SmokeVisualReport, ToolGrant,
};
