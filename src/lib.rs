//! CaptureCraft core: screen capture primitives.

mod draw;
pub mod overlay;
mod region;
pub mod tray;

use anyhow::{Context, Result};
pub use region::Region;
use std::borrow::Cow;
pub use xcap::Monitor;
pub use xcap::image::RgbaImage;
use xcap::image::imageops;

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

/// Freezes all monitors, shows the drag-to-select overlay and returns the
/// selected part. `None` if the user cancelled.
pub fn capture_interactive() -> Result<Option<RgbaImage>> {
    overlay::select_interactive()
}

/// Crops `region` (image pixels) out of `img`, clipped to the image bounds.
pub fn crop(img: &RgbaImage, region: Region) -> Option<RgbaImage> {
    let r = region.clamp_to(img.width(), img.height())?;
    Some(imageops::crop_imm(img, r.x, r.y, r.width, r.height).to_image())
}

/// Copies an image to the system clipboard.
///
/// Note: on Linux the clipboard content is owned by this process and is lost
/// when it exits, unless a clipboard manager picks it up. Tray mode keeps
/// the clipboard open, so it works there.
pub fn copy_to_clipboard(img: &RgbaImage) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("failed to open clipboard")?;
    copy_image(&mut clipboard, img)
}

/// Copies an image using an existing clipboard handle.
pub fn copy_image(clipboard: &mut arboard::Clipboard, img: &RgbaImage) -> Result<()> {
    clipboard
        .set_image(arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: Cow::Borrowed(img.as_raw()),
        })
        .context("failed to copy image to clipboard")
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcap::image::Rgba;

    fn gradient(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| Rgba([x as u8, y as u8, 0, 255]))
    }

    #[test]
    fn crop_inside() {
        let out = crop(&gradient(10, 10), Region::new(2, 3, 4, 5)).unwrap();
        assert_eq!(out.dimensions(), (4, 5));
        assert_eq!(out.get_pixel(0, 0), &Rgba([2, 3, 0, 255]));
        assert_eq!(out.get_pixel(3, 4), &Rgba([5, 7, 0, 255]));
    }

    #[test]
    fn crop_clips_overflow() {
        let out = crop(&gradient(10, 10), Region::new(8, 8, 50, 50)).unwrap();
        assert_eq!(out.dimensions(), (2, 2));
    }

    #[test]
    fn crop_outside_is_none() {
        assert!(crop(&gradient(10, 10), Region::new(10, 0, 5, 5)).is_none());
    }
}
