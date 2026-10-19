//! Connected-component labelling, one task per 256² raster tile: each tile
//! is labelled with a local union-find (two-pass), then the labels are
//! merged across the tile seams and numbered in raster order, so the result
//! does not depend on the thread count.

use rayon::prelude::*;

use crate::image::tile_spans;

/// No label (an inactive pixel).
pub const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conn {
    Four,
    Eight,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Labels {
    pub width: usize,
    pub height: usize,
    /// Per pixel, row-major: `0..count` or [`NONE`].
    pub labels: Vec<u32>,
    pub count: usize,
}

struct Uf(Vec<u32>);

impl Uf {
    fn find(&mut self, mut x: u32) -> u32 {
        while let Some(&p) = self.0.get(x as usize) {
            if p == x {
                break;
            }
            let gp = self.0.get(p as usize).copied().unwrap_or(p);
            if let Some(slot) = self.0.get_mut(x as usize) {
                *slot = gp;
            }
            x = p;
        }
        x
    }

    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = (ra.min(rb), ra.max(rb));
            if let Some(slot) = self.0.get_mut(hi as usize) {
                *slot = lo;
            }
        }
    }

    fn push(&mut self) -> u32 {
        let id = self.0.len() as u32;
        self.0.push(id);
        id
    }
}

/// Labels the `width × height` grid (whose top-left sits at `origin` in
/// base space, so tiles follow the raster grid): pixels where `active` holds
/// are joined to `conn` neighbours `j` with `same(i, j)`.
pub fn label<A, S>(
    width: usize,
    height: usize,
    origin: (i32, i32),
    conn: Conn,
    active: A,
    same: S,
) -> Labels
where
    A: Fn(usize) -> bool + Sync,
    S: Fn(usize, usize) -> bool + Sync,
{
    let n = width * height;
    if n == 0 {
        return Labels {
            width,
            height,
            labels: Vec::new(),
            count: 0,
        };
    }
    let local_spans = |o: i32, len: usize| -> Vec<(usize, usize)> {
        tile_spans(o, o + len as i32)
            .into_iter()
            .map(|(a, b)| ((a - o) as usize, (b - o) as usize))
            .collect()
    };
    let xs = local_spans(origin.0, width);
    let ys = local_spans(origin.1, height);
    let blocks: Vec<(usize, usize, usize, usize)> = ys
        .iter()
        .flat_map(|&(y0, y1)| xs.iter().map(move |&(x0, x1)| (x0, x1, y0, y1)))
        .collect();
    let locals: Vec<(Vec<u32>, u32)> = blocks
        .par_iter()
        .map(|&(x0, x1, y0, y1)| label_block(width, (x0, x1, y0, y1), conn, &active, &same))
        .collect();
    let mut offsets = Vec::with_capacity(locals.len());
    let mut total: u32 = 0;
    for (_, c) in &locals {
        offsets.push(total);
        total = total.saturating_add(*c);
    }
    let mut col_of = vec![0usize; width];
    for (k, &(a, b)) in xs.iter().enumerate() {
        col_of[a..b].fill(k);
    }
    let mut row_of = vec![0usize; height];
    for (k, &(a, b)) in ys.iter().enumerate() {
        row_of[a..b].fill(k);
    }
    let ncols = xs.len();
    let global = |x: usize, y: usize| -> u32 {
        let b = row_of[y] * ncols + col_of[x];
        let (x0, x1, y0, _) = blocks[b];
        let l = locals[b].0[(y - y0) * (x1 - x0) + (x - x0)];
        if l == NONE { NONE } else { offsets[b] + l }
    };
    let mut uf = Uf((0..total).collect());
    let join = |uf: &mut Uf, (ax, ay): (usize, usize), (bx, by): (usize, usize)| {
        let (i, j) = (ay * width + ax, by * width + bx);
        if active(i) && active(j) && same(i, j) {
            let (a, b) = (global(ax, ay), global(bx, by));
            if a != NONE && b != NONE {
                uf.union(a, b);
            }
        }
    };
    // Seams between tile columns, then between tile rows.
    for &(xb, _) in xs.iter().skip(1) {
        for y in 0..height {
            join(&mut uf, (xb - 1, y), (xb, y));
            if conn == Conn::Eight {
                if y > 0 {
                    join(&mut uf, (xb - 1, y - 1), (xb, y));
                }
                if y + 1 < height {
                    join(&mut uf, (xb - 1, y + 1), (xb, y));
                }
            }
        }
    }
    for &(yb, _) in ys.iter().skip(1) {
        for x in 0..width {
            join(&mut uf, (x, yb - 1), (x, yb));
            if conn == Conn::Eight {
                if x > 0 {
                    join(&mut uf, (x - 1, yb - 1), (x, yb));
                }
                if x + 1 < width {
                    join(&mut uf, (x + 1, yb - 1), (x, yb));
                }
            }
        }
    }
    let mut compact = vec![NONE; total as usize];
    let mut next = 0u32;
    let mut labels = vec![NONE; n];
    for y in 0..height {
        for x in 0..width {
            let g = global(x, y);
            if g == NONE {
                continue;
            }
            let r = uf.find(g) as usize;
            if compact[r] == NONE {
                compact[r] = next;
                next += 1;
            }
            labels[y * width + x] = compact[r];
        }
    }
    Labels {
        width,
        height,
        labels,
        count: next as usize,
    }
}

