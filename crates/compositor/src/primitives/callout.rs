//! Callouts: a body (rounded rectangle, ellipse, cloud, banner) with any
//! number of draggable tails, merged into one outline.
//!
//! The body contour and every tail polygon are united with the exact
//! boolean union ([`super::boolean::union`]) *before* the stroke and the
//! shadow are built. The stroke therefore follows only the merged
//! perimeter (no seam where a tail meets the body) and the drop shadow is
//! cast by one silhouette.

use photocraft_doc::{Knot, LineCap, Path, Subpath};
use photocraft_geom::Point;
use photocraft_vector::{Polyline, flatten_path, flatten_subpath, shapes};

use super::{P, Silhouette, TOLERANCE, boolean, point_ok, stroke};
use crate::scene::{CalloutBody, CalloutSpec, CalloutTail, RectF32, TailAnchor};

/// Most puffs a cloud may have.
pub const MAX_PUFFS: u32 = 64;
/// Most tails a callout may have.
pub const MAX_TAILS: usize = 32;

fn rect_ok(r: &RectF32) -> bool {
    r.is_valid()
        && point_ok(crate::scene::PointF32::new(r.x, r.y))
        && point_ok(crate::scene::PointF32::new(r.w, r.h))
}

fn ellipse_loop(cx: f64, cy: f64, rx: f64, ry: f64) -> Vec<Polyline> {
    flatten_path(
        &shapes::ellipse(cx - rx, cy - ry, rx * 2.0, ry * 2.0),
        TOLERANCE,
    )
}

/// Contour loops of the callout body (before the union).
pub fn body_loops(body: &CalloutBody) -> Vec<Polyline> {
    let r = body.rect();
    if !rect_ok(&r) {
        return Vec::new();
    }
    let (x, y, w, h) = (
        f64::from(r.x),
        f64::from(r.y),
        f64::from(r.w),
        f64::from(r.h),
    );
    match body {
        CalloutBody::RoundedRect { radius, .. } => {
            let rad = if radius.is_finite() {
                f64::from(radius.max(0.0))
            } else {
                0.0
            };
            flatten_path(&shapes::rounded_rect(x, y, w, h, [rad; 4]), TOLERANCE)
        }
        CalloutBody::Ellipse(_) => flatten_path(&shapes::ellipse(x, y, w, h), TOLERANCE),
        CalloutBody::Cloud { puffs, .. } => cloud_loops(x, y, w, h, (*puffs).clamp(5, MAX_PUFFS)),
        CalloutBody::Banner { notch, .. } => {
            let n = if notch.is_finite() {
                f64::from(notch.max(0.0)).min(w * 0.25)
            } else {
                0.0
            };
            let pts = vec![
                (x, y),
                (x + w, y),
                (x + w - n, y + h * 0.5),
                (x + w, y + h),
                (x, y + h),
                (x + n, y + h * 0.5),
            ];
            let knot = vec![true; pts.len()];
            vec![Polyline {
                pts,
                knot,
                closed: true,
            }]
        }
    }
}

/// Ramanujan's approximation of an ellipse perimeter.
fn ellipse_perimeter(a: f64, b: f64) -> f64 {
    std::f64::consts::PI * (3.0 * (a + b) - ((3.0 * a + b) * (a + 3.0 * b)).sqrt())
}

/// Puffs around an elliptical core, all inside the body rectangle.
fn cloud_loops(x: f64, y: f64, w: f64, h: f64, puffs: u32) -> Vec<Polyline> {
    let (cx, cy) = (x + w * 0.5, y + h * 0.5);
    let (a, b) = (w * 0.5, h * 0.5);
    // Puff radius so neighbours overlap by about a third, solved against
    // the ellipse the puff centres sit on (inset by the radius).
    let mut pr = ellipse_perimeter(a, b) / f64::from(puffs) * 0.65;
    for _ in 0..4 {
        let (ia, ib) = ((a - pr).max(a * 0.2), (b - pr).max(b * 0.2));
        pr = (ellipse_perimeter(ia, ib) / f64::from(puffs) * 0.65).min(a.min(b) * 0.8);
    }
    let (ia, ib) = ((a - pr).max(a * 0.2), (b - pr).max(b * 0.2));
    let mut loops = ellipse_loop(cx, cy, ia, ib);
    // Puffs at equal arc-length steps (sampled), so they space evenly on
    // stretched clouds too.
    const SAMPLES: usize = 720;
    let pts: Vec<P> = (0..=SAMPLES)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / SAMPLES as f64 - std::f64::consts::FRAC_PI_2;
            (cx + ia * t.cos(), cy + ib * t.sin())
        })
        .collect();
    let mut cum = vec![0.0];
    for wnd in pts.windows(2) {
        let l = cum.last().copied().unwrap_or(0.0);
        cum.push(l + (wnd[1].0 - wnd[0].0).hypot(wnd[1].1 - wnd[0].1));
    }
    let total = cum.last().copied().unwrap_or(0.0);
    let mut j = 0;
    for k in 0..puffs {
        let target = total * f64::from(k) / f64::from(puffs);
        while j + 1 < cum.len() && cum[j + 1] < target {
            j += 1;
        }
        let (p, q) = (pts[j], pts[(j + 1).min(pts.len() - 1)]);
        let seg = (cum.get(j + 1).copied().unwrap_or(total) - cum[j]).max(1e-12);
        let f = ((target - cum[j]) / seg).clamp(0.0, 1.0);
        // Alternate puff sizes slightly, like a hand-drawn cloud.
        let r = if k % 2 == 0 { pr } else { pr * 0.85 };
        loops.extend(ellipse_loop(
            p.0 + (q.0 - p.0) * f,
            p.1 + (q.1 - p.1) * f,
            r,
            r,
        ));
    }
    loops
}

