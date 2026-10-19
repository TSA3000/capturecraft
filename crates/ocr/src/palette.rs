//! Auto-extracted Simplify palettes: the screenshot quantized to six
//! colours (`photocraft-algo`'s perceptual median cut + k-means), then each
//! swatch given a role by frequency, lightness and saturation.

use capturecraft_engine::SimplifyPalette;
use capturecraft_format::Color;
use photocraft_algo::quantize::{Forced, PaletteKind, build_palette};

use crate::image::{Px, RgbaImage, luma, saturation};

/// Pixels sampled at most (a regular grid over the image).
const MAX_SAMPLES: usize = 1 << 16;

fn color(p: Px) -> Color {
    Color::rgba(p[0], p[1], p[2], 255)
}

fn mix(a: Px, b: Px, t: f32) -> Px {
    std::array::from_fn(|c| {
        (f32::from(a[c]) + (f32::from(b[c]) - f32::from(a[c])) * t).round() as u8
    })
}

fn dist2(a: [u8; 3], p: Px) -> u32 {
    (0..3).map(|c| u32::from(a[c].abs_diff(p[c])).pow(2)).sum()
}

/// Background, surface, accent, muted text, border and on-accent colours
/// for the pixels of `img`.
pub fn extract(img: &RgbaImage) -> Result<SimplifyPalette, String> {
    let px = img.pixels();
    if px.is_empty() {
        return Err("empty image".into());
    }
    let step = px.len().div_ceil(MAX_SAMPLES).max(1);
    let samples: Vec<Px> = px
        .iter()
        .step_by(step)
        .copied()
        .filter(|p| p[3] > 0)
        .collect();
    if samples.is_empty() {
        return Err("the image is transparent".into());
    }
    let f: Vec<[f32; 4]> = samples
        .iter()
        .map(|p| {
            [
                f32::from(p[0]) / 255.0,
                f32::from(p[1]) / 255.0,
                f32::from(p[2]) / 255.0,
                1.0,
            ]
        })
        .collect();
    let pal = build_palette(&f, PaletteKind::Perceptual, 6, Forced::None)?;
    let mut count = vec![0usize; pal.len()];
    for p in &samples {
        if let Some((k, _)) = pal.iter().enumerate().min_by_key(|(_, c)| dist2(**c, *p)) {
            count[k] += 1;
        }
    }
    let mut sw: Vec<(Px, usize)> = pal
        .iter()
        .zip(count)
        .filter(|(_, n)| *n > 0)
        .map(|(c, n)| ([c[0], c[1], c[2], 255], n))
        .collect();
    // Most common first; ties by colour so the order never varies.
    sw.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let total = samples.len();
    let bg = sw.first().map_or([255; 4], |s| s.0);
    let dark = luma(bg) < 128;
    let lum = |p: Px| i32::from(luma(p));
    let share = |n: usize| n as f32 / total as f32;

    let surface = sw
        .iter()
        .skip(1)
        .filter(|(c, n)| {
            saturation(*c) < 0.2 && share(*n) >= 0.02 && (lum(*c) - lum(bg)).abs() <= 40
        })
        .min_by_key(|(c, _)| (lum(*c) - lum(bg)).abs())
        .map(|s| s.0)
        .unwrap_or_else(|| {
            if dark {
                mix(bg, [255; 4], 0.06)
            } else {
                mix(bg, [0, 0, 0, 255], 0.03)
            }
        });
    let accent = sw
        .iter()
        .filter(|(c, n)| saturation(*c) >= 0.25 && share(*n) >= 0.003 && luma(*c) > 24)
        .max_by(|a, b| {
            let score = |s: &(Px, usize)| saturation(s.0) * share(s.1).sqrt();
            score(a).total_cmp(&score(b))
        })
        .map_or([0x3B, 0x82, 0xF6, 255], |s| s.0);
    let ink = sw
        .iter()
        .filter(|(c, _)| saturation(*c) < 0.3)
        .max_by_key(|(c, _)| (lum(*c) - lum(bg)).abs())
        .map(|s| s.0)
        .filter(|c| (lum(*c) - lum(bg)).abs() >= 48)
        .unwrap_or(if dark {
            [0xE0, 0xE0, 0xE0, 255]
        } else {
            [0x30, 0x30, 0x30, 255]
        });
    let muted = mix(ink, bg, 0.45);
    let on_accent = if luma(accent) < 150 {
        [255; 4]
    } else {
        [0x10, 0x10, 0x10, 255]
    };
    Ok(SimplifyPalette {
        background: color(bg),
        surface: color(surface),
        accent: color(accent),
        muted_text: color(muted),
        border: color(mix(bg, ink, 0.25)),
        on_accent: color(on_accent),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use photocraft_geom::Rect;

    #[test]
    fn dark_ui_gets_dark_roles() {
        let (w, h) = (100, 100);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let p: Px = if (40..60).contains(&y) && (20..80).contains(&x) {
                    [88, 101, 242, 255]
                } else if y < 20 {
                    [56, 58, 64, 255]
                } else if (70..74).contains(&y) && x < 70 {
                    [220, 221, 222, 255]
                } else {
                    [43, 45, 49, 255]
                };
                rgba.extend(p);
            }
        }
        let img = RgbaImage::from_rgba8(Rect::new(0, 0, w, h), &rgba).unwrap();
        let p = extract(&img).unwrap();
        assert!(luma([p.background.r, p.background.g, p.background.b, 255]) < 64);
        assert!(p.accent.b > p.accent.r + 80, "{p:?}");
        assert_eq!(p.on_accent, Color::WHITE);
        assert!(p.muted_text.r > p.background.r);
    }
}
