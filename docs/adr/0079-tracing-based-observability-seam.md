# ADR-0079: Tracing-based observability seam

- Status: Accepted
- Date: 2026-07-30

## Context

Aegis had grown to six first-party processes (`aegis`, `aegis-cli`,
`aegis-idle`, `aegis-lock`, `aegis-portal`, `aegis-settings`). Observability
was inconsistent and weak:

- The compositor initialized `env_logger` with a custom `info` default and
  second-granularity timestamps; `aegis-idle`, `aegis-lock`, and
  `aegis-portal` called bare `env_logger::init()` (defaulting to `error`); and
  `aegis-cli` and `aegis-settings` installed no subscriber at all. Bring-up of
  the supervised session clients was therefore invisible by default.
- Every record was unstructured text. Concurrent frame, capture, IPC, realm,
  and session-lock work could not be correlated, which made the timing and
  recovery bugs addressed in ADR-0078 harder to diagnose than they should have
  been.
- Log levels were inverted: a default `info` run was noisy with per-request
  D-Bus traffic, while genuine exceptions were buried.

The intended seam — "the facade in every crate, the impl only in the binary"
(documented at the top of the workspace `Cargo.toml`) was sound; only the
binary-side impl and the level discipline were missing.

## Decision

Keep the `log` facade in crates and move the binary-side impl to a
`tracing`-based subscriber, installed once per process through a shared
`aegis-logging` crate:

- `aegis-logging::init` installs a `tracing_subscriber` registry with an
  `EnvFilter`, a human-readable `fmt` layer (or JSON when
  `AEGIS_LOG_FORMAT=json`), and the `tracing-log` `LogTracer` bridge so
  existing `log::` records are captured with span context. All output goes to
  stderr for uniform journal capture.
- Filtering honors `RUST_LOG` (an `EnvFilter` directive such as
  `info,aegis_backend::drm=debug`). Each binary passes a sensible default:
  `info` for services, `warn` for the one-shot `aegis-cli` client.
- Crates keep using `log::{error,warn,info,debug}` for events. Structured
  spans are introduced with the `tracing` crate only where request or
  lifecycle correlation is worth it (for example the live-system IPC apply
  path), so the migration is incremental rather than a rewrite of every
  callsite.

Level discipline follows the standard pyramid: `error` for things that need
attention, `warn` for unexpected but recoverable conditions, `info` for
process and session lifecycle, and `debug` for per-request flow and routine
recovery.

## Alternatives

- **Keep `env_logger`.** Rejected: it offers no spans or structured fields, no
  JSON output, and the per-binary init drift was the original symptom.
- **Rewrite every `log::` callsite to `tracing::` macros.** Rejected as the
  default path: it is high-churn, merge-conflict-prone work whose value is in
  spans and fields, not in which macro namespace a leaf event uses. The
  `log`-to-`tracing` bridge captures existing events into the new subscriber,
  so the migration proceeds call-site by call-site as code is touched.
- **A subscriber per crate.** Rejected: a subscriber is process-global and can
  only be installed once, so it belongs in the binary root, not in libraries.

## Consequences

- One init call per process gives uniform filtering, format, and identity.
  The AI-adaptation phase can later attach another `tracing` layer (for
  example to subscribe to compositor events) without changing the facade.
- JSON output is available for aggregation without a rebuild via
  `AEGIS_LOG_FORMAT=json`.
- Level hygiene is an ongoing discipline; the convention is recorded in
  `docs/dev/observability.md`. Per-request and routine-recovery traffic that
  previously ran at `info`/`warn` is downgraded to `debug` as it is
  identified.

See [Comparative Survey](../explanation/comparative-survey.md) for how the
seam relates to other compositors and [Nested Backend
Development](../dev/nested-backend.md) for the `RUST_LOG` workflow.
