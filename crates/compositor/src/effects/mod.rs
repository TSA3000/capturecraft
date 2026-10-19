//! Per-pixel math shared by both compositors.
//!
//! Everything here is the CPU reference of a WGSL function in
//! `shaders/effects.wgsl`, written in the same f32 operations and the same
//! summation order, so the two backends agree to float rounding. When one
//! side changes, the other must change with it (the parity tests catch a
//! mismatch).

pub mod backdrop;
pub mod cutout;

use photocraft_doc::{FillRule, PathOp};
use photocraft_geom::Rect;
use photocraft_vector::Rasterizer;
use rayon::prelude::*;

use crate::primitives::P;

/// Premultiplied RGBA.
pub type Px = [f32; 4];

#[inline]
pub fn over(src: Px, dst: Px) -> Px {
    let k = 1.0 - src[3];
    [
        src[0] + dst[0] * k,
        src[1] + dst[1] * k,
        src[2] + dst[2] * k,
        src[3] + dst[3] * k,
    ]
}

#[inline]
pub fn scale(p: Px, s: f32) -> Px {
    [p[0] * s, p[1] * s, p[2] * s, p[3] * s]
}

/// WGSL `mix(a, b, t)`: `a * (1 - t) + b * t`.
#[inline]
pub fn mix(a: Px, b: Px, t: f32) -> Px {
    let u = 1.0 - t;
    [
        a[0] * u + b[0] * t,
        a[1] * u + b[1] * t,
        a[2] * u + b[2] * t,
        a[3] * u + b[3] * t,
    ]
}

/// Coverage (`0..=1`) over `rect`, row-major; zero outside.
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    pub rect: Rect,
    pub data: Vec<f32>,
}

impl Mask {
    #[inline]
    pub fn at(&self, x: i32, y: i32) -> f32 {
        if !self.rect.contains(x, y) {
            return 0.0;
        }
        let i =
            (y - self.rect.y0) as usize * self.rect.width() as usize + (x - self.rect.x0) as usize;
        self.data.get(i).copied().unwrap_or(0.0)
    }

    /// Exact-area coverage of the union of `components` (each filled
    /// non-zero), clipped to `clip`, with `pad` empty pixels around it (room
    /// for a blur). `None` when nothing is covered.
    pub fn rasterize(components: &[Vec<Vec<P>>], clip: Rect, pad: u32) -> Option<Mask> {
        let mut r = Rasterizer::new(false);
        for c in components.iter().filter(|c| !c.is_empty()) {
            r.add_component(c, PathOp::Combine, FillRule::NonZero);
        }
        let rect = r.pixel_bounds()?.intersect(&clip).inflate(pad as i32);
        if rect.is_empty() || rect.width() as u64 * rect.height() as u64 > MAX_MASK_PX {
            return None;
        }
        let data = r.render(rect);
        data.iter().any(|v| *v > 0.0).then_some(Mask { rect, data })
    }
}

/// Largest mask (pixels): 16k × 16k.
pub const MAX_MASK_PX: u64 = 1 << 28;

/// Most stops a gradient keeps (the format's limit).
pub const MAX_STOPS: usize = 16;

/// A fill paint resolved for the compositors. Mirrors `paint_at`.
#[derive(Clone, Debug, PartialEq)]
pub enum PaintPx {
    /// Premultiplied colour.
    Solid(Px),
    Gradient(Gradient),
}

impl PaintPx {
    /// Premultiplied colour at pixel `(x, y)` (sampled at its centre).
    #[inline]
    pub fn at(&self, x: i32, y: i32) -> Px {
        match self {
            PaintPx::Solid(c) => *c,
            PaintPx::Gradient(g) => g.at(x, y),
        }
    }

    /// Nothing visible anywhere.
    pub fn is_clear(&self) -> bool {
        match self {
            PaintPx::Solid(c) => c[3] <= 0.0,
            PaintPx::Gradient(g) => g.stops.iter().all(|s| s.1[3] <= 0.0),
        }
    }
}

/// Linear or radial gradient over straight-alpha stops.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub radial: bool,
    /// Linear: `from.x, from.y, d.x / |d|², d.y / |d|²` (`d = to - from`);
    /// radial: `center.x, center.y, 1 / radius, 0`.
    pub geo: [f32; 4],
    /// `(offset, straight sRGB-encoded RGBA)`, sorted by offset, all
    /// channels in `0..=1`; at least one.
    pub stops: Vec<(f32, [f32; 4])>,
}

impl Gradient {
    /// Position along the gradient, clamped to `0..=1`.
    #[inline]
    pub fn t(&self, x: i32, y: i32) -> f32 {
        let px = x as f32 + 0.5;
        let py = y as f32 + 0.5;
        let g = self.geo;
        let t = if self.radial {
            let dx = px - g[0];
            let dy = py - g[1];
            (dx * dx + dy * dy).sqrt() * g[2]
        } else {
            (px - g[0]) * g[2] + (py - g[1]) * g[3]
        };
        t.clamp(0.0, 1.0)
    }

