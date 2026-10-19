//! Workspace tooling: `cargo xtask <task>`.

mod layers;
mod models;

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    let result = match task.as_deref() {
        Some("layers") => check_layers(),
        Some("models") => models::run(&std::env::args().skip(2).collect::<Vec<_>>()),
        _ => Err("usage: cargo xtask layers | models [DIR]".to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn check_layers() -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let crates = layers::from_metadata(&meta)?;
    let errors = layers::check(&crates);
    if errors.is_empty() {
        println!("layers: {} workspace crates OK", crates.len());
        Ok(())
    } else {
        Err(format!("layer violations:\n  {}", errors.join("\n  ")))
    }
}
