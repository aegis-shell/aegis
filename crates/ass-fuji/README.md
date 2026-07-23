# ass-fuji

`ass-fuji` is the fuji (宓姬, named after Lady Fu of the *Luoshen Fu*) agent
product for ASS: one crate, two binaries.

- `ass-fuji-mcp` — the scoped ASS platform bridge. It exposes desktop and
  Agent Realm tools over stdio MCP for fuji and any other MCP client.
- `fuji` — fuji's own agent runtime: streaming providers, the agent loop,
  built-in tools, an stdio MCP client, sessions, skills, and permissions.
  It reaches the desktop only as an MCP client of the bridge; the
  compositor remains model-free.

```bash
cargo build --release -p ass-fuji
target/release/fuji print-config   # annotated example configuration
target/release/fuji check          # validate config, credentials, MCP
target/release/ass-fuji-mcp check  # probe the compositor-granted scope
```

The default IPC scope is `fuji`. It must exist in ASS configuration before
the MCP process starts. See [Connect fuji to
ASS](../../docs/how-to/fuji.md), the [bridge
reference](../../docs/reference/fuji.md), and the [agent
reference](../../docs/reference/fuji-agent.md).
