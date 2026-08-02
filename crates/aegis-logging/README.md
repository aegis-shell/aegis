# aegis-logging

Shared tracing-based observability init for Aegis processes.

Every first-party binary (`aegis`, `aegis-idle`, `aegis-lock`,
`xdg-desktop-portal-aegis`, `aegis-settings`) calls `aegis_logging::init`
before doing work. Crates keep using the `log` facade; this crate installs a
`tracing`-based subscriber and bridges `log::` records into it, so structured
spans and fields added around `log::` events are captured without rewriting
every callsite.

## Configuration

- `RUST_LOG` — a `tracing_subscriber::EnvFilter` directive, e.g.
  `info,aegis_backend::drm=debug`. Defaults to the filter passed to `init`
  (`info` when `aegis` runs the compositor and `warn` for one-shot management
  commands).
- `AEGIS_LOG_FORMAT=json` — emit machine-readable JSON instead of text.
- `NO_COLOR` — disable ANSI color.

All output is written to stderr so journal capture is uniform across
processes. See ADR-0079 for the seam contract.
