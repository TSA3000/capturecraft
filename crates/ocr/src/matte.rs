//! Soft alpha matting of a lifted object against the flat colour it sat on
//! (Smart Move): anti-aliased edges and rounded corners come out partly
//! transparent instead of carrying a fringe of the old background.

use crate::image::{Px, RgbaImage, dist, mode, ring_pixels};

/// Straight-alpha RGBA8 of `img` with the background `bg` keyed out.
///
/// Pixels within `lo` (max-channel distance) of `bg` become transparent and
/// pixels away from the background stay opaque, so multi-colour icons keep
/// their faint parts. Pixels on the boundary (next to background) are the
/// anti-aliased mix `P = α·F + (1 − α)·B`: α is their distance from `bg`
/// over that of the most object-like neighbour, and the colour is
/// un-blended to `F = B + (P − B) / α`.
pub fn matte(img: &RgbaImage, bg: Px) -> Vec<u8> {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let d: Vec<u8> = img.pixels().iter().map(|p| dist(*p, bg)).collect();
    let far = d.iter().copied().max().unwrap_or(0);
    let lo = (far / 4).min(6);
    let at = |x: i32, y: i32| -> Option<u8> {
        (x >= 0 && y >= 0 && x < w && y < h).then(|| d[(y * w + x) as usize])
    };
    let mut out = Vec::with_capacity(img.pixels().len() * 4);
    for (i, &p) in img.pixels().iter().enumerate() {
        let (x, y) = (i as i32 % w, i as i32 / w);
        let di = d[i];
        if di <= lo {
            out.extend([0, 0, 0, 0]);
            continue;
        }
        let mut edge = false;
        let mut strongest = di;
        for dy in -1..=1 {
            for dx in -1..=1 {
                if let Some(v) = at(x + dx, y + dy) {
                    edge |= v <= lo;
                    strongest = strongest.max(v);
                }
            }
        }
        if !edge {
            out.extend(p);
            continue;
        }
        let a = (f32::from(di) / f32::from(strongest.max(1))).clamp(0.0, 1.0);
        let mut px = [0u8; 4];
        for c in 0..3 {
            let (pv, bv) = (f32::from(p[c]), f32::from(bg[c]));
            px[c] = (bv + (pv - bv) / a).round().clamp(0.0, 255.0) as u8;
        }
        px[3] = (a * f32::from(p[3])).round().clamp(0.0, 255.0) as u8;
        out.extend(px);
    }
    out
}

/// The background around a local rectangle: the most common colour of the
/// 2 px band outside it.
pub fn surrounding(img: &RgbaImage, r: photocraft_geom::Rect) -> Option<Px> {
    mode(ring_pixels(img, r, 2, |_, _| false).into_iter())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use photocraft_geom::Rect;

    #[test]
    fn keys_out_the_background_and_unblends_edges() {
        let bg = [30, 30, 30, 255];
        let fill = [88, 101, 242, 255];
        // bg | 50 % blend | fill
        let half = [59, 66, 136, 255];
        let rgba: Vec<u8> = [bg, half, fill].into_iter().flatten().collect();
        let img = RgbaImage::from_rgba8(Rect::new(0, 0, 3, 1), &rgba).unwrap();
        let m = matte(&img, bg);
        assert_eq!(m[3], 0);
        assert_eq!(&m[8..12], &fill);
        // The edge pixel is half transparent with the object's own colour.
        assert!((100..=160).contains(&m[7]), "{m:?}");
        assert!(
            m[4..7].iter().zip(&fill).all(|(a, b)| a.abs_diff(*b) <= 12),
            "{m:?}"
        );
    }
}
