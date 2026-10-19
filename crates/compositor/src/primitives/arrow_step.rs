//! Straight and Bézier arrows.
//!
//! The head is aligned with the curve tangent at its end, and the shaft is
//! trimmed by the head's setback so it ends inside the head: with a
//! semi-transparent colour the shaft never shows through past the head's
//! outline. Shaft and heads come out as stroke and fill pieces of one
//! silhouette; the coverage step unites them, so where they overlap the
//! colour is applied once.
//!
//! Step badges (numbered circles, teardrops, shields) need text shaping and
//! join this module together with photocraft-text.

use photocraft_doc::{Knot, LineCap, LineJoin, Subpath};
use photocraft_geom::Point;
use photocraft_vector::{Polyline, StrokeStyle, flatten_subpath, stroke_polygons};

use super::{P, Silhouette, TOLERANCE, point_ok, stroke};
use crate::scene::{ArrowCurve, ArrowHeadShape, ArrowHeadStyle, ArrowSpec, PointF32, StrokeSpec};

/// Cubic Bézier `[p0, c1, c2, p1]`.
pub type Cubic = [P; 4];

fn lerp(a: P, b: P, t: f64) -> P {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

pub fn eval(c: &Cubic, t: f64) -> P {
    let ab = lerp(c[0], c[1], t);
    let bc = lerp(c[1], c[2], t);
    let cd = lerp(c[2], c[3], t);
    lerp(lerp(ab, bc, t), lerp(bc, cd, t), t)
}

/// The part of `c` between `t0` and `t1` (de Casteljau).
pub fn sub_curve(c: &Cubic, t0: f64, t1: f64) -> Cubic {
    let split_hi = |c: &Cubic, t: f64| -> Cubic {
        // Right part of a split at t.
        let ab = lerp(c[0], c[1], t);
        let bc = lerp(c[1], c[2], t);
        let cd = lerp(c[2], c[3], t);
        let abc = lerp(ab, bc, t);
        let bcd = lerp(bc, cd, t);
        [lerp(abc, bcd, t), bcd, cd, c[3]]
    };
    let split_lo = |c: &Cubic, t: f64| -> Cubic {
        let ab = lerp(c[0], c[1], t);
        let bc = lerp(c[1], c[2], t);
        let cd = lerp(c[2], c[3], t);
        let abc = lerp(ab, bc, t);
        let bcd = lerp(bc, cd, t);
        [c[0], ab, abc, lerp(abc, bcd, t)]
    };
    let right = split_hi(c, t0);
    let t = if t0 < 1.0 {
        ((t1 - t0) / (1.0 - t0)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    split_lo(&right, t)
}

/// Cubic form of the arrow's shaft.
pub fn shaft_curve(spec: &ArrowSpec) -> Option<Cubic> {
    let ok = |p: PointF32| point_ok(p);
    if !ok(spec.start) || !ok(spec.end) {
        return None;
    }
    let (p0, p1) = (spec.start.f64(), spec.end.f64());
    Some(match spec.curve {
        ArrowCurve::Straight => [p0, lerp(p0, p1, 1.0 / 3.0), lerp(p0, p1, 2.0 / 3.0), p1],
        ArrowCurve::Quadratic { control } if ok(control) => {
            let q = control.f64();
            [p0, lerp(p0, q, 2.0 / 3.0), lerp(p1, q, 2.0 / 3.0), p1]
        }
        ArrowCurve::Cubic { control1, control2 } if ok(control1) && ok(control2) => {
            [p0, control1.f64(), control2.f64(), p1]
        }
        _ => return None,
    })
}

fn unit(d: P) -> Option<P> {
    let l = d.0.hypot(d.1);
    (l > 1e-9).then(|| (d.0 / l, d.1 / l))
}

/// Unit tangent pointing out of the curve at its end (`at_end`) or start.
pub fn end_tangent(c: &Cubic, at_end: bool) -> Option<P> {
    let [p0, c1, c2, p1] = *c;
    if at_end {
        unit((p1.0 - c2.0, p1.1 - c2.1))
            .or_else(|| unit((p1.0 - c1.0, p1.1 - c1.1)))
            .or_else(|| unit((p1.0 - p0.0, p1.1 - p0.1)))
    } else {
        unit((p0.0 - c1.0, p0.1 - c1.1))
            .or_else(|| unit((p0.0 - c2.0, p0.1 - c2.1)))
            .or_else(|| unit((p0.0 - p1.0, p0.1 - p1.1)))
    }
}

/// Parameter where the curve is `dist` (chord) away from its end point
/// (`at_end`) or start point; `None` if the whole curve is closer.
pub fn trim_param(c: &Cubic, dist: f64, at_end: bool) -> Option<f64> {
    if dist <= 0.0 {
        return Some(if at_end { 1.0 } else { 0.0 });
    }
    let anchor = if at_end { c[3] } else { c[0] };
    let d = |t: f64| {
        let p = eval(c, t);
        (p.0 - anchor.0).hypot(p.1 - anchor.1)
    };
    // Walk from the anchor until the distance is first reached, then bisect.
    const STEPS: usize = 128;
    let at = |i: usize| {
        if at_end {
            1.0 - i as f64 / STEPS as f64
        } else {
            i as f64 / STEPS as f64
        }
    };
    let mut prev = at(0);
    for i in 1..=STEPS {
        let t = at(i);
        if d(t) >= dist {
            let (mut lo, mut hi) = (prev, t);
            for _ in 0..48 {
                let mid = (lo + hi) * 0.5;
                if d(mid) >= dist {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            return Some(hi);
        }
        prev = t;
    }
    None
}

/// Head polygon (filled shapes) or chevron polyline (open), tip at `tip`
/// pointing along `dir`.
fn head_geometry(h: &ArrowHeadStyle, tip: P, dir: P, shaft_w: f64) -> (Vec<Vec<P>>, f64) {
    let len = f64::from(h.length).clamp(0.0, 2000.0);
    let hw = f64::from(h.width).clamp(0.0, 2000.0) * 0.5;
    let n = (-dir.1, dir.0);
    let base = (tip.0 - dir.0 * len, tip.1 - dir.1 * len);
    let c1 = (base.0 + n.0 * hw, base.1 + n.1 * hw);
    let c2 = (base.0 - n.0 * hw, base.1 - n.1 * hw);
    // Setback: where the head is as wide as the shaft (plus a round cap),
    // so the shaft end is hidden inside the head.
    let cover = if hw > 1e-9 {
        len * (shaft_w * 0.5) / hw
    } else {
        len
    };
    match h.shape {
        ArrowHeadShape::Triangle => (
            vec![vec![tip, c1, c2]],
            (cover + shaft_w * 0.5).max(len * 0.5).min(len),
        ),
        ArrowHeadShape::Notched => {
            let notch = (tip.0 - dir.0 * len * 0.7, tip.1 - dir.1 * len * 0.7);
            (
                vec![vec![tip, c1, notch, c2]],
                (cover + shaft_w * 0.5).max(len * 0.4).min(len * 0.6),
            )
        }
        ArrowHeadShape::Open => {
            let pl = Polyline {
                pts: vec![c1, tip, c2],
                knot: vec![true; 3],
                closed: false,
            };
            let style = StrokeStyle {
                width: shaft_w,
                cap: LineCap::Round,
                join: LineJoin::Miter,
                miter_limit: 10.0,
                ..Default::default()
            };
            (stroke_polygons(&[pl], &style, TOLERANCE), shaft_w * 0.5)
        }
    }
}

/// Silhouette of an arrow: shaft as stroke pieces, heads as fill pieces.
pub fn silhouette(spec: &ArrowSpec) -> Silhouette {
    let Some(curve) = shaft_curve(spec) else {
        return Silhouette::default();
    };
    if !spec.width.is_finite() || spec.width <= 0.0 {
        return Silhouette::default();
    }
    let w = f64::from(spec.width.min(1000.0));
    let mut fill = Vec::new();
    let (mut t0, mut t1) = (0.0, 1.0);
    for (head, at_end) in [(spec.head, true), (spec.tail_head, false)] {
        let Some(h) = head else { continue };
        let Some(dir) = end_tangent(&curve, at_end) else {
            continue;
        };
        let tip = if at_end { curve[3] } else { curve[0] };
        let (polys, setback) = head_geometry(&h, tip, dir, w);
        fill.extend(polys);
        match trim_param(&curve, setback, at_end) {
            Some(t) if at_end => t1 = t,
            Some(t) => t0 = t,
            // The whole shaft hides under the head.
            None => t1 = t0,
        }
    }
    let mut outline = Vec::new();
    let mut shaft = Vec::new();
    if t1 > t0 {
        let c = sub_curve(&curve, t0, t1);
        let k = |a: P, i: P, o: P| Knot {
            anchor: Point::new(a.0, a.1),
            in_ctrl: Point::new(i.0, i.1),
            out_ctrl: Point::new(o.0, o.1),
            smooth: false,
        };
        let sub = Subpath {
            closed: false,
            knots: vec![k(c[0], c[0], c[1]), k(c[3], c[2], c[3])],
            op: Default::default(),
        };
        let pl = flatten_subpath(&sub, TOLERANCE);
        let s = StrokeSpec {
            color: spec.color,
            width: spec.width,
            join: LineJoin::Round,
            dashes: spec.dashes.clone(),
        };
        shaft = stroke(std::slice::from_ref(&pl), &s, LineCap::Round);
        outline.push(pl);
    }
    Silhouette {
        fill,
        stroke: shaft,
        outline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Color;

    fn arrow(curve: ArrowCurve) -> ArrowSpec {
        ArrowSpec {
            start: PointF32::new(10.0, 100.0),
            end: PointF32::new(200.0, 40.0),
            curve,
            color: Color::new(1.0, 0.0, 0.0, 0.5),
            width: 6.0,
            head: Some(ArrowHeadStyle {
                shape: ArrowHeadShape::Triangle,
                length: 24.0,
                width: 22.0,
            }),
            tail_head: None,
            dashes: Vec::new(),
            shadow: None,
        }
    }

    #[test]
    fn tangent_follows_the_control_point() {
        let c = shaft_curve(&arrow(ArrowCurve::Quadratic {
            control: PointF32::new(200.0, 140.0),
        }))
        .unwrap_or([(0.0, 0.0); 4]);
        let t = end_tangent(&c, true).unwrap_or((0.0, 0.0));
        // Arriving from straight below the end point.
        assert!(t.0.abs() < 1e-9 && t.1 < -0.999, "{t:?}");
    }

    #[test]
    fn shaft_stops_inside_the_head() {
        let spec = arrow(ArrowCurve::Quadratic {
            control: PointF32::new(120.0, 160.0),
        });
        let sil = silhouette(&spec);
        let tip = (200.0, 40.0);
        // No shaft vertex reaches closer to the tip than the head's
        // narrow part (where the head is narrower than the shaft).
        let min_d = sil
            .stroke
            .iter()
            .flatten()
            .map(|p| (p.0 - tip.0).hypot(p.1 - tip.1))
            .fold(f64::INFINITY, f64::min);
        let cover = 24.0 * 3.0 / 11.0;
        assert!(min_d >= cover - 1e-6, "{min_d}");
        assert_eq!(sil.fill.len(), 1);
    }

    #[test]
    fn sub_curve_matches_the_original() {
        let c: Cubic = [(0.0, 0.0), (10.0, 30.0), (40.0, -20.0), (50.0, 10.0)];
        let s = sub_curve(&c, 0.2, 0.7);
        for i in 0..=10 {
            let u = i as f64 / 10.0;
            let a = eval(&s, u);
            let b = eval(&c, 0.2 + 0.5 * u);
            assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
        }
    }
}
