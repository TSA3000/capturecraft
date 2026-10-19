//! Exact boolean union of flattened polygons.
//!
//! photocraft-vector combines paths at coverage level (its rasterizer folds
//! components with path operations), which is enough to *fill* a union but
//! not to *stroke* one: stroking every input polygon leaves seam lines
//! wherever they overlap. Callouts and clouds need the merged outline
//! itself, so this module computes it from the edge arrangement:
//!
//! 1. every edge is split at every intersection with every other edge
//!    (collinear overlaps split at the other edge's end points); a split
//!    point is computed once and shared by both edges, so the pieces meet
//!    bit-exactly;
//! 2. a piece lies on the union's boundary when the union (non-zero per
//!    input polygon, OR across polygons) is inside on exactly one side of
//!    it; boundary pieces are kept, oriented with the inside on their left;
//! 3. kept pieces are linked end to start into closed loops, turning
//!    towards the inside at vertices where loops touch, so every loop is
//!    simple.
//!
//! Outer loops and holes come out with opposite orientations, so the result
//! fills correctly with the non-zero rule. Knot flags of the input
//! polylines are carried over (intersection points are knots: stroke joins
//! apply there).

use std::collections::HashMap;

use photocraft_vector::Polyline;

type P = (f64, f64);

#[derive(Clone, Copy)]
struct Edge {
    a: P,
    b: P,
    ka: bool,
    kb: bool,
}

impl Edge {
    fn min_x(&self) -> f64 {
        self.a.0.min(self.b.0)
    }
    fn max_x(&self) -> f64 {
        self.a.0.max(self.b.0)
    }
    fn min_y(&self) -> f64 {
        self.a.1.min(self.b.1)
    }
    fn max_y(&self) -> f64 {
        self.a.1.max(self.b.1)
    }
}

fn sub(a: P, b: P) -> P {
    (a.0 - b.0, a.1 - b.1)
}
fn cross(a: P, b: P) -> f64 {
    a.0 * b.1 - a.1 * b.0
}
fn dot(a: P, b: P) -> f64 {
    a.0 * b.0 + a.1 * b.1
}
fn finite(p: P) -> bool {
    p.0.is_finite() && p.1.is_finite()
}

fn key(p: P) -> (u64, u64) {
    // +0.0 and -0.0 are the same point.
    ((p.0 + 0.0).to_bits(), (p.1 + 0.0).to_bits())
}

/// Splits a point is inserted at, by edge parameter.
#[derive(Clone, Copy)]
struct Split {
    t: f64,
    p: P,
}

const T_EPS: f64 = 1e-9;

fn interior(t: f64) -> bool {
    t > T_EPS && t < 1.0 - T_EPS
}

/// Parameter of `p` projected on `e` (no clamping).
fn param(e: &Edge, p: P) -> f64 {
    let r = sub(e.b, e.a);
    let l2 = dot(r, r);
    if l2 > 0.0 {
        dot(sub(p, e.a), r) / l2
    } else {
        0.0
    }
}

fn intersect(e: &Edge, f: &Edge, se: &mut Vec<Split>, sf: &mut Vec<Split>) {
    let r = sub(e.b, e.a);
    let s = sub(f.b, f.a);
    let qp = sub(f.a, e.a);
    let denom = cross(r, s);
    let (lr, ls) = (dot(r, r).sqrt(), dot(s, s).sqrt());
    if lr == 0.0 || ls == 0.0 {
        return;
    }
    if denom.abs() > 1e-12 * lr * ls {
        let t = cross(qp, s) / denom;
        let u = cross(qp, r) / denom;
        let tol = 1e-12;
        if !(-tol..=1.0 + tol).contains(&t) || !(-tol..=1.0 + tol).contains(&u) {
            return;
        }
        // Prefer an existing end point so pieces share it exactly.
        let p = if t <= T_EPS {
            e.a
        } else if t >= 1.0 - T_EPS {
            e.b
        } else if u <= T_EPS {
            f.a
        } else if u >= 1.0 - T_EPS {
            f.b
        } else {
            (e.a.0 + r.0 * t, e.a.1 + r.1 * t)
        };
        if interior(t) {
            se.push(Split { t, p });
        }
        if interior(u) {
            sf.push(Split { t: u, p });
        }
        return;
    }
    // Parallel: only collinear overlaps matter.
    if cross(qp, r).abs() > 1e-9 * lr {
        return;
    }
    for p in [f.a, f.b] {
        let t = param(e, p);
        if interior(t) {
            se.push(Split { t, p });
        }
    }
    for p in [e.a, e.b] {
        let u = param(f, p);
        if interior(u) {
            sf.push(Split { t: u, p });
        }
    }
}

