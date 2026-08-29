use anyhow::{Context, Result, bail};
use clap::Parser;
use std::process::Command;

#[derive(Parser, Debug)]
pub struct ClippyArgs {
    /// Automatically apply lint suggestions (`clippy --fix`)
    #[arg(long)]
    pub fix: bool,

    /// Allow dirty working tree when running `clippy --fix`
    #[arg(long)]
    pub allow_dirty: bool,

    /// Specific package to check
    #[arg(short, long)]
    pub package: Option<String>,
}

pub fn run_clippy(args: ClippyArgs) -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut cmd = Command::new(&cargo);
    cmd.arg("clippy");

    if let Some(pkg) = &args.package {
        cmd.args(["--package", pkg]);
    } else {
        cmd.arg("--workspace");
    }

    cmd.arg("--all-targets");

    if args.fix {
        cmd.arg("--fix");
        if args.allow_dirty {
            cmd.arg("--allow-dirty");
        }
    }

    cmd.args(["--", "-D", "warnings"]);

    println!("Running: {:?}", cmd);
    let status = cmd.status().context("Failed to run cargo clippy")?;
    if !status.success() {
        bail!("clippy failed with exit code: {}", status);
    }
    Ok(())
}
