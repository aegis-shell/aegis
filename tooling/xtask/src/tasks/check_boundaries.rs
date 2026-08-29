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
                        if dep_name == "aegis" || dep_name.starts_with("aegis-") {
                            internal.insert(dep_name.clone());
                        }
                    }
                }
                crate_internal_deps.insert(pkg_name.to_string(), internal);
            }
        }
    }

    let rules: Vec<(&str, &[&str])> = vec![
        ("aegis-model", &[]),
        ("aegis-wayland-protocols", &[]),
        ("aegis-security", &["aegis-model"]),
        ("aegis-semantic", &["aegis-model"]),
        ("aegis-config", &["aegis-model"]),
        (
            "aegis-ipc",
            &["aegis-model", "aegis-security", "aegis-semantic"],
        ),
        ("aegis-ipc-client", &["aegis-model", "aegis-ipc"]),
        (
            "aegis-commands",
            &["aegis-model", "aegis-config", "aegis-ipc", "aegis-security"],
        ),
        ("aegis-backend", &["aegis-model", "aegis-wayland-protocols"]),
        ("aegis-render", &["aegis-model"]),
        (
            "aegis-compositor",
            &["aegis-model", "aegis-semantic", "aegis-wayland-protocols"],
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
