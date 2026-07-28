use std::process::ExitCode;
use std::time::Duration;

use aegis_fuji::bridge::{AssPlatform, BridgeConfig};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "aegis-fuji-mcp",
    version,
    about = "Scoped Aegis desktop and Agent Realm MCP bridge for fuji"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Serve MCP over newline-delimited JSON-RPC on stdin/stdout (default).
    Serve,
    /// Probe the configured Aegis scope and print the effective tool grant.
    Check,
    /// Run a live, reversible notification and Agent Realm smoke test.
    Smoke {
        /// Seconds to keep the temporary Realm visible (paused, then active).
        #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u64).range(1..=30))]
        observe_seconds: u64,
        /// Visible human-controlled window to transfer temporarily and move
        /// the Agent pointer over. No click or keyboard input is generated.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        input_window: Option<u64>,
    },
    /// Print a fuji config.toml MCP entry for this executable.
    PrintConfig,
}

fn main() -> ExitCode {
    match Cli::parse().command.unwrap_or(Command::Serve) {
        Command::PrintConfig => print_config(),
        Command::Check => with_platform(|platform| {
            let output = serde_json::json!({
                "grant": platform.grant(),
                "tools": platform.tool_names()
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&output).expect("JSON value")
            );
            Ok::<(), std::convert::Infallible>(())
        }),
        Command::Smoke {
            observe_seconds,
            input_window,
        } => with_platform(|platform| {
            let report = platform.smoke_with_input(
                Duration::from_secs(observe_seconds),
                input_window.map(aegis_core::window::WindowId),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("serializable smoke report")
            );
            Ok::<(), aegis_fuji::bridge::PlatformError>(())
        }),
        Command::Serve => with_platform(aegis_fuji::bridge::serve),
    }
}

fn with_platform<E: std::fmt::Display>(
    run: impl FnOnce(&mut AssPlatform) -> Result<(), E>,
) -> ExitCode {
    let config = match BridgeConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("aegis-fuji-mcp: {error}");
            return ExitCode::from(2);
        }
    };
    let mut platform = match AssPlatform::connect(config) {
        Ok(platform) => platform,
        Err(error) => {
            eprintln!("aegis-fuji-mcp: {error}");
            return ExitCode::FAILURE;
        }
    };
    match run(&mut platform) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("aegis-fuji-mcp: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print_config() -> ExitCode {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("aegis-fuji-mcp: cannot resolve executable path: {error}");
            return ExitCode::FAILURE;
        }
    };
    let command = serde_json::to_string(&executable.to_string_lossy()).expect("string JSON");
    println!("[mcp.aegis]\ncommand = [{command}]\nenabled = true\nread_only = false");
    ExitCode::SUCCESS
}