/// One tile: labels `0..count` local to the block (row-major in the block).
fn label_block<A, S>(
    width: usize,
    (x0, x1, y0, y1): (usize, usize, usize, usize),
    conn: Conn,
    active: &A,
    same: &S,
) -> (Vec<u32>, u32)
where
    A: Fn(usize) -> bool + Sync,
    S: Fn(usize, usize) -> bool + Sync,
{
    let bw = x1 - x0;
    let mut lab = vec![NONE; bw * (y1 - y0)];
    let mut uf = Uf(Vec::new());
    for y in y0..y1 {
        for x in x0..x1 {
            let i = y * width + x;
            if !active(i) {
                continue;
            }
            let mut cur = NONE;
            let look = |nx: usize, ny: usize, cur: &mut u32, uf: &mut Uf, lab: &[u32]| {
                let lj = lab[(ny - y0) * bw + (nx - x0)];
                if lj != NONE && same(i, ny * width + nx) {
                    if *cur == NONE {
                        *cur = lj;
                    } else {
                        uf.union(*cur, lj);
                    }
                }
            };
            if x > x0 {
                look(x - 1, y, &mut cur, &mut uf, &lab);
            }
            if y > y0 {
                look(x, y - 1, &mut cur, &mut uf, &lab);
                if conn == Conn::Eight {
                    if x > x0 {
                        look(x - 1, y - 1, &mut cur, &mut uf, &lab);
                    }
                    if x + 1 < x1 {
                        look(x + 1, y - 1, &mut cur, &mut uf, &lab);
                    }
                }
            }
            if cur == NONE {
                cur = uf.push();
            }
            lab[(y - y0) * bw + (x - x0)] = cur;
        }
    }
    let mut compact = vec![NONE; uf.0.len()];
    let mut next = 0u32;
    for l in &mut lab {
        if *l == NONE {
            continue;
        }
        let r = uf.find(*l) as usize;
        if compact[r] == NONE {
            compact[r] = next;
            next += 1;
        }
        *l = compact[r];
    }
    (lab, next)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Reference: flood fill.
    fn flood(w: usize, h: usize, on: &[bool], conn: Conn) -> usize {
        let mut seen = vec![false; w * h];
        let mut n = 0;
        for s in 0..w * h {
            if !on[s] || seen[s] {
                continue;
            }
            n += 1;
            let mut stack = vec![s];
            seen[s] = true;
            while let Some(i) = stack.pop() {
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                for (dx, dy) in [
                    (-1, 0),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (1, -1),
                    (-1, 1),
                    (1, 1),
                ] {
                    if conn == Conn::Four && dx != 0 && dy != 0 {
                        continue;
                    }
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if on[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        n
    }

    #[test]
    fn matches_flood_fill_across_tile_seams() {
        let (w, h) = (600, 530);
        // Pseudo-random blobs plus lines crossing every seam.
        let mut on = vec![false; w * h];
        let mut s: u64 = 7;
        for (i, v) in on.iter_mut().enumerate() {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let (x, y) = (i % w, i / w);
            *v = (s >> 33) % 7 < 3 || y == 300 || x == 255 || x + y == 520;
        }
        for conn in [Conn::Four, Conn::Eight] {
            for origin in [(0, 0), (100, 37)] {
                let l = label(w, h, origin, conn, |i| on[i], |_, _| true);
                assert_eq!(l.count, flood(w, h, &on, conn), "{conn:?} {origin:?}");
                // Raster-order numbering: the first labelled pixel is 0.
                let first = l.labels.iter().find(|&&v| v != NONE).copied();
                assert_eq!(first, Some(0));
            }
        }
    }

    #[test]
    fn joins_similar_colours_only() {
        let (w, h) = (300, 2);
        let px: Vec<u8> = (0..w * h)
            .map(|i| if i % w < 150 { 10 } else { 200 })
            .collect();
        let l = label(
            w,
            h,
            (0, 0),
            Conn::Four,
            |_| true,
            |a, b| px[a].abs_diff(px[b]) < 5,
        );
        assert_eq!(l.count, 2);
    }
}
