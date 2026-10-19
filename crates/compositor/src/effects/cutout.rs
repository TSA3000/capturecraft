//! Cut Out: removes a strip and joins the two pieces along a straight or
//! torn-paper seam, optionally with a gap and a shadow cast across it.
//!
//! The math works in seam coordinates: *along* runs parallel to the seam
//! (x for a horizontal cut), *across* perpendicular to it. Output pixels
//! before the seam sample the source unchanged, pixels after it sample the
//! source shifted past the removed strip. The torn edges are 1D fBm
//! gradient noise of the along coordinate, from an integer hash of the
//! seed, so every machine and both backends tear the same way.

use photocraft_geom::Rect;

use super::{Px, over, scale};
use crate::scene::{CutAxis, CutOutSeamSpec, SeamStyle};

/// Octaves of the tear profile.
pub const OCTAVES: u32 = 6;
pub const MAX_AMPLITUDE: f32 = 256.0;
pub const MAX_SHADOW_SAMPLES: u32 = 32;

/// 32-bit integer hash (lowbias32).
#[inline]
pub fn hash32(v: u32) -> u32 {
    let mut x = v;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// Gradient at lattice point `i` of `octave`, in `[-1, 1]`.
#[inline]
pub fn lattice(seed: [u32; 2], octave: u32, i: i32) -> f32 {
    let h = hash32(
        hash32(hash32((i as u32) ^ seed[0]).wrapping_add(seed[1]))
            ^ octave.wrapping_mul(0x9e37_79b9),
    );
    (h >> 8) as f32 * (1.0 / 16_777_215.0) * 2.0 - 1.0
}

/// 1D gradient (Perlin) noise with the quintic fade, roughly `[-1, 1]`.
#[inline]
pub fn noise1(x: f32, seed: [u32; 2], octave: u32) -> f32 {
    let fi = x.floor();
    let f = x - fi;
    let i = fi as i32;
    let g0 = lattice(seed, octave, i) * f;
    let g1 = lattice(seed, octave, i.wrapping_add(1)) * (f - 1.0);
    let t = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    (g0 + (g1 - g0) * t) * 2.0
}

/// Fractional Brownian motion: octaves at doubling frequency, amplitude
/// multiplied by `roughness` each step, normalized.
#[inline]
pub fn fbm(s: f32, seed: [u32; 2], octaves: u32, roughness: f32, inv_wavelength: f32) -> f32 {
    let (mut sum, mut amp, mut norm, mut freq) = (0.0f32, 1.0f32, 0.0f32, inv_wavelength);
    for o in 0..octaves {
        sum += amp * noise1(s * freq, seed, o);
        norm += amp;
        amp *= roughness;
        freq *= 2.0;
    }
    sum / norm.max(1e-6)
}

/// Tear depth profile in `[0, 1]`.
#[inline]
pub fn profile(s: f32, seed: [u32; 2], octaves: u32, roughness: f32, inv_wavelength: f32) -> f32 {
    (0.5 + 0.75 * fbm(s, seed, octaves, roughness, inv_wavelength)).clamp(0.0, 1.0)
}

/// Splits a 64-bit seed into the two words the shader hashes.
pub fn seed_words(seed: u64) -> [u32; 2] {
    [seed as u32, (seed >> 32) as u32]
}

/// A validated Cut Out, in the form both backends evaluate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CutParams {
    pub vertical: bool,
    pub src_w: u32,
    pub src_h: u32,
    pub dst_w: u32,
    pub dst_h: u32,
    pub t0: i32,
    pub removed: i32,
    pub gap: i32,
    pub amplitude: f32,
    pub roughness: f32,
    pub inv_wavelength: f32,
    pub seed_upper: [u32; 2],
    pub seed_lower: [u32; 2],
    /// `(along, across)` offset of the seam shadow, its sample count and
    /// premultiplied colour.
    pub shadow: Option<([f32; 2], u32, Px)>,
    pub gap_fill: Px,
}

