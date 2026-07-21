//! ass-control entry point: parse argv via `clap`, dispatch, and exit with
//! the contract documented in `docs/reference/cli.md` (0 success, 1 runtime
//! failure, 2 usage / argument error).

use std::process::ExitCode;

use ass_control::Cli;

fn main() -> ExitCode {
    use clap::Parser;
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // clap renders its own help/error text and picks stdout vs stderr.
            let use_stderr = error.use_stderr();
            error.print().expect("print clap message");
            return ExitCode::from(if use_stderr { 2 } else { 0 });
        }
    };
    let socket = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => std::path::PathBuf::from(d).join("ass.sock"),
        None => {
            eprintln!("ass-control: $XDG_RUNTIME_DIR is unset; cannot locate ass.sock");
            return ExitCode::from(2);
        }
    };
    match ass_control::run_with(&socket, cli) {
        Ok(output) if !output.is_empty() => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ass-control: {error}");
            ExitCode::from(error.exit_code() as u8)
        }
    }
}
