//! aegis-agent — the Aegis agent runtime behind the `aegis-agent` binary.
//! aegis-agent reaches the desktop exclusively as an MCP client of the platform bridge
//! in the `aegis-mcp` crate, never through `aegis_*` imports (ADR-0087, ADR-0089).
//! The agent retains fuji (宓姬) as its internal persona identity.

pub mod agent;
