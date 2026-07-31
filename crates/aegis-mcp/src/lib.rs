//! aegis-mcp — the Aegis platform's MCP bridge.
//!
//! This crate is deliberately not an agent runtime. It is the independently
//! launched, named-scope MCP adapter that translates agent tool calls into
//! the public Aegis IPC contract, and it is the standard access point for
//! any agent product — fuji or third-party — operating the desktop. Agent
//! products reach the desktop only as MCP clients of this bridge, never
//! through `aegis_*` imports (ADR-0047, ADR-0050, ADR-0087).

mod config;
mod mcp;
mod realm;
mod tools;

pub use config::{BridgeConfig, ConfigError};
pub use mcp::{McpError, serve};
pub use tools::{
    AegisPlatform, PlatformError, SmokeNotificationReport, SmokeRealmReport, SmokeReport,
    SmokeVisualReport, ToolGrant,
};
