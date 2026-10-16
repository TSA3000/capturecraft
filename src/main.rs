//! `capturecraft [output.png]` — captures the primary screen to a PNG.

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("capture.png"));

    let img = capturecraft::capture_primary()?;
    img.save(&out)?;
    println!("saved {} ({}x{})", out.display(), img.width(), img.height());
    Ok(())
}
