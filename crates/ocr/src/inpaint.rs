//! Background healing: what was behind a UI element, from the pixels
//! around it.
//!
//! 1. **Ring sampling.** The `ring`-pixel band just outside the hole is
//!    sampled, skipping other foreground (`protect`). Flat UI backgrounds
//!    are reconstructed analytically: a solid colour, a least-squares
//!    linear gradient (a plane per channel), or, for smooth but non-linear
//!    backgrounds, a bilinear blend of the four sides. These are exact for
//!    the solid and gradient fills that make up most screenshots.
//! 2. **Fast marching (Telea).** Textured surroundings are propagated
//!    inwards pixel by pixel in order of distance from the edge (A. Telea,
//!    *An Image Inpainting Technique Based on the Fast Marching Method*,
//!    JGT 2004): each pixel is a weighted first-order extrapolation of the
//!    known pixels within `radius`.
//! 3. **PatchMatch.** Large textured holes use `photocraft-algo`'s
//!    content-aware completion (Wexler/PatchMatch), which copies texture
//!    instead of smoothing it; Telea is the fallback when it finds nothing
//!    to copy from.
//!
//! Every path is deterministic. [`heal_all`] heals independent holes in
//! parallel; [`apply`] writes a result into a copy-on-write surface, so
//! only the 256² tiles under the hole are duplicated.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use photocraft_algo::inpaint::{CompleteParams, Px as Role, complete_masked};
use photocraft_color::PixelFormat;
use photocraft_geom::{Rect, TILE_SIZE};
use photocraft_raster::{Interrupt, Surface};
use rayon::prelude::*;

use crate::OcrError;
use crate::image::RgbaImage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealMethod {
    Solid,
    Gradient,
    Bilinear,
    Telea,
    PatchMatch,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InpaintParams {
    /// Width of the sampled band around the hole.
    pub ring: u32,
    /// Largest RMS residual (0..255 units) the analytic fits may leave.
    pub max_rms: f32,
    /// Largest step between neighbouring ring pixels for the bilinear fill.
    pub max_step: f32,
    /// Telea neighbourhood radius.
    pub radius: u32,
    /// Textured holes at least this large (pixels) use PatchMatch.
    pub patch_match_area: u64,
}

impl Default for InpaintParams {
    fn default() -> Self {
        Self {
            ring: 3,
            max_rms: 3.0,
            max_step: 6.0,
            radius: 5,
            patch_match_area: 48 * 48,
        }
    }
}

/// A healed hole: RGBA8 for `rect`.
#[derive(Clone, Debug, PartialEq)]
pub struct HealPatch {
    pub rect: Rect,
    pub rgba: Vec<u8>,
    pub method: HealMethod,
}

/// Heals every hole of `src` independently (in parallel): each sees the
/// original pixels, and `protect` (plus the other holes) is never sampled.
pub fn heal_all(
    src: &Surface,
    bounds: Rect,
    holes: &[Rect],
    protect: &[Rect],
    p: &InpaintParams,
) -> Vec<Result<HealPatch, OcrError>> {
    holes
        .par_iter()
        .enumerate()
        .map(|(k, &hole)| {
            let others: Vec<Rect> = protect
                .iter()
                .copied()
                .chain(
                    holes
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| *j != k)
                        .map(|(_, r)| *r),
                )
                .filter(|r| !r.contains_rect(&hole))
                .collect();
            inpaint_region(src, bounds, hole, &others, p)
        })
        .collect()
}

/// Heals `hole` (clipped to `bounds`) from its surroundings in `src`.
pub fn inpaint_region(
    src: &Surface,
    bounds: Rect,
    hole: Rect,
    protect: &[Rect],
    p: &InpaintParams,
) -> Result<HealPatch, OcrError> {
    let hole = hole.intersect(&bounds);
    if hole.is_empty() {
        return Err(OcrError::Empty);
    }
    let margin = (p.radius.max(p.ring) + 2) as i32;
    // PatchMatch wants room to find source patches.
    let reach = margin.max((hole.width().max(hole.height()) as i32).min(256));
    let big = hole.size().area() >= p.patch_match_area;
    let window = hole
        .inflate(if big { reach } else { margin })
        .intersect(&bounds);
    let img = RgbaImage::from_surface(src, window)?;
    let local_hole = img.local(hole);
    let protect: Vec<Rect> = protect
        .iter()
        .map(|r| img.local(r.inflate(1)))
        .filter(|r| !r.is_empty())
        .collect();
    let field = Field::new(&img, local_hole, &protect);

    let (rgba, method) = match analytic(&field, &img, local_hole, p) {
        Some(found) => found,
        None => {
            let pm = big.then(|| patch_match(&field, &img, local_hole)).flatten();
            match pm {
                Some(v) => (v, HealMethod::PatchMatch),
                None => (telea(&field, &img, local_hole, p.radius), HealMethod::Telea),
            }
        }
    };
    Ok(HealPatch {
        rect: hole,
        rgba,
        method,
    })
}

