//! Binary morphology with rectangular structuring elements, by running
//! sums (cost independent of the radius).

use rayon::prelude::*;

/// Dilation then erosion: joins set pixels less than `2·rx + 1` apart
/// horizontally (and `2·ry + 1` vertically) without growing the outline.
pub fn close(mask: &[bool], w: usize, h: usize, rx: usize, ry: usize) -> Vec<bool> {
    if mask.len() != w * h || w == 0 || h == 0 {
        return mask.to_vec();
    }
    let d = pass_v(&pass_h(mask, w, rx, Op::Dilate), w, h, ry, Op::Dilate);
    pass_v(&pass_h(&d, w, rx, Op::Erode), w, h, ry, Op::Erode)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Dilate,
    /// Pixels outside the grid count as set, so erosion does not eat the
    /// border (closing stays extensive).
    Erode,
}

fn decide(op: Op, set: u32, window: u32) -> bool {
    match op {
        Op::Dilate => set > 0,
        Op::Erode => set == window,
    }
}

fn pass_h(src: &[bool], w: usize, r: usize, op: Op) -> Vec<bool> {
    let mut out = vec![false; src.len()];
    out.par_chunks_mut(w)
        .zip(src.par_chunks(w))
        .for_each(|(o, row)| {
            let mut pre = vec![0u32; w + 1];
            for (k, &v) in row.iter().enumerate() {
                pre[k + 1] = pre[k] + u32::from(v);
            }
            for (x, slot) in o.iter_mut().enumerate() {
                let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
                *slot = decide(op, pre[b] - pre[a], (b - a) as u32);
            }
        });
    out
}

fn pass_v(src: &[bool], w: usize, h: usize, r: usize, op: Op) -> Vec<bool> {
    if r == 0 {
        return src.to_vec();
    }
    // Column prefix sums, row by row.
    let mut pre = vec![0u32; (h + 1) * w];
    for y in 0..h {
        for x in 0..w {
            pre[(y + 1) * w + x] = pre[y * w + x] + u32::from(src[y * w + x]);
        }
    }
    let mut out = vec![false; src.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, o)| {
        let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
        for (x, slot) in o.iter_mut().enumerate() {
            *slot = decide(op, pre[b * w + x] - pre[a * w + x], (b - a) as u32);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_bridges_small_gaps_only() {
        // ##..##.....##  (gap of 2, then gap of 5)
        let row = "##..##.....##";
        let m: Vec<bool> = row.chars().map(|c| c == '#').collect();
        let c = close(&m, m.len(), 1, 1, 0);
        let s: String = c.iter().map(|&b| if b { '#' } else { '.' }).collect();
        assert_eq!(s, "######.....##");
        // Never shrinks: every set pixel stays set.
        assert!(m.iter().zip(&c).all(|(a, b)| !a || *b));
    }
}
