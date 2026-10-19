//! Sobel edge magnitude on luma, and run-length projections over it: how
//! much of a box outline is backed by real edges (container confidence) and
//! how busy an area is (photos versus flat UI).

use photocraft_geom::Rect;
use rayon::prelude::*;

use crate::image::{RgbaImage, luma};

/// `|gx| + |gy|` of the 3 × 3 Sobel operator on luma (0..=2040), border
/// pixels replicated.
pub fn sobel(img: &RgbaImage) -> Vec<u16> {
    let (w, h) = (img.width(), img.height());
    let y: Vec<i32> = img.pixels().iter().map(|p| i32::from(luma(*p))).collect();
    let at = |x: isize, yy: isize| -> i32 {
        let x = x.clamp(0, w as isize - 1) as usize;
        let yy = yy.clamp(0, h as isize - 1) as usize;
        y[yy * w + x]
    };
    let mut out = vec![0u16; w * h];
    out.par_chunks_mut(w.max(1))
        .enumerate()
        .for_each(|(r, row)| {
            let r = r as isize;
            for (c, slot) in row.iter_mut().enumerate() {
                let c = c as isize;
                let gx = at(c + 1, r - 1) + 2 * at(c + 1, r) + at(c + 1, r + 1)
                    - at(c - 1, r - 1)
                    - 2 * at(c - 1, r)
                    - at(c - 1, r + 1);
                let gy = at(c - 1, r + 1) + 2 * at(c, r + 1) + at(c + 1, r + 1)
                    - at(c - 1, r - 1)
                    - 2 * at(c, r - 1)
                    - at(c + 1, r - 1);
                *slot = (gx.abs() + gy.abs()).min(i32::from(u16::MAX)) as u16;
            }
        });
    out
}

/// Longest run of edge pixels (`mag ≥ thr` within ±1 px across the line)
/// along a horizontal (`horizontal`) or vertical line at `at`, between
/// `from..to`; gaps up to 2 px are bridged (dashed and anti-aliased edges).
#[allow(clippy::too_many_arguments)]
fn longest_run(
    mag: &[u16],
    w: usize,
    h: usize,
    horizontal: bool,
    at: i32,
    from: i32,
    to: i32,
    thr: u16,
) -> i32 {
    let strong = |t: i32| -> bool {
        (-1..=1).any(|d| {
            let (x, y) = if horizontal { (t, at + d) } else { (at + d, t) };
            x >= 0
                && y >= 0
                && (x as usize) < w
                && (y as usize) < h
                && mag
                    .get(y as usize * w + x as usize)
                    .is_some_and(|m| *m >= thr)
        })
    };
    let (mut best, mut run, mut gap) = (0, 0, 0);
    for t in from..to {
        if strong(t) {
            run += 1 + gap;
            gap = 0;
            best = best.max(run);
        } else if run > 0 && gap < 2 {
            gap += 1;
        } else {
            run = 0;
            gap = 0;
        }
    }
    best
}

/// Mean fraction of each side of `r` (local coordinates) covered by an
/// edge run: 1 for a crisp card outline, ~0 for a box with no visible edge.
pub fn side_support(mag: &[u16], w: usize, h: usize, r: Rect, thr: u16) -> f32 {
    if r.is_empty() {
        return 0.0;
    }
    let (rw, rh) = (r.width().max(1) as f32, r.height().max(1) as f32);
    let top = longest_run(mag, w, h, true, r.y0, r.x0, r.x1, thr) as f32 / rw;
    let bottom = longest_run(mag, w, h, true, r.y1 - 1, r.x0, r.x1, thr) as f32 / rw;
    let left = longest_run(mag, w, h, false, r.x0, r.y0, r.y1, thr) as f32 / rh;
    let right = longest_run(mag, w, h, false, r.x1 - 1, r.y0, r.y1, thr) as f32 / rh;
    ((top + bottom + left + right) / 4.0).clamp(0.0, 1.0)
}

/// Fraction of pixels in `r` with `mag ≥ thr`.
pub fn edge_density(mag: &[u16], w: usize, r: Rect, thr: u16) -> f32 {
    if r.is_empty() {
        return 0.0;
    }
    let mut n = 0usize;
    for y in r.y0.max(0)..r.y1 {
        for x in r.x0.max(0)..r.x1 {
            if mag
                .get(y as usize * w + x as usize)
                .is_some_and(|m| *m >= thr)
            {
                n += 1;
            }
        }
    }
    n as f32 / r.size().area() as f32
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_drawn_box_has_full_support() {
        let (w, h) = (60usize, 40usize);
        let mut rgba = vec![255u8; w * h * 4];
        for y in 10..30 {
            for x in 10..50 {
                let i = (y * w + x) * 4;
                rgba[i..i + 3].copy_from_slice(&[40, 40, 40]);
            }
        }
        let img = RgbaImage::from_rgba8(Rect::new(0, 0, w as i32, h as i32), &rgba).unwrap();
        let mag = sobel(&img);
        assert!(side_support(&mag, w, h, Rect::new(10, 10, 50, 30), 64) > 0.95);
        assert!(side_support(&mag, w, h, Rect::new(20, 15, 40, 25), 64) < 0.05);
        assert!(edge_density(&mag, w, Rect::new(0, 0, 5, 5), 64) == 0.0);
    }
}
