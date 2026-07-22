# ass-neenee

`ass-neenee` is the scoped ASS platform adapter for the Neenee agent product.
The `ass-neenee-mcp` binary exposes desktop and Agent Realm tools over stdio
MCP. Neenee keeps provider credentials, sessions, permissions, skills, and the
Praxion runtime; the compositor remains model-free.

```bash
cargo build --release -p ass-neenee
target/release/ass-neenee-mcp check
target/release/ass-neenee-mcp print-config
```

The default IPC scope is `neenee`. It must exist in ASS configuration before
the MCP process starts. See [Connect Neenee to
ASS](../../docs/how-to/neenee.md) and the [integration
reference](../../docs/reference/neenee.md).

