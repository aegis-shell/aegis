# tessera-mcp

`tessera-mcp` is the Tessera platform's MCP bridge: the standard access point
for any agent operating the desktop. It exposes scoped desktop and Agent
Interaction Domain tools over stateless stdio MCP `2026-07-28` for tessera-agent and any
other current MCP client, owns one subject-bound Agent Interaction Domain per connector
instance, and depends only on the public Tessera model, catalog, IPC, and
rich client library (`tessera-ipc-client`) crates.
The compositor never links it and remains model-free.

```bash
cargo build --locked --release -p tessera-mcp
target/release/tessera-mcp check          # probe the compositor-granted scope
target/release/tessera-mcp smoke          # live, reversible Interaction Domain smoke test
target/release/tessera-mcp print-config   # MCP client config entry
```

The bridge is a protocol edge over the compositor's native capability
broker. It borrows capabilities through first-run pairing, persists an
instance-partitioned credential, and prompts again only for sensitive
operations on first use. Semantic observation is separate from action:
`interaction_domain_observe` returns a single-use state precondition and
`interaction_domain_input`
returns only after the compositor commits or refuses the bounded action. See the
[tessera-mcp Bridge Reference](../../docs/reference/tessera-mcp.md) for the full
command, environment, pairing, and tool contract.
