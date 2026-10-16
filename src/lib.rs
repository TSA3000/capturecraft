//! CaptureCraft core: screen capture primitives.

mod region;

use anyhow::{Context, Result};
pub use region::Region;
pub use xcap::Monitor;
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

/// Captures `region` from `monitor`. The region is clipped to the monitor;
/// errors if it lies completely outside or is empty.
///
/// The returned image may be larger than the region on HiDPI/Retina screens
/// (physical pixels vs. logical units).
pub fn capture_region(monitor: &Monitor, region: Region) -> Result<RgbaImage> {
    let (mw, mh) = (monitor.width()?, monitor.height()?);
    let clipped = region
        .clamp_to(mw, mh)
        .with_context(|| format!("region {region} is outside monitor {mw}x{mh}"))?;
    monitor
        .capture_region(clipped.x, clipped.y, clipped.width, clipped.height)
        .with_context(|| format!("failed to capture region {clipped}"))
}

/// Captures `region` from the primary monitor.
pub fn capture_primary_region(region: Region) -> Result<RgbaImage> {
    capture_region(&primary_monitor()?, region)
}
