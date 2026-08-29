use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use toml::Value;

#[derive(Parser, Debug)]
pub struct OpticsArgs {
    /// Print only the release tag for CI / scripts
    #[arg(long, short)]
    pub tag_only: bool,
}

pub fn run_optics(args: OpticsArgs) -> Result<()> {
    let manifest_path = Path::new("Cargo.toml");
    let content = fs::read_to_string(manifest_path).context("reading Cargo.toml")?;
    let toml: Value = toml::from_str(&content).context("parsing Cargo.toml")?;

    let workspace_deps = toml
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.as_table())
        .context("No [workspace.dependencies] found in Cargo.toml")?;

    let mut tags = HashSet::new();
    let mut optics_pkgs = Vec::new();

    for (name, dep) in workspace_deps {
        if let Some(table) = dep.as_table() {
            let is_optics = table
                .get("git")
                .and_then(|g| g.as_str())
                .is_some_and(|g| g.contains("github.com/ming2k/optics"));

            if is_optics {
                if let Some(tag) = table.get("tag").and_then(|t| t.as_str()) {
                    tags.insert(tag.to_string());
                    optics_pkgs.push((name.clone(), tag.to_string()));
                } else {
                    bail!("Optics dependency '{}' is missing a git tag", name);
                }
            }
        }
    }

    if optics_pkgs.is_empty() {
        bail!("No Optics dependencies found in workspace.dependencies");
    }

    if tags.len() > 1 {
        bail!("Multiple differing Optics tags found: {:?}", tags);
    }

    let resolved_tag = tags.into_iter().next().unwrap();

    if args.tag_only {
        println!("{}", resolved_tag);
    } else {
        println!(
            "Optics release tag: {} (across {} crates: {})",
            resolved_tag,
            optics_pkgs.len(),
            optics_pkgs
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(())
}
