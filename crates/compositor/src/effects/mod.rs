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
