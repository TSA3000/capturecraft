//! CaptureCraft core: screen capture primitives.

pub mod app;
mod autostart;
pub mod config;
pub(crate) mod draw;
pub mod editor;
pub mod editor_ui;
pub mod overlay;
pub mod prefs_ui;
mod region;
pub mod theme;
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
pub fn capture_interactive(
    mode: config::SelectMode,
    style: config::Overlay,
) -> Result<Option<RgbaImage>> {
    overlay::select_interactive(mode, style)
}

/// Opens `path` in the standalone editor.
pub fn open_editor(path: &std::path::Path) {
    let exe = sibling_exe("capturecraft-editor");
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    if let Err(e) = cmd.spawn() {
        eprintln!("failed to start {}: {e}", exe.display());
    }
}

/// Path of another CaptureCraft binary in the same folder as this one.
pub fn sibling_exe(name: &str) -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// 32x32 RGBA app icon: blue rounded square with white corner brackets.
pub fn icon_rgba() -> (Vec<u8>, u32, u32) {
    const S: i32 = 32;
    let mut rgba = Vec::with_capacity((S * S * 4) as usize);
    for y in 0..S {
        for x in 0..S {
            // Rounded corners (radius 6)
            let (cx, cy) = (x.clamp(6, S - 7), y.clamp(6, S - 7));
            let inside = (x - cx).pow(2) + (y - cy).pow(2) <= 36;
            let near = |v: i32, edge: i32| (edge..edge + 3).contains(&v);
            let in_corner = |v: i32| (7..14).contains(&v) || (S - 14..S - 7).contains(&v);
            let bracket = (in_corner(x) && (near(y, 7) || near(y, S - 10)))
                || (in_corner(y) && (near(x, 7) || near(x, S - 10)));
            let px = match (inside, bracket) {
                (false, _) => [0, 0, 0, 0],
                (true, true) => [255, 255, 255, 255],
                (true, false) => [0x3B, 0x82, 0xF6, 255],
            };
            rgba.extend_from_slice(&px);
        }
    }
    (rgba, S as u32, S as u32)
}

/// One capture end to end: optional delay, overlay (or full screen),
/// save to `output_dir`, and copy to the clipboard.
/// Returns the saved path, or `None` if the user cancelled.
pub fn run_capture(
    config: &config::Config,
    mode: config::SelectMode,
    delay: std::time::Duration,
) -> Result<Option<std::path::PathBuf>> {
    std::thread::sleep(delay);
    let img = match mode {
        config::SelectMode::Fullscreen => capture_primary()?,
        m => match capture_interactive(m, config.overlay.clone())? {
            Some(img) => img,
            None => return Ok(None),
        },
    };
    let path = tray::save_capture(&img, &config.output_dir)?;
    if config.clipboard
        && let Err(e) = copy_to_clipboard(&img)
    {
        eprintln!("warning: {e:#}");
    }
    // The editor is its own program: the capture is on disk first, so it is
    // never lost if the editor fails to start.
    if config.editor {
        open_editor(&path);
    }
    Ok(Some(path))
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
