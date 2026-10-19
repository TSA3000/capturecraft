//! A region of a `photocraft-raster` surface as one RGBA8 buffer, read tile
//! band by tile band in parallel, plus the colour helpers every pass uses.

use photocraft_color::PixelFormat;
use photocraft_geom::{Rect, TILE_SIZE};
use photocraft_raster::Surface;
use rayon::prelude::*;

use crate::OcrError;

/// Largest region analysed in one go (a 8K × 8K capture).
pub const MAX_PIXELS: usize = 1 << 26;

pub type Px = [u8; 4];

/// Straight-alpha RGBA8 pixels of `rect` (base space), row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbaImage {
    rect: Rect,
    w: usize,
    h: usize,
    px: Vec<Px>,
}

impl RgbaImage {
    /// Reads `region` of an RGBA8 surface; one task per row of 256² tiles.
    pub fn from_surface(s: &Surface, region: Rect) -> Result<Self, OcrError> {
        if s.format() != PixelFormat::RGBA8 {
            return Err(OcrError::Raster("surface is not RGBA8".into()));
        }
        let (w, h) = (region.width() as usize, region.height() as usize);
        let n = w
            .checked_mul(h)
            .filter(|n| *n <= MAX_PIXELS)
            .ok_or(OcrError::TooLarge)?;
        if n == 0 {
            return Err(OcrError::Empty);
        }
        let mut px: Vec<Px> = Vec::new();
        px.try_reserve_exact(n).map_err(|_| OcrError::TooLarge)?;
        px.resize(n, [0; 4]);
        let mut chunks: Vec<(Rect, &mut [Px])> = Vec::new();
        let mut rest: &mut [Px] = &mut px;
        for (y0, y1) in tile_spans(region.y0, region.y1) {
            let (band, tail) = rest.split_at_mut((y1 - y0) as usize * w);
            chunks.push((Rect::new(region.x0, y0, region.x1, y1), band));
            rest = tail;
        }
        chunks
            .into_par_iter()
            .try_for_each(|(r, out)| -> Result<(), OcrError> {
                let bytes = s
                    .try_to_interleaved(r)
                    .map_err(|e| OcrError::Raster(e.to_string()))?;
                for (o, b) in out.iter_mut().zip(bytes.as_chunks::<4>().0) {
                    *o = *b;
                }
                Ok(())
            })?;
        Ok(Self {
            rect: region,
            w,
            h,
            px,
        })
    }

    /// From interleaved RGBA8 bytes placed at `rect`.
    pub fn from_rgba8(rect: Rect, rgba: &[u8]) -> Option<Self> {
        let (w, h) = (rect.width() as usize, rect.height() as usize);
        if rgba.len() != w.checked_mul(h)?.checked_mul(4)? {
            return None;
        }
        let px = rgba.as_chunks::<4>().0.to_vec();
        Some(Self { rect, w, h, px })
    }

    /// Base-space rectangle the pixels cover.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn width(&self) -> usize {
        self.w
    }

    pub fn height(&self) -> usize {
        self.h
    }

    pub fn pixels(&self) -> &[Px] {
        &self.px
    }

    /// Local coordinates; transparent outside.
    pub fn get(&self, x: i32, y: i32) -> Px {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return [0; 4];
        }
        self.px
            .get(y as usize * self.w + x as usize)
            .copied()
            .unwrap_or([0; 4])
    }

    /// Base-space rectangle to local, clipped.
    pub fn local(&self, r: Rect) -> Rect {
        r.intersect(&self.rect)
            .translate(-self.rect.x0, -self.rect.y0)
    }

    /// Local rectangle to base space.
    pub fn global(&self, r: Rect) -> Rect {
        r.translate(self.rect.x0, self.rect.y0)
    }

    pub fn to_rgba8(&self) -> Vec<u8> {
        self.px.iter().flatten().copied().collect()
    }

    /// RGB bytes (what the text recogniser takes).
    pub fn to_rgb8(&self) -> Vec<u8> {
        self.px.iter().flat_map(|p| [p[0], p[1], p[2]]).collect()
    }
}

/// `[a, b)` split at multiples of [`TILE_SIZE`] (the raster tile grid).
pub fn tile_spans(a: i32, b: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    let mut s = a;
    while s < b {
        let e = ((s.div_euclid(TILE_SIZE) + 1) * TILE_SIZE).min(b);
        out.push((s, e));
        s = e;
    }
    out
}

/// Rec. 709 luma, integer.
pub fn luma(p: Px) -> u8 {
    let y = (2126 * u32::from(p[0]) + 7152 * u32::from(p[1]) + 722 * u32::from(p[2])) / 10_000;
    y.min(255) as u8
}

/// Largest per-channel difference (alpha included).
pub fn dist(a: Px, b: Px) -> u8 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(y))
        .max()
        .unwrap_or(0)
}

/// HSV-style saturation in 0..=1.
pub fn saturation(p: Px) -> f32 {
    let mx = p[0].max(p[1]).max(p[2]);
    let mn = p[0].min(p[1]).min(p[2]);
    if mx == 0 {
        0.0
    } else {
        f32::from(mx - mn) / f32::from(mx)
    }
}

/// The most common colour: a 15-bit histogram, then the mean of the
/// pixels in the winning bin (so it is a real colour of the input).
pub fn mode(colors: impl Iterator<Item = Px>) -> Option<Px> {
    let mut bins: std::collections::BTreeMap<u16, (u32, [u64; 4])> = Default::default();
    for p in colors {
        let k = (u16::from(p[0] >> 3) << 10) | (u16::from(p[1] >> 3) << 5) | u16::from(p[2] >> 3);
        let (n, s) = bins.entry(k).or_insert((0, [0; 4]));
        *n += 1;
        for (c, v) in s.iter_mut().zip(p) {
            *c += u64::from(v);
        }
    }
    // Ties go to the lowest bin, so the result never depends on order.
    let (_, (n, s)) = bins
        .iter()
        .max_by_key(|(k, (n, _))| (*n, std::cmp::Reverse(**k)))?;
    let n = u64::from(*n).max(1);
    Some(s.map(|c| (c / n) as u8))
}

/// Pixels on the `ring`-wide band just outside `r` (local coordinates),
/// skipping those where `skip` is true.
pub fn ring_pixels(
    img: &RgbaImage,
    r: Rect,
    ring: i32,
    skip: impl Fn(i32, i32) -> bool,
) -> Vec<Px> {
    let bounds = Rect::new(0, 0, img.w as i32, img.h as i32);
    let o = r.inflate(ring).intersect(&bounds);
    let mut out = Vec::new();
    for y in o.y0..o.y1 {
        for x in o.x0..o.x1 {
            if !r.contains(x, y) && !skip(x, y) {
                out.push(img.get(x, y));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_follow_the_tile_grid() {
        assert_eq!(tile_spans(10, 600), [(10, 256), (256, 512), (512, 600)]);
        assert_eq!(tile_spans(-5, 3), [(-5, 0), (0, 3)]);
        assert!(tile_spans(4, 4).is_empty());
    }

    #[test]
    fn mode_picks_the_common_colour() {
        let px = [[10, 10, 10, 255], [200, 0, 0, 255], [11, 9, 10, 255]];
        assert_eq!(mode(px.into_iter()), Some([10, 9, 10, 255]));
        assert_eq!(mode(std::iter::empty()), None);
    }
}