/// Winding number of `poly` around `q`.
fn winding(poly: &[P], q: P) -> i32 {
    let n = poly.len();
    let mut w = 0;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if a.1 <= q.1 {
            if b.1 > q.1 && cross(sub(b, a), sub(q, a)) > 0.0 {
                w += 1;
            }
        } else if b.1 <= q.1 && cross(sub(b, a), sub(q, a)) < 0.0 {
            w -= 1;
        }
    }
    w
}

struct Input<'a> {
    pts: &'a [P],
    bounds: (f64, f64, f64, f64),
}

fn inside(inputs: &[Input<'_>], q: P) -> bool {
    inputs.iter().any(|i| {
        let (x0, y0, x1, y1) = i.bounds;
        q.0 >= x0 && q.0 <= x1 && q.1 >= y0 && q.1 <= y1 && winding(i.pts, q) != 0
    })
}

/// The union of `polys` (closed polylines, non-zero fill each) as closed,
/// simple loops.
pub fn union(polys: &[Polyline]) -> Vec<Polyline> {
    let mut inputs = Vec::new();
    let mut edges = Vec::new();
    for pl in polys {
        if pl.pts.len() < 3 || !pl.pts.iter().all(|p| finite(*p)) {
            continue;
        }
        let n = pl.pts.len();
        let mut b = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for (i, &a) in pl.pts.iter().enumerate() {
            b = (b.0.min(a.0), b.1.min(a.1), b.2.max(a.0), b.3.max(a.1));
            let j = (i + 1) % n;
            let c = pl.pts[j];
            if key(a) == key(c) {
                continue;
            }
            let k = |idx: usize| pl.knot.get(idx).copied().unwrap_or(true);
            edges.push(Edge {
                a,
                b: c,
                ka: k(i),
                kb: k(j),
            });
        }
        inputs.push(Input {
            pts: &pl.pts,
            bounds: b,
        });
    }
    if edges.is_empty() {
        return Vec::new();
    }
    let scale = inputs.iter().fold(1.0f64, |s, i| {
        s.max((i.bounds.2 - i.bounds.0).abs())
            .max((i.bounds.3 - i.bounds.1).abs())
    });

    // 1. Split at intersections (sweep over x to prune pairs).
    let mut order: Vec<usize> = (0..edges.len()).collect();
    order.sort_by(|&i, &j| edges[i].min_x().total_cmp(&edges[j].min_x()));
    let mut splits: Vec<Vec<Split>> = vec![Vec::new(); edges.len()];
    for (oi, &i) in order.iter().enumerate() {
        let e = edges[i];
        for &j in &order[oi + 1..] {
            let f = edges[j];
            if f.min_x() > e.max_x() {
                break;
            }
            if f.min_y() > e.max_y() || f.max_y() < e.min_y() {
                continue;
            }
            let (mut se, mut sf) = (Vec::new(), Vec::new());
            intersect(&e, &f, &mut se, &mut sf);
            splits[i].extend(se);
            splits[j].extend(sf);
        }
    }

    // 2. Pieces on the boundary, inside on the left.
    let mut knots: HashMap<(u64, u64), bool> = HashMap::new();
    let mut kept: Vec<(P, P)> = Vec::new();
    let eps = 1e-8 * scale;
    for (e, sp) in edges.iter().zip(splits.iter_mut()) {
        sp.sort_by(|a, b| a.t.total_cmp(&b.t));
        let mut chain = Vec::with_capacity(sp.len() + 2);
        chain.push(e.a);
        *knots.entry(key(e.a)).or_default() |= e.ka;
        for s in sp.iter() {
            if chain.last().is_some_and(|l| key(*l) == key(s.p)) {
                continue;
            }
            chain.push(s.p);
            knots.insert(key(s.p), true);
        }
        if chain.last().is_some_and(|l| key(*l) == key(e.b)) {
            chain.pop();
        }
        chain.push(e.b);
        *knots.entry(key(e.b)).or_default() |= e.kb;
        for w in chain.windows(2) {
            let (p, q) = (w[0], w[1]);
            let d = sub(q, p);
            let len = dot(d, d).sqrt();
            if len == 0.0 {
                continue;
            }
            let m = ((p.0 + q.0) * 0.5, (p.1 + q.1) * 0.5);
            let n = (-d.1 / len * eps, d.0 / len * eps);
            let left = inside(&inputs, (m.0 + n.0, m.1 + n.1));
            let right = inside(&inputs, (m.0 - n.0, m.1 - n.1));
            match (left, right) {
                (true, false) => kept.push((p, q)),
                (false, true) => kept.push((q, p)),
                _ => {}
            }
        }
    }
    // Coincident pieces of overlapping collinear edges.
    let mut seen = std::collections::HashSet::new();
    kept.retain(|(p, q)| seen.insert((key(*p), key(*q))));

    // 3. Link into loops.
    let mut from: HashMap<(u64, u64), Vec<usize>> = HashMap::new();
    for (i, (p, _)) in kept.iter().enumerate() {
        from.entry(key(*p)).or_default().push(i);
    }
    let mut used = vec![false; kept.len()];
    let mut out = Vec::new();
    for start in 0..kept.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let origin = key(kept[start].0);
        let mut pts = vec![kept[start].0];
        let mut cur = start;
        let mut closed = false;
        for _ in 0..kept.len() {
            let (p, q) = kept[cur];
            if key(q) == origin {
                closed = true;
                break;
            }
            pts.push(q);
            let d = sub(q, p);
            let next = from.get(&key(q)).and_then(|c| {
                c.iter().copied().filter(|&i| !used[i]).max_by(|&i, &j| {
                    let turn = |k: usize| {
                        let c = sub(kept[k].1, kept[k].0);
                        cross(d, c).atan2(dot(d, c))
                    };
                    turn(i).total_cmp(&turn(j))
                })
            });
            let Some(next) = next else { break };
            used[next] = true;
            cur = next;
        }
        if !closed || pts.len() < 3 {
            continue;
        }
        let area: f64 = (0..pts.len())
            .map(|i| cross(pts[i], pts[(i + 1) % pts.len()]))
            .sum::<f64>()
            * 0.5;
        if area.abs() < 1e-9 {
            continue;
        }
        let knot = pts
            .iter()
            .map(|p| knots.get(&key(*p)).copied().unwrap_or(true))
            .collect();
        out.push(Polyline {
            pts,
            knot,
            closed: true,
        });
    }
    out
}

