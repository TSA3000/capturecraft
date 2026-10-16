//! Real screen-capture tests. They need a logged-in desktop session
//! (and Screen Recording permission on macOS), so they're skipped in CI.
//! Run locally with: cargo test -- --include-ignored

use capturecraft::{Monitor, Region, RgbaImage, capture_primary, capture_region, primary_monitor};
use std::{fs, path::PathBuf};

fn save(img: &RgbaImage, name: &str) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("test-captures");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    img.save(&path).expect("PNG save should succeed");
    println!("saved {}", path.display());
}

/// Accepts logical size or physical (scaled) size, ±1 px for rounding.
fn assert_size(img: &RgbaImage, w: u32, h: u32, monitor: &Monitor) {
    let scale = monitor.scale_factor().unwrap_or(1.0);
    let close = |a: u32, b: f32| (a as f32 - b).abs() <= 1.0;
    let ok = |s: f32| close(img.width(), w as f32 * s) && close(img.height(), h as f32 * s);
    assert!(
        ok(1.0) || ok(scale),
        "unexpected size {}x{} (expected {}x{} at scale 1.0 or {})",
        img.width(),
        img.height(),
        w,
        h,
        scale
    );
}

fn assert_not_black(img: &RgbaImage) {
    assert!(
        img.pixels().any(|p| p.0[0] > 0 || p.0[1] > 0 || p.0[2] > 0),
        "captured image is completely black"
    );
}

#[test]
#[ignore = "needs a real desktop session"]
fn captures_primary_screen() {
    let monitor = primary_monitor().expect("should find a monitor");
    let img = capture_primary().expect("capture should succeed");

    assert_size(
        &img,
        monitor.width().unwrap(),
        monitor.height().unwrap(),
        &monitor,
    );
    assert_not_black(&img);
    save(&img, "primary.png");
}

#[test]
#[ignore = "needs a real desktop session"]
fn captures_region() {
    let monitor = primary_monitor().unwrap();
    let img = capture_region(&monitor, Region::new(10, 10, 400, 300))
        .expect("region capture should succeed");

    assert_size(&img, 400, 300, &monitor);
    save(&img, "region.png");
}

#[test]
#[ignore = "needs a real desktop session"]
fn captures_region_from_reverse_drag() {
    let monitor = primary_monitor().unwrap();
    let region = Region::from_drag((410, 310), (10, 10));
    let img = capture_region(&monitor, region).expect("region capture should succeed");

    assert_size(&img, 400, 300, &monitor);
}

#[test]
#[ignore = "needs a real desktop session"]
fn oversized_region_is_clipped_to_monitor() {
    let monitor = primary_monitor().unwrap();
    let (mw, mh) = (monitor.width().unwrap(), monitor.height().unwrap());
    let img = capture_region(&monitor, Region::new(mw - 50, mh - 40, 500, 500))
        .expect("clipped capture should succeed");

    assert_size(&img, 50, 40, &monitor);
    save(&img, "clipped.png");
}

#[test]
#[ignore = "needs a real desktop session"]
fn region_outside_monitor_errors() {
    let monitor = primary_monitor().unwrap();
    let (mw, mh) = (monitor.width().unwrap(), monitor.height().unwrap());

    assert!(capture_region(&monitor, Region::new(mw, mh, 10, 10)).is_err());
}
