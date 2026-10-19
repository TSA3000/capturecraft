//! Maximally Stable Extremal Regions (J. Matas, O. Chum, M. Urban, T. Pajdla,
//! *Robust Wide Baseline Stereo from Maximally Stable Extremal Regions*,
//! BMVC 2002): connected sets of pixels all darker (or lighter) than their
//! boundary whose area barely changes over a range of thresholds. Glyphs
//! are such regions even on photos and gradients, where flat-colour
//! segmentation sees only texture.
//!
//! The component tree is built with a union-find over pixels sorted by
//! intensity (Najman & Couprie style); every node remembers its level,
//! area and bounding box.

use photocraft_geom::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MserParams {
    /// Threshold step the stability is measured over.
    pub delta: u8,
    pub min_area: usize,
    pub max_area: usize,
    /// Largest relative area growth over `delta` levels.
    pub max_variation: f32,
}

impl Default for MserParams {
    fn default() -> Self {
        Self {
            delta: 5,
            min_area: 12,
            max_area: 8_000,
            max_variation: 0.5,
        }
    }
}

/// A stable region, local coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub rect: Rect,
    pub area: usize,
    /// Darker than its surroundings (dark text on a light background).
    pub dark: bool,
    pub variation: f32,
}

const NONE: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Node {
    level: u8,
    area: u32,
    bbox: (i32, i32, i32, i32),
    parent: u32,
    /// Absorbed into a node of the same level: not a region of its own.
    dead: bool,
}

/// Dark-on-light and light-on-dark regions of a `w × h` grey image.
/// Images over 4 Mpx are skipped (empty result).
pub fn detect(gray: &[u8], w: usize, h: usize, p: &MserParams) -> Vec<Region> {
    if gray.len() != w * h || w * h == 0 || w * h > 1 << 22 {
        return Vec::new();
    }
    let mut out = detect_one(gray, w, h, p, true);
    let inv: Vec<u8> = gray.iter().map(|v| 255 - v).collect();
    out.extend(detect_one(&inv, w, h, p, false));
    out
}

