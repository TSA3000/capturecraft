//! Backdrop-sampling nodes: what they read from the composite beneath them
//! and how (magnifier resampling, blur, mosaic, spotlight dimming).
//!
//! Samplers take a `src(x, y)` closure that is only ever called with
//! coordinates already clamped to the canvas (edge pixels repeat), the same
//! clamp the shaders apply before indexing.

use super::{Px, over};

/// How the magnifier samples (WGSL `LensParams.mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum LensMode {
    CatmullRom = 0,
    Lanczos3 = 1,
    Nearest = 2,
    /// Nearest-neighbour plus a 1 px grid at source pixel boundaries.
    PixelGrid = 3,
}

/// Catmull-Rom weights for taps `-1, 0, 1, 2` at fraction `t`.
#[inline]
pub fn catmull_rom_weights(t: f32) -> [f32; 4] {
    [
        ((-0.5 * t + 1.0) * t - 0.5) * t,
        (1.5 * t - 2.5) * t * t + 1.0,
        ((-1.5 * t + 2.0) * t + 0.5) * t,
        (0.5 * t - 0.5) * t * t,
    ]
}

/// `sin(π x)` with the argument reduced to `[-½, ½]` first (the WGSL
/// `sin` is only accurate on `[-π, π]`).
#[inline]
pub fn sin_pi(x: f32) -> f32 {
    let n = x.round();
    let r = x - n;
    let s = (std::f32::consts::PI * r).sin();
    if (n as i32) & 1 == 0 { s } else { -s }
}

/// Lanczos (a = 3) kernel.
#[inline]
pub fn lanczos3(x: f32) -> f32 {
    if x.abs() < 1e-5 {
        return 1.0;
    }
    if x.abs() >= 3.0 {
        return 0.0;
    }
    let px = std::f32::consts::PI * x;
    3.0 * sin_pi(x) * sin_pi(x / 3.0) / (px * px)
}

/// Normalized Lanczos weights for taps `-2..=3` at fraction `t`.
#[inline]
pub fn lanczos3_weights(t: f32) -> [f32; 6] {
    let mut w = [0.0f32; 6];
    let mut sum = 0.0f32;
    for (i, v) in w.iter_mut().enumerate() {
        *v = lanczos3(i as f32 - 2.0 - t);
        sum += *v;
    }
    for v in &mut w {
        *v /= sum;
    }
    w
}

/// Resampled values can overshoot: clamp, and keep colour within alpha.
#[inline]
pub fn clamp_premultiplied(p: Px) -> Px {
    let a = p[3].clamp(0.0, 1.0);
    [
        p[0].clamp(0.0, 1.0).min(a),
        p[1].clamp(0.0, 1.0).min(a),
        p[2].clamp(0.0, 1.0).min(a),
        a,
    ]
}

/// Magnifier parameters as the shader sees them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensSampler {
    pub mode: LensMode,
    /// Lens centre (canvas px).
    pub center: [f32; 2],
    /// Magnified point (canvas px).
    pub source: [f32; 2],
    pub inv_zoom: f32,
    /// Premultiplied grid colour (pixel grid mode).
    pub grid: Px,
    pub width: i32,
    pub height: i32,
}

impl LensSampler {
    /// Source position seen at the centre of output pixel `(x, y)`.
    #[inline]
    pub fn source_pos(&self, x: i32, y: i32) -> [f32; 2] {
        [
            (x as f32 + 0.5 - self.center[0]) * self.inv_zoom + self.source[0],
            (y as f32 + 0.5 - self.center[1]) * self.inv_zoom + self.source[1],
        ]
    }

    #[inline]
    fn fetch(&self, src: &impl Fn(i32, i32) -> Px, x: i32, y: i32) -> Px {
        src(x.clamp(0, self.width - 1), y.clamp(0, self.height - 1))
    }