/// Writes `patch` into `dst` (RGBA8) through `try_tile_mut`: tiles shared
/// with other surfaces are copied first. Returns the number of tiles touched.
pub fn apply(dst: &mut Surface, patch: &HealPatch) -> Result<usize, OcrError> {
    if dst.format() != PixelFormat::RGBA8 {
        return Err(OcrError::Raster("surface is not RGBA8".into()));
    }
    let r = patch.rect;
    let w = r.width() as usize;
    if patch.rgba.len() != w * r.height() as usize * 4 {
        return Err(OcrError::Raster("patch size mismatch".into()));
    }
    let t = TILE_SIZE as usize;
    let mut n = 0;
    for tc in r.tiles() {
        let tr = tc.rect().intersect(&r);
        let (ox, oy) = (tc.tx * TILE_SIZE, tc.ty * TILE_SIZE);
        let span = tr.width() as usize * 4;
        let bytes = dst
            .try_tile_mut(tc)
            .map_err(|e| OcrError::Raster(e.to_string()))?
            .bytes_mut();
        for y in tr.y0..tr.y1 {
            let d = ((y - oy) as usize * t + (tr.x0 - ox) as usize) * 4;
            let s = ((y - r.y0) as usize * w + (tr.x0 - r.x0) as usize) * 4;
            if let (Some(dd), Some(ss)) = (bytes.get_mut(d..d + span), patch.rgba.get(s..s + span))
            {
                dd.copy_from_slice(ss);
            }
        }
        n += 1;
    }
    Ok(n)
}

/// What each pixel of the working window is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell {
    /// Usable background.
    Known,
    /// Foreground of something else: neither used nor filled.
    Protected,
    Hole,
}

struct Field {
    w: usize,
    h: usize,
    cells: Vec<Cell>,
}

impl Field {
    fn new(img: &RgbaImage, hole: Rect, protect: &[Rect]) -> Self {
        let (w, h) = (img.width(), img.height());
        let mut cells = vec![Cell::Known; w * h];
        for r in protect {
            for y in r.y0.max(0)..r.y1.min(h as i32) {
                for x in r.x0.max(0)..r.x1.min(w as i32) {
                    cells[y as usize * w + x as usize] = Cell::Protected;
                }
            }
        }
        for y in hole.y0..hole.y1 {
            for x in hole.x0..hole.x1 {
                cells[y as usize * w + x as usize] = Cell::Hole;
            }
        }
        Self { w, h, cells }
    }

    fn at(&self, x: i32, y: i32) -> Option<Cell> {
        (x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h)
            .then(|| self.cells[y as usize * self.w + x as usize])
    }
}

fn to_f(p: [u8; 4]) -> [f32; 4] {
    p.map(f32::from)
}

fn to_u8(v: [f32; 4]) -> [u8; 4] {
    v.map(|c| c.round().clamp(0.0, 255.0) as u8)
}

/// Least-squares plane `a + b·x + c·y` per channel over the samples.
struct Plane {
    cx: f64,
    cy: f64,
    coef: [[f64; 3]; 4],
    rms: f32,
    /// Largest change across the hole (`|b|·w + |c|·h`).
    swing: f32,
}

