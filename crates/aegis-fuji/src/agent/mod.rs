//! fuji's agent runtime: providers, the agent loop, tools, MCP client,
//! sessions, skills, and permissions.
//!
//! This module tree deliberately never imports `aegis_*` crates: fuji reaches
//! the desktop only as an MCP client of the bridge in [`crate::bridge`].
//! Both halves share one crate now, so this rule stands by discipline —
//! it preserves the ADR-0031/0047 seam that every agent is an ordinary
//! scoped IPC client.

pub mod config;
pub mod mcp_client;
pub mod permissions;
pub mod provider;
pub mod runtime;
pub mod session;
pub mod skills;
pub mod tools;

pub use runtime::{Agent, AgentError, AgentEvent, TurnOutcome};
