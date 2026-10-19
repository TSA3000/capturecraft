//! Frame comparison for the pixel parity contract: any scene the GPU
//! compositor renders must match the CPU oracle within
//! [`MAX_CHANNEL_DIFF`] per channel and reach [`MIN_SSIM`].

use crate::canvas::Frame;

/// Largest allowed per-channel difference (premultiplied, `0..=1`).
pub const MAX_CHANNEL_DIFF: f32 = 2.0 / 255.0;
/// Smallest allowed structural similarity.
pub const MIN_SSIM: f64 = 0.995;

/// Result of comparing two frames.
#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub max_diff: f32,
    /// Where `max_diff` occurs.
    pub at: (u32, u32),
    /// Pixels with any channel over [`MAX_CHANNEL_DIFF`].
    pub over_tolerance: usize,
    pub ssim: f64,
}

impl Comparison {
    pub fn passes(&self) -> bool {
        self.max_diff <= MAX_CHANNEL_DIFF && self.ssim >= MIN_SSIM
    }
}

/// Compares two frames of the same size (`None` if the sizes differ).
pub fn compare(a: &Frame, b: &Frame) -> Option<Comparison> {
    if a.width != b.width || a.height != b.height || a.pixels.len() != b.pixels.len() {
        return None;
    }
    let mut max_diff = 0.0f32;
    let mut at = (0, 0);
    let mut over_tolerance = 0;
    for (i, (p, q)) in a.pixels.iter().zip(&b.pixels).enumerate() {
        let d = (0..4).map(|c| (p[c] - q[c]).abs()).fold(0.0f32, f32::max);
        let d = if d.is_nan() { f32::INFINITY } else { d };
        if d > MAX_CHANNEL_DIFF {
            over_tolerance += 1;
        }
        if d > max_diff {
            max_diff = d;
            at = ((i % a.width as usize) as u32, (i / a.width as usize) as u32);
        }
    }
    Some(Comparison {
        max_diff,
        at,
        over_tolerance,
        ssim: ssim(a, b),
    })
}

/// Mean SSIM over all four channels, 8×8 windows with stride 4 (Wang et
/// al. 2004 constants, dynamic range 1).
pub fn ssim(a: &Frame, b: &Frame) -> f64 {
    const WIN: u32 = 8;
    const STRIDE: u32 = 4;
    let (c1, c2) = (0.01f64.powi(2), 0.03f64.powi(2));
    if a.width < WIN || a.height < WIN {
        return if a == b { 1.0 } else { 0.0 };
    }
    let (mut total, mut n) = (0.0f64, 0usize);
    let mut y = 0;
    while y + WIN <= a.height {
        let mut x = 0;
        while x + WIN <= a.width {
            for c in 0..4 {
                let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
                for j in 0..WIN {
                    for i in 0..WIN {
                        let va = f64::from(a.pixel(x + i, y + j)[c]);
                        let vb = f64::from(b.pixel(x + i, y + j)[c]);
                        sa += va;
                        sb += vb;
                        saa += va * va;
                        sbb += vb * vb;
                        sab += va * vb;
                    }
                }
                let k = f64::from(WIN * WIN);
                let (ma, mb) = (sa / k, sb / k);
                let va = (saa / k - ma * ma).max(0.0);
                let vb = (sbb / k - mb * mb).max(0.0);
                let cov = sab / k - ma * mb;
                total += ((2.0 * ma * mb + c1) * (2.0 * cov + c2))
                    / ((ma * ma + mb * mb + c1) * (va + vb + c2));
                n += 1;
            }
            x += STRIDE;
        }
        y += STRIDE;
    }
    if n == 0 { 1.0 } else { total / n as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_frames_pass_and_noise_fails() {
        let px: Vec<[f32; 4]> = (0..32 * 32)
            .map(|i| [(i % 7) as f32 / 7.0, 0.5, (i % 3) as f32 / 3.0, 1.0])
            .collect();
        let a = Frame {
            width: 32,
            height: 32,
            pixels: px.clone(),
        };
        let c = compare(&a, &a).unwrap_or(Comparison {
            max_diff: 1.0,
            at: (0, 0),
            over_tolerance: 0,
            ssim: 0.0,
        });
        assert!(c.passes() && (c.ssim - 1.0).abs() < 1e-12);
        let mut b = a.clone();
        b.pixels[100][0] += 0.2;
        let c = compare(&a, &b).unwrap_or(Comparison {
            max_diff: 0.0,
            at: (0, 0),
            over_tolerance: 0,
            ssim: 1.0,
        });
        assert!(!c.passes() && c.at == (4, 3) && c.over_tolerance == 1);
    }
}
