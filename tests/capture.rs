use capturecraft::{capture_primary, primary_monitor};
use std::{fs, path::PathBuf};

#[test]
fn captures_primary_screen() {
    let monitor = primary_monitor().expect("should find a monitor");
    let (mw, mh) = (monitor.width().unwrap(), monitor.height().unwrap());

    let img = capture_primary().expect("capture should succeed");

    // Non-empty image
    assert!(img.width() > 0 && img.height() > 0, "image is empty");

    // With DPI scaling the capture may be in physical pixels while
    // width()/height() report logical size, so accept either.
    let scale = monitor.scale_factor().unwrap_or(1.0);
    let phys = (
        (mw as f32 * scale).round() as u32,
        (mh as f32 * scale).round() as u32,
    );
    let size = (img.width(), img.height());
    assert!(
        size == (mw, mh) || size == phys,
        "unexpected size {}x{} (monitor {}x{}, scale {})",
        size.0, size.1, mw, mh, scale
    );

    // Not a fully black frame (catches silent capture failures)
    assert!(
        img.pixels().any(|p| p.0[0] > 0 || p.0[1] > 0 || p.0[2] > 0),
        "captured image is completely black"
    );

    // Save for manual inspection
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("test-captures");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("primary.png");
    img.save(&path).expect("PNG save should succeed");
    assert!(path.exists());
    println!("saved {}", path.display());
}