impl CutParams {
    /// `None` when the strip is empty or outside the canvas.
    pub fn new(spec: &CutOutSeamSpec, src_w: u32, src_h: u32) -> Option<CutParams> {
        let vertical = spec.axis == CutAxis::Vertical;
        let across = if vertical { src_w } else { src_h };
        let t1 = spec.t1.min(across);
        if spec.t0 >= t1 {
            return None;
        }
        let removed = t1 - spec.t0;
        let gap = spec.gap_px.min(4096);
        let dst_across = across - removed + gap;
        if dst_across == 0 {
            return None;
        }
        let (dst_w, dst_h) = if vertical {
            (dst_across, src_h)
        } else {
            (src_w, dst_across)
        };
        let (amplitude, roughness, inv_wavelength, seed) = match spec.seam {
            SeamStyle::Straight => (0.0, 0.0, 0.0, 0),
            SeamStyle::TornEdge {
                amplitude_px,
                roughness,
                wavelength_px,
                seed,
            } => {
                let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
                (
                    fin(amplitude_px, 0.0).clamp(0.0, MAX_AMPLITUDE),
                    fin(roughness, 0.5).clamp(0.0, 1.0),
                    1.0 / fin(wavelength_px, 32.0).clamp(2.0, 4096.0),
                    seed,
                )
            }
        };
        let shadow = spec.shadow.and_then(|s| {
            let ok = s.offset.is_finite() && s.color.a > 0.0;
            let (along, across) = if vertical {
                (s.offset.y, s.offset.x)
            } else {
                (s.offset.x, s.offset.y)
            };
            ok.then(|| {
                (
                    [along.clamp(-64.0, 64.0), across.clamp(-64.0, 64.0)],
                    s.samples.clamp(1, MAX_SHADOW_SAMPLES),
                    s.color.premultiplied(),
                )
            })
        });
        Some(CutParams {
            vertical,
            src_w,
            src_h,
            dst_w,
            dst_h,
            t0: spec.t0 as i32,
            removed: removed as i32,
            gap: gap as i32,
            amplitude,
            roughness,
            inv_wavelength,
            seed_upper: seed_words(seed),
            seed_lower: seed_words(seed.rotate_left(29) ^ 0x9e37_79b9_7f4a_7c15),
            shadow,
            gap_fill: spec.gap_fill.premultiplied(),
        })
    }

    /// Upper/left and lower/right piece edges (output across coordinate)
    /// at `along` (WGSL `cut_edges`).
    #[inline]
    pub fn edges(&self, along: f32) -> [f32; 2] {
        let t0 = self.t0 as f32;
        let gap = self.gap as f32;
        if self.amplitude <= 0.0 {
            return [t0, t0 + gap];
        }
        let hu = profile(
            along,
            self.seed_upper,
            OCTAVES,
            self.roughness,
            self.inv_wavelength,
        );
        if self.gap == 0 {
            // One shared tear: the pieces interlock.
            let e = t0 + self.amplitude * (hu - 0.5);
            return [e, e];
        }
        let hl = profile(
            along,
            self.seed_lower,
            OCTAVES,
            self.roughness,
            self.inv_wavelength,
        );
        [t0 - self.amplitude * hu, t0 + gap + self.amplitude * hl]
    }

    fn across_len(&self) -> i32 {
        (if self.vertical {
            self.src_w
        } else {
            self.src_h
        }) as i32
    }

    /// Output pixel `(x, y)` given the source canvas (WGSL `cutout`).
    pub fn pixel(&self, src: &impl Fn(i32, i32) -> Px, x: i32, y: i32) -> Px {
        let (a, c) = if self.vertical { (y, x) } else { (x, y) };
        let along = a as f32 + 0.5;
        let e = self.edges(along);
        self.compose(src, a, c, e, self.shadow_edges(along).as_ref())
    }

