//! fuji (宓姬, named after Lady Fu of the *Luoshen Fu*) — the Aegis agent
//! product in one crate: [`bridge`] is the scoped MCP platform adapter
//! behind the `aegis-fuji-mcp` binary; [`agent`] is fuji's self-contained
//! agent runtime behind the `fuji` binary. The two halves ship together
//! but keep their process boundary: `fuji` reaches the desktop only as an
//! MCP client of the bridge, never through `aegis_*` imports.

pub mod agent;
pub mod bridge;
