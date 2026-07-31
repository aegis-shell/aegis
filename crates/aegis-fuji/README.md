# aegis-fuji

`aegis-fuji` is the fuji (宓姬, named after Lady Fu of the *Luoshen Fu*)
agent product for Aegis: fuji's own agent runtime behind the `fuji` binary —
streaming providers, the agent loop, built-in tools, an stdio MCP client,
sessions, skills, and permissions. It reaches the desktop only as an MCP
client of the platform bridge in the `aegis-mcp` crate; the compositor
remains model-free.

```bash
cargo build --locked --release -p aegis-fuji
target/release/fuji print-config   # annotated example configuration
target/release/fuji check          # validate config, credentials, MCP
```

The default MCP server entry spawns `aegis-mcp` with `AEGIS_MCP_SCOPE=desktop-operator`;
the `desktop-operator` scope must exist in Aegis configuration before the bridge starts.
See [Connect fuji to Aegis](../../docs/how-to/fuji.md), the [bridge
reference](../../docs/reference/aegis-mcp.md), and the [agent
reference](../../docs/reference/fuji-agent.md).
