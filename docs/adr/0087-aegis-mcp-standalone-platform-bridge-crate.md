# ADR-0087: aegis-mcp as the standalone platform bridge crate (amends ADR-0050 and ADR-0066)

- Status: Accepted
- Date: 2026-07-30

## Context

[ADR-0050](0050-fuji-agent-product-and-bridge-rename.md) consolidated the
scoped MCP platform bridge and the fuji agent runtime into one `aegis-fuji`
crate, rejecting a crate split because fuji was the only agent product and
the two binaries already kept their process boundary. The bridge therefore
shipped inside one agent product's crate even though it encodes Aegis
platform policy — named scopes, the bridge-managed Agent Realm, scoped tool
grants — and [ADR-0047](0047-neenee-agent-realm-platform-bridge.md) had
already ruled that such an adapter belongs with the platform it adapts.

The bridge is now the standard access point for agents operating the
desktop, not a fuji component. fuji consumes it exactly the way any
third-party MCP client does. Keeping the bridge inside `aegis-fuji` forces
every other agent to depend on the fuji product's crate to reach the
platform, and leaves the crate boundary unable to enforce the "no `aegis_*`
imports in the agent runtime" rule that previously stood by module
discipline alone. The project is pre-1.0 and can complete the rename before
these interfaces become stable.

## Decision

Extract the bridge from `aegis-fuji` into a standalone `aegis-mcp` crate.
The crate ships the `aegis-mcp` binary — the scoped Aegis desktop and Agent
Realm MCP bridge — and depends only on the public Aegis model, catalog, and
IPC crates. It is the platform's standard agent access point: any agent
product, fuji or third-party, spawns it as an MCP server and requests a
configured named scope.

The bridge's product-coupled surface becomes neutral:

- Binary: `aegis-fuji-mcp` → `aegis-mcp`; MCP `serverInfo.name` becomes
  `aegis-mcp`.
- Environment variables: `AEGIS_FUJI_*` → `AEGIS_MCP_*`. No compatibility
  aliases, per ADR-0066.
- State directory: `$XDG_RUNTIME_DIR/aegis-fuji/` →
  `$XDG_RUNTIME_DIR/aegis-mcp/`. Realm recovery records do not migrate.
- The `realm_transfer_window` tool's `target` value `fuji` becomes `agent`;
  tool descriptions, notification `app_id`, and error text no longer name a
  product. Every other MCP tool name and schema is unchanged.
- The Rust type `AssPlatform` becomes `AegisPlatform`, retiring the last
  pre-Aegis namespace remnant in active code.

The default scope stays `fuji` and the default Realm label stays `Fuji`:
the out-of-the-box deployment is the in-tree fuji product, and the scope
must match a `[[agent.scope]]` declaration in the compositor configuration.
Genericity comes from `AEGIS_MCP_SCOPE` being settable per spawned bridge,
not from the default value.

`aegis-fuji` keeps only the agent runtime — providers, the agent loop,
built-in tools, the stdio MCP client, sessions, skills, and permissions —
behind the `fuji` binary. Its default MCP server entry spawns `aegis-mcp`
with `AEGIS_MCP_SCOPE=fuji`. The crate boundary now enforces what ADR-0050
kept by discipline: the agent runtime cannot import `aegis_*` crates, and
the compositor never links either crate.

## Alternatives

- **Keep bridge and runtime in one `aegis-fuji` crate.** Rejected because
  the premise of ADR-0050's rejection — fuji as the only consumer — no
  longer holds once the bridge is the platform's standard agent access
  point. A third-party agent should not depend on the fuji product's crate
  to obtain the platform adapter, and a crate boundary enforces the import
  ban that module discipline only requested.
- **Rename the default scope to `aegis`.** Rejected because the scope names
  a user-declared `[[agent.scope]]` entry; changing the default would break
  every existing compositor configuration without making the bridge more
  generic, since the scope is already per-process configurable.
- **Keep `fuji` as the `realm_transfer_window` target value.** Rejected
  because the value denotes the bridge-managed Agent Realm, not the fuji
  product; a product-neutral bridge cannot carry another product's name in
  its public tool schema.
- **Provide compatibility aliases for the old binary and variables.**
  Rejected per ADR-0066: the project is pre-1.0 and the explicit goal is
  one consistent namespace rather than a staged dual-name interface.

## Consequences

- One checkout still builds the complete agent: `cargo build -p aegis-mcp
  -p aegis-fuji` produces the bridge and the `fuji` CLI.
- Existing deployments must rename the binary (`aegis-fuji-mcp` →
  `aegis-mcp`), the environment variables (`AEGIS_FUJI_*` → `AEGIS_MCP_*`),
  and any `realm_transfer_window` callers using `target = "fuji"` (now
  `"agent"`). Old Realm recovery records under
  `$XDG_RUNTIME_DIR/aegis-fuji/` do not migrate.
- The bridge stays spawnable by third-party MCP clients; for them only the
  binary name, the variable prefix, and the transfer target value change.
- The bridge and the agent runtime now have independent review and release
  surfaces: platform policy changes land in `aegis-mcp`, model-side changes
  in `aegis-fuji`.
- This decision amends ADR-0050's one-crate consolidation (the process
  boundary, scoped-authority model, and model-free compositor stand) and
  the naming details in ADR-0066, which the table in that ADR already
  anticipates by making the MCP server and tool prefix `aegis` and
  `mcp__aegis__*`.
