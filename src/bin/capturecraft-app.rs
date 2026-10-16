//! CaptureCraft window: Capture button and settings (no console on Windows).
//!
//! Also runs the capture itself when started with `--overlay <mode>`: the
//! window spawns a copy of *this* binary, so the overlay can never be an
//! older build than the window.
#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{Context, Result};
use capturecraft::config::{Config, SelectMode};
use std::time::Duration;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--overlay") => capture(&args),
        _ => capturecraft::app::run(),
    }
}

/// `--overlay <region|window|fullscreen> [--delay-ms N]`
fn capture(args: &[String]) -> Result<()> {
    let mode = match args.get(1).map(String::as_str) {
        Some("window") => SelectMode::Window,
        Some("fullscreen") => SelectMode::Fullscreen,
        _ => SelectMode::Region,
    };
    let delay = match args.iter().position(|a| a == "--delay-ms") {
        Some(i) => args
            .get(i + 1)
            .and_then(|v| v.parse().ok())
            .context("--delay-ms needs a number")?,
        None => 0,
    };

    let config = Config::load_or_create()?;
    match capturecraft::run_capture(&config, mode, Duration::from_millis(delay))? {
        Some(path) => println!("saved {}", path.display()),
        None => println!("cancelled"),
    }
    Ok(())
}
