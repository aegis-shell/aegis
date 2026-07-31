# aegis-mcp

`aegis-mcp` is the Aegis platform's MCP bridge: the standard access point
for any agent operating the desktop. It exposes scoped desktop and Agent
Realm tools over stdio MCP for fuji and any other MCP client, owns one
bridge-managed Agent Realm per scope, and depends only on the public Aegis
model, catalog, and IPC crates. The compositor never links it and remains
model-free.

```bash
cargo build --locked --release -p aegis-mcp
target/release/aegis-mcp check          # probe the compositor-granted scope
target/release/aegis-mcp smoke          # live, reversible Realm smoke test
target/release/aegis-mcp print-config   # MCP client config entry
```

The bridge requests a configured named scope (default `desktop-operator`, override with
`AEGIS_MCP_SCOPE`) that must exist in Aegis configuration before the bridge
starts. See the [aegis-mcp Bridge
Reference](../../docs/reference/aegis-mcp.md) for the full command,
environment, scope, and tool contract.
