use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use toml::Value;

#[derive(Parser, Debug)]
pub struct CheckBoundariesArgs {}

pub fn run_check_boundaries(_args: CheckBoundariesArgs) -> Result<()> {
    let root_path = Path::new("Cargo.toml");
    if !root_path.exists() {
        bail!("Must run from workspace root");
    }

    let mut crate_internal_deps: HashMap<String, HashSet<String>> = HashMap::new();

    for entry in fs::read_dir("crates").context("reading crates/")? {
        let entry = entry?;
        let manifest_path = entry.path().join("Cargo.toml");
        if manifest_path.exists() {
            let content = fs::read_to_string(&manifest_path)?;
            let toml: Value = toml::from_str(&content)?;
            if let Some(pkg_name) = toml
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
            {
                let mut internal = HashSet::new();
                if let Some(deps) = toml.get("dependencies").and_then(|d| d.as_table()) {
                    for (dep_name, _) in deps {
                        if dep_name == "tessera" || dep_name.starts_with("tessera-") {
                            internal.insert(dep_name.clone());
                        }
                    }
                }
                crate_internal_deps.insert(pkg_name.to_string(), internal);
            }
        }
    }

    let rules: Vec<(&str, &[&str])> = vec![
        ("tessera-model", &[]),
        ("tessera-wayland-protocols", &[]),
        ("tessera-security", &["tessera-model"]),
        ("tessera-semantic", &["tessera-model"]),
        ("tessera-config", &["tessera-model"]),
        (
            "tessera-ipc",
            &["tessera-model", "tessera-security", "tessera-semantic"],
        ),
        ("tessera-ipc-client", &["tessera-model", "tessera-ipc"]),
        (
            "tessera-commands",
            &["tessera-model", "tessera-config", "tessera-ipc", "tessera-security"],
        ),
        ("tessera-backend", &["tessera-model", "tessera-wayland-protocols"]),
        ("tessera-render", &["tessera-model"]),
        (
            "tessera-compositor",
            &["tessera-model", "tessera-semantic", "tessera-wayland-protocols"],
        ),
    ];

    let mut failed = false;

    for (pkg, allowed) in rules {
        if let Some(actual_deps) = crate_internal_deps.get(pkg) {
            let allowed_set: HashSet<String> = allowed.iter().map(|s| s.to_string()).collect();
            for actual in actual_deps {
                if !allowed_set.contains(actual) {
                    eprintln!(
                        "Architectural violation: {} must not depend on {}",
                        pkg, actual
                    );
                    failed = true;
                }
            }
        }
    }

    if failed {
        bail!("Crate boundary check failed");
    }

    println!("Crate dependency boundaries: OK");
    Ok(())
}
