//! CaptureCraft core: screen capture primitives.

use anyhow::{Context, Result};
use xcap::Monitor;
pub use xcap::image::RgbaImage;

/// Returns the primary monitor (falls back to the first one found).
pub fn primary_monitor() -> Result<Monitor> {
    let monitors = Monitor::all().context("failed to enumerate monitors")?;
    let primary = monitors
        .iter()
        .position(|m| m.is_primary().unwrap_or(false))
        .unwrap_or(0);
    monitors
        .into_iter()
        .nth(primary)
        .context("no monitors found")
}

/// Captures the full primary screen as RGBA.
pub fn capture_primary() -> Result<RgbaImage> {
    primary_monitor()?
        .capture_image()
        .context("failed to capture primary monitor")
}
