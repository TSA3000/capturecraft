//! CaptureCraft Editor: annotate a capture and save or copy it.
//!
//! `capturecraft-editor [image.png]` — without a path it opens the newest
//! capture in the configured output folder. It is a separate program so it
//! can be opened on its own, and because the capture process has already
//! used up its one event loop.
#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{Context, Result, bail};
use capturecraft::config::Config;
use std::path::PathBuf;

fn main() -> Result<()> {
    let config = Config::load_or_create()?;
    let arg = std::env::args().nth(1);
    let path = match arg.as_deref() {
        Some("-h") | Some("--help") => {
            println!("usage: capturecraft-editor [image.png]");
            return Ok(());
        }
        Some(path) => PathBuf::from(path),
        None => capturecraft::app::newest_capture(&config.output_dir)
            .with_context(|| format!("no captures found in {}", config.output_dir.display()))?,
    };
    if !path.exists() {
        bail!("{} does not exist", path.display());
    }

    let image = xcap::image::open(&path)
        .with_context(|| format!("failed to read {}", path.display()))?
        .to_rgba8();
    capturecraft::editor_ui::run(image, config, Some(path))?;
    Ok(())
}