    /// Upper edge at every shadow sample position for `along`.
    pub fn shadow_edges(&self, along: f32) -> Option<[f32; MAX_SHADOW_SAMPLES as usize]> {
        self.shadow.map(|(off, n, _)| {
            let mut buf = [0.0f32; MAX_SHADOW_SAMPLES as usize];
            for (k, v) in buf.iter_mut().enumerate().take(n as usize) {
                let t = (k as u32 + 1) as f32 / n as f32;
                *v = self.edges(along - off[0] * t)[0];
            }
            buf
        })
    }

    /// The pixel from precomputed edges (`shadow_edges[k]` = upper edge at
    /// the k-th shadow sample).
    pub fn compose(
        &self,
        src: &impl Fn(i32, i32) -> Px,
        a: i32,
        c: i32,
        e: [f32; 2],
        shadow_edges: Option<&[f32; MAX_SHADOW_SAMPLES as usize]>,
    ) -> Px {
        let cf = c as f32;
        let cu = (e[0] - cf).clamp(0.0, 1.0);
        let cl = (cf + 1.0 - e[1]).clamp(0.0, 1.0);
        let max = self.across_len() - 1;
        let cu_src = c.clamp(0, max);
        let cl_src = (c - self.gap + self.removed).clamp(0, max);
        let at = |across: i32| {
            if self.vertical {
                src(across, a)
            } else {
                src(a, across)
            }
        };
        let mut out = scale(self.gap_fill, (1.0 - cu - cl).max(0.0));
        out = over(scale(at(cl_src), cl), out);
        if let (Some((off, n, color)), Some(edges)) = (self.shadow, shadow_edges) {
            let mut s = 0.0f32;
            for (k, ea) in edges.iter().enumerate().take(n as usize) {
                let t = (k as u32 + 1) as f32 / n as f32;
                s += (ea - (cf - off[1] * t)).clamp(0.0, 1.0);
            }
            s /= n as f32;
            out = over(scale(color, s), out);
        }
        over(scale(at(cu_src), cu), out)
    }

    /// Source rectangles read to produce output `out`.
    pub fn read_rects(&self, out: Rect) -> [Rect; 2] {
        let src = Rect::new(0, 0, self.src_w as i32, self.src_h as i32);
        let shift = self.removed - self.gap;
        let lower = if self.vertical {
            out.translate(shift, 0)
        } else {
            out.translate(0, shift)
        };
        [out.intersect(&src), lower.intersect(&src)]
    }

    /// Output rectangle affected by a change of source rectangle `r`.
    pub fn forward(&self, r: Rect) -> Rect {
        let dst = Rect::new(0, 0, self.dst_w as i32, self.dst_h as i32);
        let reach = self.amplitude.ceil() as i32 + 2;
        let (lo, hi) = if self.vertical {
            (r.x0, r.x1)
        } else {
            (r.y0, r.y1)
        };
        let mut out = Rect::EMPTY;
        // Upper piece: identity up to the deepest tear.
        let u_hi = hi.min(self.t0 + reach);
        if lo < u_hi {
            out = out.union(&self.with_across(r, lo, u_hi));
        }
        // Lower piece: shifted back over the removed strip.
        let l_lo = lo.max(self.t0 + self.removed - reach);
        if l_lo < hi {
            let shift = self.gap - self.removed;
            out = out.union(&self.with_across(r, l_lo + shift, hi + shift));
        }
        out.intersect(&dst)
    }

