# aegis-agent

`aegis-agent` is the agent runtime for Aegis: the agent CLI (`aegis-agent`) —
streaming providers, the agent loop, built-in tools, an stdio MCP client,
sessions, skills, and permissions. The internal agent persona identity is
fuji (宓姬). It reaches the desktop only as an MCP client of the platform bridge in
the `aegis-mcp` crate; the compositor remains model-free.

```bash
cargo build --locked --release -p aegis-agent
target/release/aegis-agent print-config   # annotated example configuration
target/release/aegis-agent check          # validate config, credentials, MCP
```

The default MCP server entry spawns `aegis-mcp`, which borrows desktop
capabilities through first-run pairing; no Aegis configuration is needed.
See [Connect agent to Aegis](../../docs/how-to/agent.md), the [bridge
reference](../../docs/reference/aegis-mcp.md), and the [agent
reference](../../docs/reference/agent.md).
