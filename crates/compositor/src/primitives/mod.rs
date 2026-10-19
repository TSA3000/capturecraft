//! Vector geometry of the annotation primitives.
//!
//! Every primitive reduces to a [`Silhouette`]: fill polygons and stroke
//! polygons (both filled with the non-zero rule), plus the outline the
//! stroke was built from. The prepare step turns those into exact-area
//! coverage masks with photocraft-vector's rasterizer; the masks are shared
//! by both compositors, so vector anti-aliasing is identical on CPU and GPU.

pub mod arrow_step;
pub mod boolean;
pub mod callout;

use photocraft_doc::{LineCap, LineJoin, Path};
use photocraft_geom::{Affine, Point};
use photocraft_vector::{Polyline, StrokeStyle, flatten_path, shapes, stroke_polygons};

use crate::scene::{DropShadowStyle, PointF32, ShapeGeometry, ShapeSpec, StrokeSpec};

pub type P = (f64, f64);

/// Flattening tolerance of annotation outlines, in pixels. The exact-area
/// rasterizer keeps edges smooth at this chord error.
pub const TOLERANCE: f64 = 0.05;

/// Largest coordinate magnitude accepted; anything beyond is dropped.
pub const MAX_COORD: f64 = 1_000_000.0;

/// Area covered by an annotation.
#[derive(Clone, Debug, Default)]
pub struct Silhouette {
    /// Fill area (non-zero).
    pub fill: Vec<Vec<P>>,
    /// Stroke outline pieces (non-zero).
    pub stroke: Vec<Vec<P>>,
    /// Closed or open outline the stroke follows.
    pub outline: Vec<Polyline>,
}

impl Silhouette {
    pub fn is_empty(&self) -> bool {
        self.fill.is_empty() && self.stroke.is_empty()
    }

    /// Components of the drop shadow: fill and stroke (and the spread) moved
    /// by the shadow offset. Each component is filled non-zero and the
    /// components are combined, so overlaps never darken twice.
    pub fn shadow_components(&self, s: &DropShadowStyle) -> Vec<Vec<Vec<P>>> {
        let d = (f64::from(s.offset.x), f64::from(s.offset.y));
        if !d.0.is_finite() || !d.1.is_finite() {
            return Vec::new();
        }
        let mut comps = vec![translate(&self.fill, d), translate(&self.stroke, d)];
        if s.spread.is_finite() && s.spread > 0.0 {
            let style = StrokeStyle {
                width: f64::from(s.spread.min(500.0)) * 2.0,
                cap: LineCap::Round,
                join: LineJoin::Round,
                ..Default::default()
            };
            comps.push(translate(
                &stroke_polygons(&self.outline, &style, TOLERANCE),
                d,
            ));
        }
        comps.retain(|c| !c.is_empty());
        comps
    }
}

impl Silhouette {
    /// The silhouette drawn through `t` (strokes turn and scale with it).
    pub fn transformed(self, t: &Affine) -> Silhouette {
        if *t == Affine::IDENTITY {
            return self;
        }
        Silhouette {
            fill: transform_polys(self.fill, t),
            stroke: transform_polys(self.stroke, t),
            outline: transform_lines(self.outline, t),
        }
    }
}

fn apply(t: &Affine, p: P) -> P {
    let q = t.apply(Point::new(p.0, p.1));
    (q.x, q.y)
}

/// Polygons through `t`; any that leave the coordinate range are dropped.
pub fn transform_polys(polys: Vec<Vec<P>>, t: &Affine) -> Vec<Vec<P>> {
    if *t == Affine::IDENTITY {
        return polys;
    }
    polys
        .into_iter()
        .map(|p| p.into_iter().map(|q| apply(t, q)).collect::<Vec<P>>())
        .filter(|p| p.iter().all(|q| coord_ok(q.0) && coord_ok(q.1)))
        .collect()
}

/// Polylines through `t`; any that leave the coordinate range are dropped.
pub fn transform_lines(lines: Vec<Polyline>, t: &Affine) -> Vec<Polyline> {
    if *t == Affine::IDENTITY {
        return lines;
    }
    lines
        .into_iter()
        .map(|mut l| {
            l.pts.iter_mut().for_each(|q| *q = apply(t, *q));
            l
        })
        .filter(|l| l.pts.iter().all(|q| coord_ok(q.0) && coord_ok(q.1)))
        .collect()
}

pub fn translate(polys: &[Vec<P>], d: P) -> Vec<Vec<P>> {
    polys
        .iter()
        .map(|p| p.iter().map(|q| (q.0 + d.0, q.1 + d.1)).collect())
        .collect()
}