    fn with_across(&self, r: Rect, a0: i32, a1: i32) -> Rect {
        if self.vertical {
            Rect::new(a0, r.y0, a1, r.y1)
        } else {
            Rect::new(r.x0, a0, r.x1, a1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Color, PointF32, SeamShadow};

    fn spec(seam: SeamStyle, gap: u32) -> CutOutSeamSpec {
        CutOutSeamSpec {
            axis: CutAxis::Horizontal,
            t0: 40,
            t1: 70,
            gap_px: gap,
            gap_fill: Color::TRANSPARENT,
            seam,
            shadow: Some(SeamShadow {
                color: Color::new(0.0, 0.0, 0.0, 0.5),
                offset: PointF32::new(2.0, 4.0),
                samples: 6,
            }),
        }
    }

    #[test]
    fn noise_is_deterministic_and_bounded() {
        let a: Vec<f32> = (0..500)
            .map(|i| fbm(i as f32 * 0.37, [7, 9], OCTAVES, 0.6, 1.0 / 24.0))
            .collect();
        let b: Vec<f32> = (0..500)
            .map(|i| fbm(i as f32 * 0.37, [7, 9], OCTAVES, 0.6, 1.0 / 24.0))
            .collect();
        assert_eq!(a, b);
        assert!(a.iter().all(|v| v.abs() <= 1.0));
        let c: Vec<f32> = (0..500)
            .map(|i| fbm(i as f32 * 0.37, [8, 9], OCTAVES, 0.6, 1.0 / 24.0))
            .collect();
        assert_ne!(a, c, "the seed changes the tear");
        // Continuous: neighbouring samples stay close.
        assert!(a.windows(2).all(|w| (w[0] - w[1]).abs() < 0.5));
    }

    #[test]
    fn straight_cut_joins_rows() {
        let mut s = spec(SeamStyle::Straight, 0);
        s.shadow = None;
        let p = CutParams::new(&s, 10, 100).unwrap_or(empty());
        assert_eq!((p.dst_w, p.dst_h), (10, 70));
        let src = |x: i32, y: i32| [x as f32 / 10.0, y as f32 / 100.0, 0.0, 1.0];
        let row = |y: i32| (p.pixel(&src, 5, y)[1] * 100.0).round() as i32;
        assert_eq!(row(10), 10);
        assert_eq!(row(39), 39);
        assert_eq!(row(40), 70);
        assert_eq!(row(69), 99);
    }

    fn empty() -> CutParams {
        CutParams {
            vertical: false,
            src_w: 0,
            src_h: 0,
            dst_w: 0,
            dst_h: 0,
            t0: 0,
            removed: 0,
            gap: 0,
            amplitude: 0.0,
            roughness: 0.0,
            inv_wavelength: 0.0,
            seed_upper: [0; 2],
            seed_lower: [0; 2],
            shadow: None,
            gap_fill: [0.0; 4],
        }
    }

    #[test]
    fn torn_gap_leaves_transparent_space_with_a_shadow() {
        let seam = SeamStyle::TornEdge {
            amplitude_px: 6.0,
            roughness: 0.6,
            wavelength_px: 20.0,
            seed: 42,
        };
        let p = CutParams::new(&spec(seam, 12), 200, 100).unwrap_or(empty());
        assert_eq!(p.dst_h, 82);
        let src = |_: i32, _: i32| [1.0, 1.0, 1.0, 1.0];
        for x in 0..200 {
            let e = p.edges(x as f32 + 0.5);
            assert!(e[0] <= 40.0 && e[0] >= 34.0 && e[1] >= 52.0 && e[1] <= 58.0);
            // Middle of the gap: no piece, only the shadow (dark, partial).
            let m = p.pixel(&src, x, 46);
            assert!(m[3] < 0.6 && m[0] == 0.0);
            // Far from the seam: the image.
            assert_eq!(p.pixel(&src, x, 10), [1.0; 4]);
            assert_eq!(p.pixel(&src, x, 75), [1.0; 4]);
        }
    }

    #[test]
    fn forward_maps_both_pieces() {
        let p = CutParams::new(&spec(SeamStyle::Straight, 0), 100, 100).unwrap_or(empty());
        assert_eq!(
            p.forward(Rect::new(0, 80, 10, 90)),
            Rect::new(0, 50, 10, 60)
        );
        assert_eq!(p.forward(Rect::new(0, 50, 10, 60)), Rect::EMPTY);
        assert_eq!(
            p.read_rects(Rect::new(0, 50, 10, 60))[1],
            Rect::new(0, 80, 10, 90)
        );
    }
}