/// Nearest point of `loops` to `q`.
fn nearest_on(loops: &[Polyline], q: P) -> Option<P> {
    let mut best: Option<(f64, P)> = None;
    for l in loops {
        let n = l.pts.len();
        for i in 0..n {
            let (a, b) = (l.pts[i], l.pts[(i + 1) % n]);
            let d = (b.0 - a.0, b.1 - a.1);
            let l2 = d.0 * d.0 + d.1 * d.1;
            let t = if l2 > 0.0 {
                (((q.0 - a.0) * d.0 + (q.1 - a.1) * d.1) / l2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let p = (a.0 + d.0 * t, a.1 + d.1 * t);
            let dist = (q.0 - p.0).hypot(q.1 - p.1);
            if best.is_none_or(|(bd, _)| dist < bd) {
                best = Some((dist, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

fn inside_any(loops: &[Polyline], q: P) -> bool {
    loops.iter().any(|l| {
        let n = l.pts.len();
        let mut w = 0i32;
        for i in 0..n {
            let (a, b) = (l.pts[i], l.pts[(i + 1) % n]);
            let c = (b.0 - a.0) * (q.1 - a.1) - (b.1 - a.1) * (q.0 - a.0);
            if a.1 <= q.1 {
                if b.1 > q.1 && c > 0.0 {
                    w += 1;
                }
            } else if b.1 <= q.1 && c < 0.0 {
                w -= 1;
            }
        }
        w != 0
    })
}

fn knot(anchor: P, in_ctrl: P, out_ctrl: P) -> Knot {
    Knot {
        anchor: Point::new(anchor.0, anchor.1),
        in_ctrl: Point::new(in_ctrl.0, in_ctrl.1),
        out_ctrl: Point::new(out_ctrl.0, out_ctrl.1),
        smooth: false,
    }
}

/// Polygon of one tail: base corners inside the body, sides bent by
/// `curve_bias` (quadratic arcs, both bowing the same way). `None` when the
/// tip lies inside the body or the tail is degenerate.
pub fn tail_loop(body: &[Polyline], center: P, tail: &CalloutTail) -> Option<Polyline> {
    if !point_ok(tail.tail_tip) || !tail.base_width.is_finite() || tail.base_width <= 0.0 {
        return None;
    }
    let tip = tail.tail_tip.f64();
    if inside_any(body, tip) {
        return None;
    }
    let half = f64::from(tail.base_width) * 0.5;
    let base = match tail.anchor {
        TailAnchor::Center => center,
        TailAnchor::NearestEdge => {
            let e = nearest_on(body, tip)?;
            // Sink the base into the body (never past its centre) so the
            // base corners are covered and the union closes cleanly.
            let to_c = (center.0 - e.0, center.1 - e.1);
            let lc = to_c.0.hypot(to_c.1);
            if lc > 1e-9 {
                let depth = half.min(lc);
                (e.0 + to_c.0 / lc * depth, e.1 + to_c.1 / lc * depth)
            } else {
                e
            }
        }
    };
    let d = (tip.0 - base.0, tip.1 - base.1);
    let len = d.0.hypot(d.1);
    if len < 1e-6 {
        return None;
    }
    let (ux, uy) = (d.0 / len, d.1 / len);
    let n = (-uy, ux);
    let b1 = (base.0 + n.0 * half, base.1 + n.1 * half);
    let b2 = (base.0 - n.0 * half, base.1 - n.1 * half);
    let bias = if tail.curve_bias.is_finite() {
        f64::from(tail.curve_bias.clamp(-1.0, 1.0))
    } else {
        0.0
    };
    // Quadratic control of each side, pushed sideways; as cubic handles.
    let bend = |a: P, b: P| {
        let q = (
            (a.0 + b.0) * 0.5 + n.0 * bias * len * 0.5,
            (a.1 + b.1) * 0.5 + n.1 * bias * len * 0.5,
        );
        (
            (a.0 + (q.0 - a.0) * 2.0 / 3.0, a.1 + (q.1 - a.1) * 2.0 / 3.0),
            (b.0 + (q.0 - b.0) * 2.0 / 3.0, b.1 + (q.1 - b.1) * 2.0 / 3.0),
        )
    };
    let (s1a, s1b) = bend(b1, tip);
    let (s2a, s2b) = bend(tip, b2);
    let sub = Subpath {
        closed: true,
        knots: vec![knot(b1, b1, s1a), knot(tip, s1b, s2a), knot(b2, s2b, b2)],
        op: Default::default(),
    };
    let pl = flatten_subpath(&sub, TOLERANCE);
    (pl.pts.len() >= 3).then_some(pl)
}

/// Merged outline of body + tails.
pub fn outline(spec: &CalloutSpec) -> Vec<Polyline> {
    let body = body_loops(&spec.body);
    if body.is_empty() {
        return Vec::new();
    }
    let c = spec.body.rect().center().f64();
    let mut parts = body.clone();
    parts.extend(
        spec.tails
            .iter()
            .take(MAX_TAILS)
            .filter_map(|t| tail_loop(&body, c, t)),
    );
    boolean::union(&parts)
}

/// Fill, stroke and outline of a callout, ready for coverage and shadow.
pub fn silhouette(spec: &CalloutSpec) -> Silhouette {
    let outline = outline(spec);
    let fill = if spec.fill.is_some() {
        outline.iter().map(|l| l.pts.clone()).collect()
    } else {
        Vec::new()
    };
    let stroke = spec
        .stroke
        .as_ref()
        .map(|s| stroke(&outline, s, LineCap::Round))
        .unwrap_or_default();
    Silhouette {
        fill,
        stroke,
        outline,
    }
}

/// The merged outline as a path (for export or hit testing upstream).
pub fn outline_path(spec: &CalloutSpec) -> Path {
    Path::new(
        outline(spec)
            .iter()
            .map(|l| Subpath::polygon(&l.pts))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Color, PointF32, StrokeSpec};
    use photocraft_doc::{FillRule, PathOp};
    use photocraft_geom::Rect;
    use photocraft_vector::Rasterizer;

    fn spec(tails: Vec<CalloutTail>) -> CalloutSpec {
        CalloutSpec {
            body: CalloutBody::RoundedRect {
                rect: RectF32::new(20.0, 20.0, 120.0, 60.0),
                radius: 12.0,
            },
            tails,
            fill: Some(Color::WHITE),
            stroke: Some(StrokeSpec::solid(Color::BLACK, 3.0)),
            shadow: None,
        }
    }

    fn tail(x: f32, y: f32, bias: f32) -> CalloutTail {
        CalloutTail {
            tail_tip: PointF32::new(x, y),
            base_width: 24.0,
            curve_bias: bias,
            anchor: TailAnchor::NearestEdge,
        }
    }

    fn coverage(polys: &[Vec<P>], x: i32, y: i32) -> f32 {
        let mut r = Rasterizer::new(false);
        r.add_component(polys, PathOp::Combine, FillRule::NonZero);
        r.render(Rect::new(x, y, x + 1, y + 1))
            .first()
            .copied()
            .unwrap_or(0.0)
    }

    #[test]
    fn tails_merge_into_a_single_outline() {
        let s = spec(vec![tail(60.0, 140.0, 0.4), tail(190.0, 50.0, 0.0)]);
        let o = outline(&s);
        assert_eq!(o.len(), 1, "body and both tails form one loop");
    }

    #[test]
    fn no_stroke_seam_where_the_tail_meets_the_body() {
        let s = spec(vec![tail(80.0, 150.0, 0.0)]);
        let sil = silhouette(&s);
        // Just inside the body edge, where the tail base sits: the merged
        // outline has no edge there, so the stroke does not cover it.
        assert_eq!(coverage(&sil.stroke, 80, 79), 0.0);
        // The tail itself is filled below the body and stroked along its sides.
        assert!(coverage(&sil.fill, 80, 90) > 0.99);
        // The body edge away from the tail is stroked.
        assert!(coverage(&sil.stroke, 40, 79) > 0.5);
    }

    #[test]
    fn tip_inside_the_body_adds_nothing() {
        let a = outline(&spec(vec![]));
        let b = outline(&spec(vec![tail(60.0, 50.0, 0.0)]));
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].pts.len(), b[0].pts.len());
    }

    #[test]
    fn clouds_and_banners_are_single_loops() {
        let mut s = spec(vec![tail(200.0, 160.0, -0.5)]);
        s.body = CalloutBody::Cloud {
            rect: RectF32::new(10.0, 10.0, 160.0, 90.0),
            puffs: 11,
        };
        assert_eq!(outline(&s).len(), 1);
        s.body = CalloutBody::Banner {
            rect: RectF32::new(10.0, 10.0, 160.0, 40.0),
            notch: 12.0,
        };
        assert_eq!(outline(&s).len(), 1);
    }
}