/// Signed area of a closed polygon (positive = clockwise on screen).
pub fn signed_area(pts: &[P]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| cross(pts[i], pts[(i + 1) % n])).sum::<f64>() * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, s: f64) -> Polyline {
        Polyline {
            pts: vec![(x, y), (x + s, y), (x + s, y + s), (x, y + s)],
            knot: vec![true; 4],
            closed: true,
        }
    }

    fn total_area(loops: &[Polyline]) -> f64 {
        loops.iter().map(|l| signed_area(&l.pts)).sum()
    }

    #[test]
    fn overlapping_squares_merge_into_one_loop() {
        let u = union(&[square(0.0, 0.0, 10.0), square(5.0, 5.0, 10.0)]);
        assert_eq!(u.len(), 1);
        assert!((total_area(&u).abs() - 175.0).abs() < 1e-9);
        // Two intersection points become corners: 8 vertices.
        assert_eq!(u[0].pts.len(), 8);
    }

    #[test]
    fn contained_and_disjoint_inputs() {
        let u = union(&[square(0.0, 0.0, 10.0), square(2.0, 2.0, 3.0)]);
        assert_eq!(u.len(), 1);
        assert!((total_area(&u).abs() - 100.0).abs() < 1e-9);
        let u = union(&[square(0.0, 0.0, 1.0), square(5.0, 0.0, 1.0)]);
        assert_eq!(u.len(), 2);
    }

    #[test]
    fn shared_edges_and_reversed_orientation() {
        let mut b = square(10.0, 0.0, 10.0);
        b.pts.reverse();
        let u = union(&[square(0.0, 0.0, 10.0), b]);
        assert_eq!(u.len(), 1);
        assert!((total_area(&u).abs() - 200.0).abs() < 1e-9);
    }

    #[test]
    fn ring_keeps_its_hole() {
        // A U and a bar closing it leave a hole.
        let u_shape = Polyline {
            pts: vec![
                (0.0, 0.0),
                (30.0, 0.0),
                (30.0, 30.0),
                (20.0, 30.0),
                (20.0, 10.0),
                (10.0, 10.0),
                (10.0, 30.0),
                (0.0, 30.0),
            ],
            knot: vec![true; 8],
            closed: true,
        };
        let bar = Polyline {
            pts: vec![(0.0, 25.0), (30.0, 25.0), (30.0, 35.0), (0.0, 35.0)],
            knot: vec![true; 4],
            closed: true,
        };
        let u = union(&[u_shape, bar]);
        assert_eq!(u.len(), 2, "outer loop + hole");
        let a = total_area(&u).abs();
        // 30x35 bounds minus the 10x15 hole left in the slot.
        assert!((a - (1050.0 - 150.0)).abs() < 1e-6, "{a}");
        assert!(signed_area(&u[0].pts).signum() != signed_area(&u[1].pts).signum());
    }
}
