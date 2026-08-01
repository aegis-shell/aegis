//! Typed command errors and exit-code mapping.
//!
//! Replaces the previous `Result<_, String>` surface with a structured enum.
//! The dispatcher returns [`CliError`]; the binary maps each variant to a
//! stable process exit code (0 success, 1 runtime, 2 usage).

/// All failures surfaced through native `aegis` domain commands.
///
/// `clap::Error` is wrapped verbatim because clap already renders an
/// appropriate `error: ... \n\nUsage: ...` message; we just need to forward
/// its choice of stdout vs stderr and exit code.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Argument parsing or unknown subcommand. Exit code 2.
    #[error("{0}")]
    Usage(#[from] clap::Error),
    /// Could not open the IPC socket or negotiate capabilities.
    #[error("connect: {0}")]
    Connect(String),
    /// IPC read/write failure or server-side protocol error.
    #[error("{0}")]
    Io(String),
    /// Filesystem error writing a capture or creating a directory.
    #[error("{0}")]
    Fs(String),
    /// A Realm id of `0` was supplied; the protocol reserves it.
    #[error("Realm id zero is invalid")]
    ZeroRealmId,
    /// A Realm argument referenced an unknown id in a fallback slot.
    #[error("fallback Realm {0} is invalid")]
    InvalidFallbackRealm(u64),
}

impl CliError {
    /// Map to the same exit-code contract documented in `docs/reference/cli.md`:
    /// 0 success, 1 any runtime failure, 2 usage / argument error.
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Usage(_) => 2,
            _ => 1,
        }
    }
}
