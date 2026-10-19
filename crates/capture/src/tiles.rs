//! Writing pixels into `photocraft-raster` RGBA8 surfaces, tile by tile,
//! with fallible allocation and checked indexing (the raster crate's
//! `write_interleaved` asserts on length mismatches).

use photocraft_color::PixelFormat;
use photocraft_geom::{Rect, TILE_SIZE};
use photocraft_raster::Surface;

use crate::{CaptureError, Result, RgbaFrame};

/// Writes `dst` (surface coordinates) with pixels from `src(x, y)`.
pub(crate) fn blit(
    surface: &mut Surface,
    dst: Rect,
    mut src: impl FnMut(i32, i32) -> [u8; 4],
) -> Result<()> {
    if surface.format() != PixelFormat::RGBA8 {
        return Err(CaptureError::InvalidFrame("surface is not RGBA8".into()));
    }
    if dst.is_empty() {
        return Ok(());
    }
    let t = TILE_SIZE as usize;
    for tc in dst.tiles() {
        let tr = tc.rect().intersect(&dst);
        let (ox, oy) = (tc.tx * TILE_SIZE, tc.ty * TILE_SIZE);
        let bytes = surface.try_tile_mut(tc)?.bytes_mut();
        for y in tr.y0..tr.y1 {
            let row = (y - oy) as usize * t;
            for x in tr.x0..tr.x1 {
                let i = (row + (x - ox) as usize) * 4;
                if let Some(p) = bytes.get_mut(i..i + 4) {
                    p.copy_from_slice(&src(x, y));
                }
            }
        }
    }
    Ok(())
}

/// Copies a whole frame to `(x, y)`.
pub(crate) fn write_frame(surface: &mut Surface, f: &RgbaFrame, x: i32, y: i32) -> Result<()> {
    let dst = Rect::from_xywh(x, y, f.width(), f.height());
    blit(surface, dst, |sx, sy| {
        f.pixel(i64::from(sx - x), i64::from(sy - y))
            .unwrap_or_default()
    })
}
