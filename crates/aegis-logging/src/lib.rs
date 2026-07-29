//! Shared observability init for Aegis processes.
//!
//! Every first-party binary calls [`init`] before doing work. Crates keep
//! using the `log` facade; this module installs a `tracing`-based subscriber
//! and bridges `log::` records into it, so structured spans and fields added
//! around `log::` events are captured without rewriting every callsite. See
//! ADR-0079 for the seam contract.
//!
//! Filtering honors `RUST_LOG` as a `tracing_subscriber::EnvFilter` directive
//! (for example `info,aegis_backend=debug`). `AEGIS_LOG_FORMAT=json` switches
//! the console format to JSON for log aggregation; otherwise logs are written
//! as human-readable text with ANSI color auto-detected from the TTY and
//! `NO_COLOR`. Every process writes to stderr so journal capture is uniform.

use std::env;
use std::io::IsTerminal;

use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Environment variable selecting the console format. `json` selects the
/// machine-readable formatter; any other value (or unset) selects text.
pub const FORMAT_ENV: &str = "AEGIS_LOG_FORMAT";
/// Environment variable disabling ANSI color, matching the `NO_COLOR`
/// convention honored by `env_logger` and most CLI tooling.
pub const NO_COLOR_ENV: &str = "NO_COLOR";

/// Install the shared subscriber for this process.
///
/// `default_filter` is used when `RUST_LOG` is unset; pass `"info"` for
/// long-running services and `"warn"` for one-shot CLI clients. Safe to call
/// once per process; a second call is a no-op (the global subscriber is
/// already installed).
///
/// Ordering matters: the `log` facade is bridged first so records emitted
/// before the subscriber is fully installed are still captured by it.
pub fn init(default_filter: &str) {
    // Bridge the `log` facade into the tracing subscriber so existing
    // log::{info,warn,...} callsites are captured with span context.
    let _ = tracing_log::LogTracer::init();

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_filter));

    let use_json = env::var(FORMAT_ENV)
        .map(|value| value.eq_ignore_ascii_case("json"))
        .unwrap_or(false);
    let ansi = env::var_os(NO_COLOR_ENV).is_none() && std::io::stderr().is_terminal();

    let registry = tracing_subscriber::registry().with(filter);
    if use_json {
        registry
            .with(
                fmt::layer()
                    .json()
                    .with_writer(std::io::stderr)
                    .with_current_span(true)
                    .with_span_list(false),
            )
            .try_init()
            .ok();
    } else {
        registry
            .with(
                fmt::layer()
                    .with_target(true)
                    .with_level(true)
                    .with_ansi(ansi)
                    .with_writer(std::io::stderr),
            )
            .try_init()
            .ok();
    }
}