fn detect_one(gray: &[u8], w: usize, h: usize, p: &MserParams, dark: bool) -> Vec<Region> {
    let n = w * h;
    // Counting sort by intensity, darkest first.
    let mut start = [0usize; 257];
    for &v in gray {
        start[usize::from(v) + 1] += 1;
    }
    for k in 1..257 {
        start[k] += start[k - 1];
    }
    let mut order = vec![0u32; n];
    let mut fill = start;
    for (i, &v) in gray.iter().enumerate() {
        order[fill[usize::from(v)]] = i as u32;
        fill[usize::from(v)] += 1;
    }

    let mut uf: Vec<u32> = vec![NONE; n];
    let mut node_of: Vec<u32> = vec![NONE; n];
    let mut nodes: Vec<Node> = Vec::new();
    let find = |uf: &mut Vec<u32>, mut x: u32| -> u32 {
        while uf[x as usize] != x {
            let gp = uf[uf[x as usize] as usize];
            uf[x as usize] = gp;
            x = gp;
        }
        x
    };
    for &pi in &order {
        let i = pi as usize;
        let level = gray[i];
        let (x, y) = ((i % w) as i32, (i / w) as i32);
        uf[i] = pi;
        nodes.push(Node {
            level,
            area: 1,
            bbox: (x, y, x + 1, y + 1),
            parent: NONE,
            dead: false,
        });
        node_of[i] = (nodes.len() - 1) as u32;
        let neighbours = [
            (x > 0).then(|| i - 1),
            ((x as usize) + 1 < w).then(|| i + 1),
            (y > 0).then(|| i - w),
            ((y as usize) + 1 < h).then(|| i + w),
        ];
        for q in neighbours.into_iter().flatten() {
            if uf[q] == NONE {
                continue;
            }
            let (rp, rq) = (find(&mut uf, pi), find(&mut uf, q as u32));
            if rp == rq {
                continue;
            }
            let (np, nq) = (node_of[rp as usize], node_of[rq as usize]);
            let target = if nodes[np as usize].level == level {
                np
            } else if nodes[nq as usize].level == level {
                nq
            } else {
                nodes.push(Node {
                    level,
                    area: 0,
                    bbox: (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
                    parent: NONE,
                    dead: false,
                });
                (nodes.len() - 1) as u32
            };
            for child in [np, nq] {
                if child == target {
                    continue;
                }
                let c = nodes[child as usize];
                let t = &mut nodes[target as usize];
                t.area += c.area;
                t.bbox = (
                    t.bbox.0.min(c.bbox.0),
                    t.bbox.1.min(c.bbox.1),
                    t.bbox.2.max(c.bbox.2),
                    t.bbox.3.max(c.bbox.3),
                );
                let cn = &mut nodes[child as usize];
                cn.parent = target;
                cn.dead = cn.level == level;
            }
            let root = rp.min(rq);
            uf[rp.max(rq) as usize] = root;
            node_of[root as usize] = target;
        }
    }

    // Stability: area growth from a node to its ancestor `delta` levels up.
    let up = |k: usize| -> usize {
        let lim = u16::from(nodes[k].level) + u16::from(p.delta);
        let mut a = k;
        loop {
            let par = nodes[a].parent;
            if par == NONE || u16::from(nodes[par as usize].level) > lim {
                return a;
            }
            a = par as usize;
        }
    };
    let var: Vec<f32> = (0..nodes.len())
        .map(|k| {
            let a = up(k);
            (nodes[a].area as f32 - nodes[k].area as f32) / nodes[k].area.max(1) as f32
        })
        .collect();
    // Local minimum along the branch: not more variable than the parent
    // (or than any child, checked through the children's parent link).
    let mut child_min = vec![f32::INFINITY; nodes.len()];
    for (k, nd) in nodes.iter().enumerate() {
        if nd.parent != NONE && !nd.dead {
            let c = &mut child_min[nd.parent as usize];
            *c = c.min(var[k]);
        }
    }
    let mut out = Vec::new();
    for (k, nd) in nodes.iter().enumerate() {
        let area = nd.area as usize;
        // The root is the whole image, not a region of it.
        if nd.dead || nd.parent == NONE {
            continue;
        }
        if area < p.min_area || area > p.max_area || var[k] > p.max_variation {
            continue;
        }
        let parent_var = var[nd.parent as usize];
        if var[k] <= parent_var && var[k] <= child_min[k] {
            let (x0, y0, x1, y1) = nd.bbox;
            // Spanning the whole image: the background, not a region in it.
            if x0 == 0 && y0 == 0 && x1 as usize == w && y1 as usize == h {
                continue;
            }
            out.push(Region {
                rect: Rect::new(x0, y0, x1, y1),
                area,
                dark,
                variation: var[k],
            });
        }
    }
    out.sort_by_key(|r| (r.rect.y0, r.rect.x0, r.rect.y1, r.rect.x1));
    out.dedup_by(|a, b| a.rect == b.rect);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_dark_and_light_blobs() {
        let (w, h) = (80, 40);
        // Mid-grey noise-free field with a dark and a light square.
        let mut g = vec![128u8; w * h];
        for y in 10..20 {
            for x in 10..20 {
                g[y * w + x] = 20;
            }
            for x in 50..60 {
                g[y * w + x] = 240;
            }
        }
        let r = detect(&g, w, h, &MserParams::default());
        let dark = r.iter().find(|r| r.dark).map(|r| r.rect);
        let light = r.iter().find(|r| !r.dark).map(|r| r.rect);
        assert_eq!(dark, Some(Rect::new(10, 10, 20, 20)));
        assert_eq!(light, Some(Rect::new(50, 10, 60, 20)));
    }
}
