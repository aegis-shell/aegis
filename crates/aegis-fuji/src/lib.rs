//! fuji (宓姬, named after Lady Fu of the *Luoshen Fu*) — the Aegis agent
//! product: a self-contained agent runtime behind the `fuji` binary. fuji
//! reaches the desktop only as an MCP client of the platform bridge in the
//! `aegis-mcp` crate, never through `aegis_*` imports (ADR-0087).

pub mod agent;