    /// Lens colour at output pixel `(x, y)`.
    pub fn sample(&self, src: &impl Fn(i32, i32) -> Px, x: i32, y: i32) -> Px {
        let u = self.source_pos(x, y);
        match self.mode {
            LensMode::CatmullRom => {
                let (sx, sy) = (u[0] - 0.5, u[1] - 0.5);
                let (fx, fy) = (sx.floor(), sy.floor());
                let (ix, iy) = (fx as i32, fy as i32);
                let wx = catmull_rom_weights(sx - fx);
                let wy = catmull_rom_weights(sy - fy);
                let mut acc = [0.0f32; 4];
                for (j, wyj) in wy.iter().enumerate() {
                    let mut row = [0.0f32; 4];
                    for (i, wxi) in wx.iter().enumerate() {
                        let p = self.fetch(src, ix - 1 + i as i32, iy - 1 + j as i32);
                        for c in 0..4 {
                            row[c] += wxi * p[c];
                        }
                    }
                    for c in 0..4 {
                        acc[c] += wyj * row[c];
                    }
                }
                clamp_premultiplied(acc)
            }
            LensMode::Lanczos3 => {
                let (sx, sy) = (u[0] - 0.5, u[1] - 0.5);
                let (fx, fy) = (sx.floor(), sy.floor());
                let (ix, iy) = (fx as i32, fy as i32);
                let wx = lanczos3_weights(sx - fx);
                let wy = lanczos3_weights(sy - fy);
                let mut acc = [0.0f32; 4];
                for (j, wyj) in wy.iter().enumerate() {
                    let mut row = [0.0f32; 4];
                    for (i, wxi) in wx.iter().enumerate() {
                        let p = self.fetch(src, ix - 2 + i as i32, iy - 2 + j as i32);
                        for c in 0..4 {
                            row[c] += wxi * p[c];
                        }
                    }
                    for c in 0..4 {
                        acc[c] += wyj * row[c];
                    }
                }
                clamp_premultiplied(acc)
            }
            LensMode::Nearest => self.fetch(src, u[0].floor() as i32, u[1].floor() as i32),
            LensMode::PixelGrid => {
                let (fx, fy) = (u[0].floor(), u[1].floor());
                let p = self.fetch(src, fx as i32, fy as i32);
                if u[0] - fx < self.inv_zoom || u[1] - fy < self.inv_zoom {
                    over(self.grid, p)
                } else {
                    p
                }
            }
        }
    }

    /// Source rectangle (canvas px, before clamping) read for output `out`.
    pub fn footprint(&self, out: photocraft_geom::Rect) -> photocraft_geom::Rect {
        let a = self.source_pos(out.x0, out.y0);
        let b = self.source_pos(out.x1 - 1, out.y1 - 1);
        let margin = 4;
        photocraft_geom::Rect::new(
            a[0].min(b[0]).floor() as i32 - margin,
            a[1].min(b[1]).floor() as i32 - margin,
            a[0].max(b[0]).ceil() as i32 + margin,
            a[1].max(b[1]).ceil() as i32 + margin,
        )
    }
}

/// Spotlight dimming: lays `dim_rgb` at opacity `a` over `f`, keeping its
/// alpha (WGSL `spotlight`).
#[inline]
pub fn dim(f: Px, dim_rgb: [f32; 3], a: f32) -> Px {
    let k = 1.0 - a;
    let fa = a * f[3];
    [
        f[0] * k + dim_rgb[0] * fa,
        f[1] * k + dim_rgb[1] * fa,
        f[2] * k + dim_rgb[2] * fa,
        f[3],
    ]
}

/// Mean of the block `[x0, x1) × [y0, y1)`, rows then columns
/// (WGSL `mosaic_reduce`).
pub fn block_mean(src: &impl Fn(i32, i32) -> Px, x0: i32, y0: i32, x1: i32, y1: i32) -> Px {
    let mut acc = [0.0f32; 4];
    for y in y0..y1 {
        for x in x0..x1 {
            let p = src(x, y);
            for c in 0..4 {
                acc[c] += p[c];
            }
        }
    }
    let n = ((x1 - x0).max(0) * (y1 - y0).max(0)) as f32;
    if n > 0.0 {
        [acc[0] / n, acc[1] / n, acc[2] / n, acc[3] / n]
    } else {
        [0.0; 4]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolating_kernels_reproduce_samples() {
        let w = catmull_rom_weights(0.0);
        assert_eq!(w, [0.0, 1.0, 0.0, 0.0]);
        let s: f32 = catmull_rom_weights(0.37).iter().sum();
        assert!((s - 1.0).abs() < 1e-6);
        let l = lanczos3_weights(0.0);
        assert!((l[2] - 1.0).abs() < 1e-6 && l[0].abs() < 1e-6);
        assert!((sin_pi(2.5) - 1.0).abs() < 1e-6 && (sin_pi(-1.5) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lens_magnifies_around_the_source() {
        let lens = LensSampler {
            mode: LensMode::Nearest,
            center: [100.0, 100.0],
            source: [10.0, 20.0],
            inv_zoom: 0.25,
            grid: [0.0; 4],
            width: 200,
            height: 200,
        };
        let src = |x: i32, y: i32| [x as f32, y as f32, 0.0, 1.0];
        // Four output pixels per source pixel.
        assert_eq!(lens.sample(&src, 100, 100), [10.0, 20.0, 0.0, 1.0]);
        assert_eq!(lens.sample(&src, 103, 100), [10.0, 20.0, 0.0, 1.0]);
        assert_eq!(lens.sample(&src, 104, 100), [11.0, 20.0, 0.0, 1.0]);
        // Clamped at the canvas edge.
        assert_eq!(lens.sample(&src, 0, 0)[0], 0.0);
    }

    #[test]
    fn dim_keeps_alpha() {
        let d = dim([0.5, 0.5, 0.5, 0.5], [0.0, 0.0, 0.0], 0.6);
        assert!((d[0] - 0.2).abs() < 1e-6 && d[3] == 0.5);
    }
}