    /// Straight colours interpolated between the stops around `t`, then
    /// premultiplied.
    #[inline]
    pub fn at(&self, x: i32, y: i32) -> Px {
        let t = self.t(x, y);
        let n = self.stops.len();
        let Some(first) = self.stops.first() else {
            return [0.0; 4];
        };
        let mut c = first.1;
        if t > first.0 {
            c = self.stops[n - 1].1;
            for i in 1..n {
                let (o0, c0) = self.stops[i - 1];
                let (o1, c1) = self.stops[i];
                if t <= o1 {
                    let span = o1 - o0;
                    let f = if span > 0.0 { (t - o0) / span } else { 1.0 };
                    c = mix(c0, c1, f);
                    break;
                }
            }
        }
        [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]
    }
}

/// Coverage with a soft edge `feather` pixels wide, centred on the outline
/// of `hard` (exact-area coverage, empty `pad` pixels around it): a
/// smoothstep of the signed distance from each pixel centre to the edge.
/// The distances are photocraft-algo's Euclidean distance transforms of
/// the inside and the outside. `hard` must have room for half the feather.
pub fn feather_mask(hard: &Mask, feather: f32) -> Mask {
    if !feather.is_finite() || feather < 0.5 {
        return hard.clone();
    }
    let w = hard.rect.width() as usize;
    let h = hard.rect.height() as usize;
    let inside: Vec<bool> = hard.data.iter().map(|v| *v >= 0.5).collect();
    let outside: Vec<bool> = inside.iter().map(|v| !v).collect();
    // Distance to the nearest inside pixel (outside pixels) and to the
    // nearest outside pixel (inside pixels); the edge lies half a pixel
    // short of either.
    let to_inside = photocraft_algo::selection::edt(&inside, w, h);
    let to_outside = photocraft_algo::selection::edt(&outside, w, h);
    if to_inside.len() != w * h || to_outside.len() != w * h {
        return hard.clone();
    }
    let data = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let sd = if inside[i] {
                0.5 - to_outside[i]
            } else {
                to_inside[i] - 0.5
            };
            let t = (0.5 - sd / feather).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        })
        .collect();
    Mask {
        rect: hard.rect,
        data,
    }
}

/// Normalized 1D Gaussian, radius `ceil(3σ)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Kernel {
    pub radius: u32,
    pub weights: Vec<f32>,
}

/// Largest blur sigma accepted (radius 384 px).
pub const MAX_SIGMA: f32 = 128.0;

impl Kernel {
    /// `None` for a negligible (or invalid) sigma.
    pub fn gaussian(sigma: f32) -> Option<Kernel> {
        if !sigma.is_finite() || sigma < 0.05 {
            return None;
        }
        let s = f64::from(sigma.min(MAX_SIGMA));
        let radius = (3.0 * s).ceil().max(1.0) as u32;
        let w: Vec<f64> = (-(radius as i64)..=radius as i64)
            .map(|i| (-((i * i) as f64) / (2.0 * s * s)).exp())
            .collect();
        let sum: f64 = w.iter().sum();
        Some(Kernel {
            radius,
            weights: w.iter().map(|v| (v / sum) as f32).collect(),
        })
    }
}

/// Separable blur of a mask, zero outside it (same rectangle out).
/// Mirrors `blur_mask_h` / `blur_mask_v`.
pub fn blur_mask(m: &Mask, k: &Kernel) -> Mask {
    let w = m.rect.width() as usize;
    let h = m.rect.height() as usize;
    let r = k.radius as i64;
    let mut tmp = vec![0.0f32; w * h];
    tmp.par_chunks_mut(w.max(1))
        .enumerate()
        .for_each(|(y, row)| {
            let src = &m.data[y * w..(y + 1) * w];
            for (x, out) in row.iter_mut().enumerate() {
                let mut acc = 0.0f32;
                for (kk, wt) in k.weights.iter().enumerate() {
                    let sx = x as i64 + kk as i64 - r;
                    if sx >= 0 && (sx as usize) < w {
                        acc += wt * src[sx as usize];
                    }
                }
                *out = acc;
            }
        });
    let mut data = vec![0.0f32; w * h];
    data.par_chunks_mut(w.max(1))
        .enumerate()
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                let mut acc = 0.0f32;
                for (kk, wt) in k.weights.iter().enumerate() {
                    let sy = y as i64 + kk as i64 - r;
                    if sy >= 0 && (sy as usize) < h {
                        acc += wt * tmp[sy as usize * w + x];
                    }
                }
                *out = acc;
            }
        });
    Mask { rect: m.rect, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_is_normalized_and_symmetric() {
        let k = Kernel::gaussian(2.5).unwrap_or(Kernel {
            radius: 0,
            weights: vec![],
        });
        assert_eq!(k.radius, 8);
        assert_eq!(k.weights.len(), 17);
        assert!((k.weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert_eq!(k.weights[0], k.weights[16]);
        assert!(Kernel::gaussian(0.0).is_none() && Kernel::gaussian(f32::NAN).is_none());
    }

    #[test]
    fn blurred_mask_keeps_its_mass() {
        let rect = Rect::new(0, 0, 40, 40);
        let mut data = vec![0.0; 1600];
        for y in 15..25 {
            for x in 15..25 {
                data[y * 40 + x] = 1.0;
            }
        }
        let k = Kernel::gaussian(2.0).unwrap_or(Kernel {
            radius: 0,
            weights: vec![1.0],
        });
        let b = blur_mask(&Mask { rect, data }, &k);
        assert!((b.data.iter().sum::<f32>() - 100.0).abs() < 1e-2);
        assert!(b.at(20, 20) > 0.9 && b.at(14, 20) > 0.0 && b.at(14, 20) < 0.5);
    }
}
