# ADR-0050: fuji agent product and the ass-fuji bridge rename (amends ADR-0047)

- Status: Accepted
- Date: 2026-07-23

## Context

[ADR-0047](0047-neenee-agent-realm-platform-bridge.md) split the agent stack
across three repositories: `ass-neenee` in this workspace as the scoped MCP
platform bridge, Neenee as the sibling agent product, and Praxion as the
reusable runtime beneath Neenee. That split kept the compositor model-free,
but it made the agent's smallest useful unit span three checkouts and two
products, and it kept a source dependency on `../praxion` in the product's
history.

The product that drives ASS is being renamed to fuji (宓姬, after Lady Fu
(宓妃) of the *Luoshen Fu*). The rename is an opportunity to consolidate:
the bridge and the agent runtime live together in this workspace as one
self-contained project with no Praxion or Neenee dependency, while the
ADR-0047 boundary — an out-of-process, named-scope MCP bridge, and a
model-free compositor — stays exactly as decided.

## Decision

Rename `ass-neenee` to `ass-fuji`. The bridge keeps its architecture: the
`ass-fuji-mcp` binary serves newline-delimited MCP over stdio, requests a
configured named scope (default `fuji`), owns one bridge-managed Agent Realm,
and depends only on the public ASS model, catalog, and IPC crates. Its
user-facing surface is renamed consistently: `ASS_FUJI_*` environment
variables, the `fuji` default scope, the `Fuji` default Realm label, and the
`$XDG_RUNTIME_DIR/ass-fuji/` state directory. The `realm_transfer_window`
tool's `target` value `neenee` becomes `fuji`; every other MCP tool name and
schema is unchanged.

Grow `ass-fuji` into the complete fuji product: alongside the bridge, the
same crate carries fuji's own agent runtime. It owns provider credentials
and policy (Anthropic and OpenAI-compatible streaming providers), the agent
loop, built-in file/shell/image tools, an stdio MCP client, sessions,
skills, and permissions, and ships the `fuji` binary next to
`ass-fuji-mcp`. The runtime uses no Praxion, no Neenee, and — by module
discipline inside the crate — no `ass-*` import: it reaches ASS exclusively
as an MCP client of the bridge, the same integration seam every other MCP
client uses. The compositor never links the crate.

This amends ADR-0047 only in product ownership and naming: the agent product
and its runtime move into the ASS tree, self-contained. The out-of-process
bridge, the scoped-authority model, and the model-free compositor remain in
force.

## Alternatives

- **Keep Neenee + Praxion as the agent product.** Rejected because the
  rename would still leave the smallest agent deployment spanning three
  repositories, and the `../praxion` source dependency the rename is meant
  to retire.
- **Move the bridge out of the ASS tree into a standalone fuji project.**
  Rejected for the same reason ADR-0047 rejected it: the bridge encodes ASS
  platform policy (named scopes, windows, Realms) and should ship with the
  platform it adapts.
- **Link the agent runtime into the compositor or into the bridge
  process.** Rejected because network, credentials, prompts, and model
  cadence must stay out of the stability-sensitive compositor, and the
  bridge stays a lean adapter spawnable by any MCP client.
- **Depend on Neenee's rewritten provider crates.** Rejected because it
  recreates the cross-repository coupling this decision removes.
- **Keep bridge and runtime in separate crates.** Rejected because fuji is
  purpose-built for ASS rather than a reusable agent substrate. One crate
  keeps the product's build, review, and release in one place. The MCP
  process boundary between the two binaries is preserved, so a split would
  have bought only a cargo-enforced `ass-*` import ban at the price of a
  heavier-by-default bridge or feature-gated CI.

## Consequences

- One checkout builds the complete agent: `cargo build -p ass-fuji`
  produces the bridge and the `fuji` CLI. No other repository is required.
- Existing deployments must rename the binary (`ass-neenee-mcp` →
  `ass-fuji-mcp`), the `ASS_NEENEE_*` variables (`ASS_FUJI_*`), the scope
  (`neenee` → `fuji`), and the Realm label (`Neenee` → `Fuji`); old Realm
  recovery records under `$XDG_RUNTIME_DIR/ass-neenee/` do not migrate.
- The bridge stays spawnable by third-party MCP clients; only the default
  scope, label, and the `realm_transfer_window` target value change for
  them.
- The agent runtime evolves inside the ASS workspace, so provider, session,
  and skill changes share the bridge's review and release cycle. Generic
  model-tool mechanisms no longer have a separate substrate to go to;
  mechanisms land in the crate's agent modules or not at all.
- Neenee and Praxion remain independent sibling projects; nothing in this
  decision constrains them.
