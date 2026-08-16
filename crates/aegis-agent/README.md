# aegis-agent

`aegis-agent` is the shared agent client layer for the Aegis platform
(ADR-0125). Every out-of-process agent that operates the desktop performs
the same client discipline, and this crate owns it so each agent surface
keeps only its own protocol translation:

- pairing handshake discipline: a generous handshake timeout for the
  interactive pairing prompt, then a short per-request I/O timeout;
- credential policies: a durable, atomically persisted pairing identity
  (`IdentityStore`) and launcher-injected ephemeral credentials
  (`read_credential_from_stdin`);
- per-instance state recovery: a `flock`-held recovery lock and the
  managed Agent Interaction Domain record (`InteractionDomainSession`),
  with crash recovery proven by the authenticated principal subject;
- connection-bound observation lease retention (`ObservationLeases`).

It is a client library, not a daemon: consumers keep their own processes,
supervision, and reconnection policies. Current consumers are `aegis-mcp`
(the MCP bridge) and `aegis-atspi` (the accessibility adapter). The
compositor never links it.