fn fit_plane(samples: &[(i32, i32, [f32; 4])], extent: (f64, f64)) -> Option<Plane> {
    let n = samples.len() as f64;
    if samples.len() < 3 {
        return None;
    }
    let cx = samples.iter().map(|s| f64::from(s.0)).sum::<f64>() / n;
    let cy = samples.iter().map(|s| f64::from(s.1)).sum::<f64>() / n;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for s in samples {
        let (dx, dy) = (f64::from(s.0) - cx, f64::from(s.1) - cy);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    let det = sxx * syy - sxy * sxy;
    let mut coef = [[0.0; 3]; 4];
    for (c, k) in coef.iter_mut().enumerate() {
        let mean = samples.iter().map(|s| f64::from(s.2[c])).sum::<f64>() / n;
        let (mut sxv, mut syv) = (0.0, 0.0);
        for s in samples {
            let v = f64::from(s.2[c]) - mean;
            sxv += (f64::from(s.0) - cx) * v;
            syv += (f64::from(s.1) - cy) * v;
        }
        // Degenerate geometry (a one-row ring): fall back to the mean.
        let (b, cc) = if det.abs() > 1e-9 {
            ((sxv * syy - syv * sxy) / det, (syv * sxx - sxv * sxy) / det)
        } else {
            (0.0, 0.0)
        };
        *k = [mean, b, cc];
    }
    let mut sum = 0.0f64;
    for s in samples {
        let (dx, dy) = (f64::from(s.0) - cx, f64::from(s.1) - cy);
        let worst = coef
            .iter()
            .enumerate()
            .map(|(c, k)| (k[0] + k[1] * dx + k[2] * dy - f64::from(s.2[c])).powi(2))
            .fold(0.0, f64::max);
        sum += worst;
    }
    let swing = coef
        .iter()
        .map(|k| k[1].abs() * extent.0 + k[2].abs() * extent.1)
        .fold(0.0, f64::max);
    Some(Plane {
        cx,
        cy,
        coef,
        rms: (sum / n).sqrt() as f32,
        swing: swing as f32,
    })
}

/// Solid, gradient or bilinear fill when the ring is smooth enough.
fn analytic(
    f: &Field,
    img: &RgbaImage,
    hole: Rect,
    p: &InpaintParams,
) -> Option<(Vec<u8>, HealMethod)> {
    let ring = p.ring as i32;
    let outer = hole.inflate(ring);
    let mut samples = Vec::new();
    for y in outer.y0..outer.y1 {
        for x in outer.x0..outer.x1 {
            if f.at(x, y) == Some(Cell::Known) && !hole.contains(x, y) {
                samples.push((x, y, to_f(img.get(x, y))));
            }
        }
    }
    let extent = (f64::from(hole.width()), f64::from(hole.height()));
    let plane = fit_plane(&samples, extent)?;
    let (w, h) = (hole.width() as usize, hole.height() as usize);
    let mut out = Vec::with_capacity(w * h * 4);
    if plane.rms <= p.max_rms {
        let solid = plane.swing < 1.0;
        for y in hole.y0..hole.y1 {
            for x in hole.x0..hole.x1 {
                let (dx, dy) = (f64::from(x) - plane.cx, f64::from(y) - plane.cy);
                let v = plane.coef.map(|k| {
                    if solid {
                        k[0] as f32
                    } else {
                        (k[0] + k[1] * dx + k[2] * dy) as f32
                    }
                });
                out.extend(to_u8(v));
            }
        }
        let m = if solid {
            HealMethod::Solid
        } else {
            HealMethod::Gradient
        };
        return Some((out, m));
    }
    bilinear(f, img, hole, p.max_step).map(|v| (v, HealMethod::Bilinear))
}

/// Blends the nearest known pixel left/right of each row and above/below
/// each column, weighted by inverse distance. Only for smooth rings (no
/// step between neighbouring ring pixels above `max_step`).
fn bilinear(f: &Field, img: &RgbaImage, hole: Rect, max_step: f32) -> Option<Vec<u8>> {
    let side = |x: i32, y: i32| (f.at(x, y) == Some(Cell::Known)).then(|| to_f(img.get(x, y)));
    let left: Vec<Option<[f32; 4]>> = (hole.y0..hole.y1).map(|y| side(hole.x0 - 1, y)).collect();
    let right: Vec<Option<[f32; 4]>> = (hole.y0..hole.y1).map(|y| side(hole.x1, y)).collect();
    let top: Vec<Option<[f32; 4]>> = (hole.x0..hole.x1).map(|x| side(x, hole.y0 - 1)).collect();
    let bottom: Vec<Option<[f32; 4]>> = (hole.x0..hole.x1).map(|x| side(x, hole.y1)).collect();
    let smooth = |v: &[Option<[f32; 4]>]| {
        v.windows(2).all(|w| match (w[0], w[1]) {
            (Some(a), Some(b)) => a.iter().zip(b).all(|(x, y)| (x - y).abs() <= max_step),
            _ => true,
        })
    };
    if ![&left, &right, &top, &bottom].iter().all(|v| smooth(v)) {
        return None;
    }
    let lerp = |a: Option<[f32; 4]>, b: Option<[f32; 4]>, t: f32| match (a, b) {
        (Some(a), Some(b)) => Some(std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    let (w, h) = (hole.width() as f32, hole.height() as f32);
    let mut out = Vec::with_capacity((w * h) as usize * 4);
    for (j, y) in (hole.y0..hole.y1).enumerate() {
        for (i, x) in (hole.x0..hole.x1).enumerate() {
            let u = (i as f32 + 0.5) / w;
            let v = (j as f32 + 0.5) / h;
            let hz = lerp(left[j], right[j], u);
            let vt = lerp(top[i], bottom[i], v);
            let dh = 1.0 / ((x - hole.x0 + 1).min(hole.x1 - x) as f32);
            let dv = 1.0 / ((y - hole.y0 + 1).min(hole.y1 - y) as f32);
            let px = match (hz, vt) {
                (Some(a), Some(b)) => std::array::from_fn(|c| (a[c] * dh + b[c] * dv) / (dh + dv)),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => return None,
            };
            out.extend(to_u8(px));
        }
    }
    Some(out)
}

/// Content-aware completion over the window; `None` when there is nothing
/// to copy from.
fn patch_match(f: &Field, img: &RgbaImage, hole: Rect) -> Option<Vec<u8>> {
    let (w, h) = (f.w, f.h);
    let buf: Vec<f32> = img
        .pixels()
        .iter()
        .flat_map(|p| p.map(|c| f32::from(c) / 255.0))
        .collect();
    let roles: Vec<Role> = f
        .cells
        .iter()
        .map(|c| match c {
            Cell::Known => Role::Source,
            Cell::Protected => Role::Void,
            Cell::Hole => Role::Hole,
        })
        .collect();
    let done = complete_masked(
        w,
        h,
        4,
        &buf,
        &roles,
        &CompleteParams::default(),
        &Interrupt::NONE,
    )
    .ok()
    .flatten()?;
    let mut out = Vec::with_capacity(hole.size().area() as usize * 4);
    for y in hole.y0..hole.y1 {
        for x in hole.x0..hole.x1 {
            let i = (y as usize * w + x as usize) * 4;
            let px: [f32; 4] =
                std::array::from_fn(|c| done.get(i + c).copied().unwrap_or(0.0) * 255.0);
            out.extend(to_u8(px));
        }
    }
    Some(out)
}

/// Min-heap entry: distance first, then pixel index (deterministic ties).
#[derive(Clone, Copy, PartialEq)]
struct Front(f32, usize);

impl Eq for Front {}

impl Ord for Front {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0).then_with(|| o.1.cmp(&self.1))
    }
}

impl PartialOrd for Front {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Telea's fast-marching inpainting of the hole cells.
fn telea(f: &Field, img: &RgbaImage, hole: Rect, radius: u32) -> Vec<u8> {
    const INF: f32 = 1e6;
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Flag {
        Known,
        Band,
        Inside,
        Off,
    }
    let (w, h) = (f.w, f.h);
    let mut val: Vec<[f32; 4]> = img.pixels().iter().map(|p| to_f(*p)).collect();
    let mut flag: Vec<Flag> = f
        .cells
        .iter()
        .map(|c| match c {
            Cell::Known => Flag::Known,
            Cell::Protected => Flag::Off,
            Cell::Hole => Flag::Inside,
        })
        .collect();
    let mut t = vec![INF; w * h];
    let mut heap = BinaryHeap::new();
    let nb4 = |i: usize| -> [Option<usize>; 4] {
        let (x, y) = (i % w, i / w);
        [
            (x > 0).then(|| i - 1),
            (x + 1 < w).then(|| i + 1),
            (y > 0).then(|| i - w),
            (y + 1 < h).then(|| i + w),
        ]
    };
    // The band: known pixels next to the hole, at distance 0.
    for i in 0..w * h {
        if flag[i] == Flag::Known
            && nb4(i)
                .into_iter()
                .flatten()
                .any(|j| flag[j] == Flag::Inside)
        {
            flag[i] = Flag::Band;
            t[i] = 0.0;
            heap.push(Front(0.0, i));
        }
    }
    // Eikonal update from two neighbours (a hole with no usable
    // surroundings never enters the heap and keeps its pixels).
    let solve = |t: &[f32], flag: &[Flag], a: Option<usize>, b: Option<usize>| -> f32 {
        let known =
            |k: Option<usize>| k.filter(|&k| flag[k] != Flag::Inside && flag[k] != Flag::Off);
        match (known(a), known(b)) {
            (Some(a), Some(b)) => {
                let (t1, t2) = (t[a], t[b]);
                let r = 2.0 - (t1 - t2) * (t1 - t2);
                if r > 0.0 {
                    let s = (t1 + t2 - r.sqrt()) / 2.0;
                    if s >= t1 && s >= t2 {
                        return s;
                    }
                    let s = s + r.sqrt();
                    if s >= t1 && s >= t2 {
                        return s;
                    }
                }
                1.0 + t1.min(t2)
            }
            (Some(a), None) => 1.0 + t[a],
            (None, Some(b)) => 1.0 + t[b],
            (None, None) => INF,
        }
    };
    let r = radius.max(1) as i32;
    while let Some(Front(_, i)) = heap.pop() {
        if flag[i] == Flag::Known {
            continue;
        }
        flag[i] = Flag::Known;
        for j in nb4(i).into_iter().flatten() {
            if flag[j] != Flag::Inside {
                continue;
            }
            let [l, rr, u, d] = nb4(j);
            let tj = solve(&t, &flag, l, u)
                .min(solve(&t, &flag, rr, u))
                .min(solve(&t, &flag, l, d))
                .min(solve(&t, &flag, rr, d));
            t[j] = tj;
            // Gradient of T at j (central differences over known cells).
            let grad = |a: Option<usize>, b: Option<usize>| -> f32 {
                let ok = |k: Option<usize>| {
                    k.filter(|&k| flag[k] == Flag::Known || flag[k] == Flag::Band)
                };
                match (ok(a), ok(b)) {
                    (Some(a), Some(b)) => (t[b] - t[a]) / 2.0,
                    (Some(a), None) => tj - t[a],
                    (None, Some(b)) => t[b] - tj,
                    (None, None) => 0.0,
                }
            };
            let (gtx, gty) = (grad(l, rr), grad(u, d));
            let (jx, jy) = ((j % w) as i32, (j / w) as i32);
            let mut acc = [0.0f32; 4];
            let mut wsum = 0.0f32;
            for dy in -r..=r {
                for dx in -r..=r {
                    let (qx, qy) = (jx + dx, jy + dy);
                    if (dx == 0 && dy == 0) || dx * dx + dy * dy > r * r {
                        continue;
                    }
                    if qx < 0 || qy < 0 || qx as usize >= w || qy as usize >= h {
                        continue;
                    }
                    let q = qy as usize * w + qx as usize;
                    if flag[q] != Flag::Known && flag[q] != Flag::Band {
                        continue;
                    }
                    let (rx, ry) = ((jx - qx) as f32, (jy - qy) as f32);
                    let len2 = rx * rx + ry * ry;
                    let dir = ((rx * gtx + ry * gty) / len2.sqrt()).abs().max(1e-6);
                    let dst = 1.0 / len2;
                    let lev = 1.0 / (1.0 + (t[q] - tj).abs());
                    let wgt = dir * dst * lev;
                    // First-order term: the colour gradient at q, from known neighbours.
                    let qv = val[q];
                    let gi = |a: Option<usize>, b: Option<usize>, c: usize| -> f32 {
                        let ok = |k: Option<usize>| {
                            k.filter(|&k| flag[k] == Flag::Known || flag[k] == Flag::Band)
                        };
                        match (ok(a), ok(b)) {
                            (Some(a), Some(b)) => (val[b][c] - val[a][c]) / 2.0,
                            _ => 0.0,
                        }
                    };
                    let [ql, qr, qu, qd] = nb4(q);
                    for c in 0..4 {
                        let g = gi(ql, qr, c) * rx + gi(qu, qd, c) * ry;
                        acc[c] += wgt * (qv[c] + g);
                    }
                    wsum += wgt;
                }
            }
            if wsum > 0.0 {
                val[j] = acc.map(|a| (a / wsum).clamp(0.0, 255.0));
            }
            flag[j] = Flag::Band;
            heap.push(Front(tj, j));
        }
    }
    let mut out = Vec::with_capacity(hole.size().area() as usize * 4);
    for y in hole.y0..hole.y1 {
        for x in hole.x0..hole.x1 {
            out.extend(to_u8(val[y as usize * w + x as usize]));
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn surface(w: i32, h: i32, f: impl Fn(i32, i32) -> [u8; 4]) -> Surface {
        let mut s = Surface::new(PixelFormat::RGBA8);
        let rgba: Vec<u8> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| f(x, y))
            .collect();
        apply(
            &mut s,
            &HealPatch {
                rect: Rect::new(0, 0, w, h),
                rgba,
                method: HealMethod::Solid,
            },
        )
        .unwrap();
        s
    }

    fn max_err(p: &HealPatch, f: impl Fn(i32, i32) -> [u8; 4]) -> u8 {
        let mut e = 0;
        for (k, px) in p.rgba.as_chunks::<4>().0.iter().enumerate() {
            let w = p.rect.width() as i32;
            let (x, y) = (p.rect.x0 + k as i32 % w, p.rect.y0 + k as i32 / w);
            let want = f(x, y);
            for c in 0..4 {
                e = e.max(px[c].abs_diff(want[c]));
            }
        }
        e
    }

    #[test]
    fn solid_and_gradient_backgrounds_are_exact() {
        let bounds = Rect::new(0, 0, 300, 200);
        let solid = |_: i32, _: i32| [30, 31, 35, 255];
        let mut s = surface(300, 200, solid);
        // A "button" to remove.
        let hole = Rect::new(100, 80, 180, 110);
        let mut btn = HealPatch {
            rect: hole,
            rgba: [88, 101, 242, 255].repeat(80 * 30),
            method: HealMethod::Solid,
        };
        apply(&mut s, &btn).unwrap();
        let p = inpaint_region(&s, bounds, hole, &[], &InpaintParams::default()).unwrap();
        assert_eq!(p.method, HealMethod::Solid);
        assert_eq!(max_err(&p, solid), 0);

        let grad = |x: i32, y: i32| [(x / 2) as u8, (100 + y / 4) as u8, 50, 255];
        let mut s = surface(300, 200, grad);
        btn.rgba = [255, 255, 255, 255].repeat(80 * 30);
        apply(&mut s, &btn).unwrap();
        let p = inpaint_region(&s, bounds, hole, &[], &InpaintParams::default()).unwrap();
        assert_eq!(p.method, HealMethod::Gradient);
        assert!(max_err(&p, grad) <= 1);
    }

    #[test]
    fn protected_neighbours_are_not_sampled() {
        let bounds = Rect::new(0, 0, 120, 60);
        let bg = |_: i32, _: i32| [240, 240, 240, 255];
        let mut s = surface(120, 60, bg);
        // A dark neighbour right next to the hole.
        let n = Rect::new(50, 20, 70, 40);
        apply(
            &mut s,
            &HealPatch {
                rect: n,
                rgba: [0, 0, 0, 255].repeat(20 * 20),
                method: HealMethod::Solid,
            },
        )
        .unwrap();
        let hole = Rect::new(30, 20, 50, 40);
        let p = inpaint_region(&s, bounds, hole, &[n], &InpaintParams::default()).unwrap();
        assert_eq!(max_err(&p, bg), 0);
        let unprotected = inpaint_region(&s, bounds, hole, &[], &InpaintParams::default()).unwrap();
        assert!(max_err(&unprotected, bg) > 0);
    }

    #[test]
    fn textured_holes_blend_without_seams() {
        let bounds = Rect::new(0, 0, 80, 80);
        // A checker texture the analytic fits cannot express.
        let tex = |x: i32, y: i32| {
            let v = if (x / 2 + y / 2) % 2 == 0 { 60 } else { 200 };
            [v, v, v, 255]
        };
        let s = surface(80, 80, tex);
        let hole = Rect::new(36, 36, 44, 44);
        let p = inpaint_region(&s, bounds, hole, &[], &InpaintParams::default()).unwrap();
        assert_eq!(p.method, HealMethod::Telea);
        // Inpainted values stay within the surrounding range.
        assert!(
            p.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|px| (55..=205).contains(&px[0]))
        );
        let again = inpaint_region(&s, bounds, hole, &[], &InpaintParams::default()).unwrap();
        assert_eq!(p, again);
    }

    #[test]
    fn apply_copies_only_touched_tiles() {
        let base = surface(600, 300, |_, _| [1, 2, 3, 255]);
        let mut copy = base.clone();
        let n = apply(
            &mut copy,
            &HealPatch {
                rect: Rect::new(10, 10, 20, 20),
                rgba: vec![9; 400],
                method: HealMethod::Solid,
            },
        )
        .unwrap();
        assert_eq!(n, 1);
        let c = photocraft_geom::TileCoord::new;
        assert!(!std::sync::Arc::ptr_eq(
            base.tile(c(0, 0)).unwrap(),
            copy.tile(c(0, 0)).unwrap()
        ));
        assert!(std::sync::Arc::ptr_eq(
            base.tile(c(1, 0)).unwrap(),
            copy.tile(c(1, 0)).unwrap()
        ));
    }
}
