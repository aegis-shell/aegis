# ADR-0089: aegis-agent Crate, CLI, and Fuji Identity Preservation (amends ADR-0050 & ADR-0066)

- Status: Accepted
- Date: 2026-08-01

## Context

[ADR-0050](0050-fuji-agent-product-and-bridge-rename.md) renamed the agent product and bridge crate to `aegis-fuji` and `fuji`.
[ADR-0066](0066-canonical-aegis-namespace.md) established canonical `Aegis` product naming and `$XDG_CONFIG_HOME/aegis` configuration paths across desktop components.

However, using `fuji` as the name for the agent crate (`aegis-fuji`), CLI binary (`fuji`), environment variables (`FUJI_CONFIG`), and standalone config directory (`$XDG_CONFIG_HOME/fuji`) conflated generic Agent runtime mechanics with the agent's specific internal codename and persona ("fuji" / 宓姬). This created inconsistency with the rest of the workspace and maintained a non-canonical configuration directory outside `$XDG_CONFIG_HOME/aegis/`.

## Decision

1. **Rename Crate & Binary**: Rename `aegis-fuji` crate to `aegis-agent`, and the binary from `fuji` to `aegis-agent`.
2. **Canonical Config & Data Paths**: Move configuration file path from `$XDG_CONFIG_HOME/fuji/config.toml` to `$XDG_CONFIG_HOME/aegis/agent.toml` (override via `AEGIS_AGENT_CONFIG`). Store session files under `$XDG_DATA_HOME/aegis/agent/sessions/`.
3. **Preserve Agent Identity**: Retain `fuji` (宓姬) as the internal agent persona, system prompt identity, and default character codename.
4. **Update Documentation & Tooling**: Rename user-facing docs to `docs/how-to/agent.md` and `docs/reference/agent.md`, and update CI workflows to target `aegis-agent`.

## Alternatives

- **Keep `fuji` as the binary and crate name.** Rejected because it mixes generic runtime mechanisms with specific persona naming and breaks `$XDG_CONFIG_HOME/aegis` configuration scoping.
- **Rename persona to "agent".** Rejected because preserving `fuji` (宓姬) maintains brand identity and prompt consistency while keeping code architecture decoupled.

## Consequences

- Cargo workspace crate becomes `aegis-agent`, building binary `aegis-agent`.
- Configuration files move to `$XDG_CONFIG_HOME/aegis/agent.toml`.
- Internal agent loop and system prompt continue to identify the agent persona as `fuji`.
- Users and CI scripts call `aegis-agent` CLI instead of `fuji`.
