//! fuji's agent runtime: providers, the agent loop, tools, MCP client,
//! sessions, skills, and permissions.
//!
//! This module tree deliberately never imports `aegis_*` crates: fuji reaches
//! the desktop only as an MCP client of the platform bridge in the
//! `aegis-mcp` crate. The crate boundary now enforces the rule that
//! ADR-0031/0047 established: every agent is an ordinary scoped IPC client.

pub mod config;
pub mod mcp_client;
pub mod permissions;
pub mod provider;
pub mod runtime;
pub mod session;
pub mod skills;
pub mod tools;

pub use runtime::{Agent, AgentError, AgentEvent, TurnOutcome};
