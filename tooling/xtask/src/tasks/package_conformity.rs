use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use toml::Value;

#[derive(Parser, Debug)]
pub struct PackageConformityArgs {
    /// Automatically fix missing workspace lints declarations in member crates
    #[arg(long)]
    pub fix: bool,
}

pub fn run_package_conformity(args: PackageConformityArgs) -> Result<()> {
    let root_manifest_path = Path::new("Cargo.toml");
    if !root_manifest_path.exists() {
        bail!("Must be run from workspace root containing Cargo.toml");
    }

    let mut crates = Vec::new();
    for entry in fs::read_dir("crates").context("reading crates/ directory")? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let manifest_path = path.join("Cargo.toml");
            if manifest_path.exists() {
                crates.push(manifest_path);
            }
        }
    }
    crates.sort();

    let mut non_workspace_dependencies: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut missing_workspace_lints: Vec<PathBuf> = Vec::new();

    for manifest_path in &crates {
        let content = fs::read_to_string(manifest_path)
            .with_context(|| format!("reading manifest at {:?}", manifest_path))?;
        let toml: Value = toml::from_str(&content)
            .with_context(|| format!("parsing manifest at {:?}", manifest_path))?;

        let package_name = toml
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| manifest_path.display().to_string());

        let has_workspace_lints = toml
            .get("lints")
            .and_then(|l| l.get("workspace"))
            .and_then(|w| w.as_bool())
            .unwrap_or(false);

        if !has_workspace_lints {
            missing_workspace_lints.push(manifest_path.clone());
        }

        for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(deps) = toml.get(section).and_then(|d| d.as_table()) {
                for (dep_name, dep_val) in deps {
                    let is_workspace = match dep_val {
                        Value::Table(t) => t
                            .get("workspace")
                            .and_then(|w| w.as_bool())
                            .unwrap_or(false),
                        _ => false,
                    };
                    if !is_workspace {
                        non_workspace_dependencies
                            .entry(dep_name.clone())
                            .or_default()
                            .push(package_name.clone());
                    }
                }
            }
        }
    }

    let mut failed = false;

    if !missing_workspace_lints.is_empty() {
        if args.fix {
            for path in &missing_workspace_lints {
                let mut content = fs::read_to_string(path)?;
                if !content.contains("[lints]") {
                    content.push_str("\n[lints]\nworkspace = true\n");
                    fs::write(path, content)?;
                    println!("Fixed missing [lints] in {:?}", path);
                }
            }
        } else {
            eprintln!(
                "The following packages are not inheriting workspace lints ([lints] workspace = true):"
            );
            for path in &missing_workspace_lints {
                eprintln!("  - {:?}", path);
            }
            failed = true;
        }
    }

    if !non_workspace_dependencies.is_empty() {
        println!("Note: Found non-inherited dependency declarations across crates:");
        for (dep, packages) in non_workspace_dependencies.iter().take(10) {
            println!("  - {}: {}", dep, packages.join(", "));
        }
        if non_workspace_dependencies.len() > 10 {
            println!("  ... and {} more", non_workspace_dependencies.len() - 10);
        }
    }

    if failed {
        bail!(
            "Package conformity check failed. Run `cargo xtask package-conformity --fix` to fix lints inheritance."
        );
    }

    println!(
        "Package conformity: OK (checked {} member crates)",
        crates.len()
    );
    Ok(())
}