fn coord_ok(v: f64) -> bool {
    v.is_finite() && v.abs() <= MAX_COORD
}

pub(crate) fn point_ok(p: PointF32) -> bool {
    p.is_finite() && coord_ok(f64::from(p.x)) && coord_ok(f64::from(p.y))
}

/// Path of a plain shape, or `None` when it is degenerate or out of range.
pub fn geometry_path(g: &ShapeGeometry) -> Option<Path> {
    let ok = |r: &crate::scene::RectF32| {
        r.is_valid() && [r.x, r.y, r.w, r.h].iter().all(|v| coord_ok(f64::from(*v)))
    };
    Some(match g {
        ShapeGeometry::Rect(r) if ok(r) => {
            shapes::rect(r.x.into(), r.y.into(), r.w.into(), r.h.into())
        }
        ShapeGeometry::RoundedRect { rect: r, radius } if ok(r) => {
            let rad = if radius.is_finite() {
                f64::from(radius.max(0.0))
            } else {
                0.0
            };
            shapes::rounded_rect(r.x.into(), r.y.into(), r.w.into(), r.h.into(), [rad; 4])
        }
        ShapeGeometry::Ellipse(r) if ok(r) => {
            shapes::ellipse(r.x.into(), r.y.into(), r.w.into(), r.h.into())
        }
        ShapeGeometry::Polygon(pts)
            if pts.len() >= 3 && pts.len() <= 100_000 && pts.iter().all(|p| point_ok(*p)) =>
        {
            let pts: Vec<P> = pts.iter().map(|p| p.f64()).collect();
            Path::new(vec![photocraft_doc::Subpath::polygon(&pts)])
        }
        ShapeGeometry::Trace { points, tolerance }
            if points.len() >= 2
                && points.len() <= 100_000
                && points.iter().all(|p| point_ok(*p)) =>
        {
            let pts: Vec<[f64; 2]> = points.iter().map(|p| [p.x.into(), p.y.into()]).collect();
            let sub = photocraft_vector::trace::fit_trace(&pts, f64::from(*tolerance), false)?;
            Path::new(vec![sub])
        }
        _ => return None,
    })
}

/// Closed outline loops of a plain shape.
pub fn geometry_outline(g: &ShapeGeometry) -> Vec<Polyline> {
    geometry_path(g)
        .map(|p| flatten_path(&p, TOLERANCE))
        .unwrap_or_default()
}

fn polyline_length(lines: &[Polyline]) -> f64 {
    lines
        .iter()
        .map(|l| {
            let n = l.pts.len();
            let segs = if l.closed { n } else { n.saturating_sub(1) };
            (0..segs)
                .map(|i| {
                    let (a, b) = (l.pts[i], l.pts[(i + 1) % n]);
                    (b.0 - a.0).hypot(b.1 - a.1)
                })
                .sum::<f64>()
        })
        .sum()
}

/// Stroke geometry for `s` along `lines`. Dash patterns that would cut the
/// outline into more than 100 000 pieces are drawn solid.
pub fn stroke(lines: &[Polyline], s: &StrokeSpec, cap: LineCap) -> Vec<Vec<P>> {
    if !s.width.is_finite() || s.width <= 0.0 {
        return Vec::new();
    }
    let mut dashes: Vec<f64> = s.dashes.iter().map(|d| f64::from(*d)).collect();
    let period: f64 = dashes.iter().sum();
    if dashes.iter().any(|d| !d.is_finite() || *d < 0.0)
        || period < 0.5
        || polyline_length(lines) / period * dashes.len() as f64 > 100_000.0
    {
        dashes.clear();
    }
    let style = StrokeStyle {
        width: f64::from(s.width.min(1000.0)),
        cap,
        join: s.join,
        miter_limit: 4.0,
        dashes,
        dash_offset: 0.0,
    };
    stroke_polygons(lines, &style, TOLERANCE)
}

/// Silhouette of a Shape-tool annotation.
pub fn shape_silhouette(spec: &ShapeSpec) -> Silhouette {
    let outline = geometry_outline(&spec.geometry);
    // A trace is an open line: never filled, its ends rounded.
    let open = matches!(spec.geometry, ShapeGeometry::Trace { .. });
    let fill = if spec.fill.is_some() && !open {
        outline.iter().map(|l| l.pts.clone()).collect()
    } else {
        Vec::new()
    };
    let stroke = spec
        .stroke
        .as_ref()
        .map(|s| {
            stroke(
                &outline,
                s,
                if open { LineCap::Round } else { LineCap::Butt },
            )
        })
        .unwrap_or_default();
    Silhouette {
        fill,
        stroke,
        outline,
    }
}
