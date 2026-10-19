//! Document → render graph: what the compositor (L4) draws for an open
//! document. A pure translation; nothing here touches the document.
//!
//! Order and spaces, matching what the commands mean:
//! 1. The display pixels ([`DocState::display_pixels`]) at
//!    [`Canvas::base_origin`](capturecraft_format::Canvas::base_origin), on
//!    a canvas as large as before any cut.
//! 2. Every cut out, in document order. Format cut outs are in base space,
//!    each in the space left by the cuts before it; the scene's are in the
//!    canvas of their stage, so `t = start + base_origin`.
//! 3. Every other annotation, in document order, in canvas space. They come
//!    after all cuts because `canvas.cutOut` moves existing annotations
//!    into post-cut space and new ones are placed there.
//!
//! The compositor has no text node yet: callout text and step badge labels
//! are not drawn. The cursor layer is not drawn (its sprite is not loaded
//! into the session).

use std::sync::Arc;

use capturecraft_compositor::{
    ArrowCurve as SArrowCurve, ArrowHeadShape, ArrowHeadStyle, ArrowSpec, BackdropFilterKind,
    BackdropFilterSpec, CalloutBody as SCalloutBody, CalloutSpec, CalloutTail as SCalloutTail,
    Color as SColor, CutAxis, CutOutSeamSpec, DropShadowStyle, InnerShadowStyle,
    LineJoin as SLineJoin, MagnifierSpec, PointF32, RasterSpec, RectF32, RenderNode, RenderScene,
    Resample, SceneNode, SeamShadow, SeamStyle, ShapeBounds, ShapeGeometry, ShapeSpec,
    SpotlightSpec, StrokeSpec, TailAnchor as STailAnchor,
};
use capturecraft_format::{
    Annotation, AnnotationNode, ArrowCurve, ArrowHead, Axis, BackdropEffect, Background,
    BadgeShape, BlurRegion, Callout, CalloutBody, Color, CurvedArrow, CutOutSeam, DashStyle,
    DropShadow, Fill, HeadStyle, LensShape, LineJoin, MagnifierLens, Manifest, NodeKind, RectF,
    RegionShape, Sampling, SeamEdge, ShapeNode, SmartObject, Spotlight, StepBadge, Stroke,
    TailAnchor,
};
use photocraft_geom::Point;

use crate::{DocState, Pixels};

/// Scene id of the base raster. [`Manifest::alloc_node_id`] never hands
/// out 0, so it cannot collide with an annotation.
pub const BASE_NODE_ID: u64 = 0;

/// Feature size of a torn seam's first octave. The format has no
/// wavelength (it stores an octave count instead, which the compositor
/// fixes at its own value).
const TORN_WAVELENGTH_PX: f32 = 24.0;

/// Pixel grid colour of a magnifier in pixel grid mode (not in the format).
const GRID_COLOR: SColor = SColor::new(0.0, 0.0, 0.0, 0.25);

/// The render graph of `doc`, bottom to top.
pub fn render_scene(doc: &DocState) -> RenderScene {
    let m = &doc.manifest;
    let cuts = cut_specs(m);
    // The scene starts before the cuts: add the removed strips back.
    let (mut w, mut h) = (
        u64::from(m.canvas.size.width),
        u64::from(m.canvas.size.height),
    );
    for (_, spec) in &cuts {
        let removed = u64::from(spec.t1.saturating_sub(spec.t0));
        match spec.axis {
            CutAxis::Horizontal => h += removed,
            CutAxis::Vertical => w += removed,
        }
    }
    let side = |v: u64| u32::try_from(v).unwrap_or(u32::MAX);

    let mut nodes = Vec::with_capacity(m.annotations.len() + 1);
    if let Some(p) = doc.display_pixels() {
        let o = m.canvas.base_origin;
        nodes.push(SceneNode::new(
            BASE_NODE_ID,
            p.revision(),
            raster(p, [o.x, o.y], 1.0),
        ));
    }
    nodes.extend(
        cuts.into_iter()
            .map(|(n, spec)| SceneNode::new(n.id.0, revision(n, &[]), RenderNode::CutOut(spec))),
    );
    for n in &m.annotations {
        let NodeKind::Known(a) = &n.kind else {
            continue;
        };
        let alpha = node_opacity(n);
        if !n.visible || alpha <= 0.0 || matches!(a, Annotation::CutOutSeam(_)) {
            continue;
        }
        let (node, extra) = match a {
            Annotation::Callout(c) => (Some(callout(c, alpha)), None),
            Annotation::StepBadge(b) => (Some(step_badge(b, alpha)), None),
            Annotation::CurvedArrow(c) => (Some(arrow(c, alpha)), None),
            Annotation::BlurRegion(b) => (Some(blur_region(b)), None),
            Annotation::MagnifierLens(l) => (Some(magnifier(l, alpha)), None),
            Annotation::Spotlight(s) => (Some(spotlight(s, alpha)), None),
            Annotation::Shape(s) => (Some(shape(s, alpha)), None),
            Annotation::SmartObject(o) => match doc.object_pixels(n.id) {
                Some(p) => (Some(smart_object(o, p, alpha)), Some(p.revision())),
                None => (None, None),
            },
            Annotation::CutOutSeam(_) => (None, None),
        };
        if let Some(node) = node {
            let extra: Vec<u64> = extra.into_iter().collect();
            nodes.push(SceneNode::new(n.id.0, revision(n, &extra), node));
        }
    }
    RenderScene {
        width: side(w),
        height: side(h),
        background: match m.canvas.background {
            Background::Transparent => SColor::TRANSPARENT,
            Background::Solid { color } => rgba(color, 1.0),
        },
        nodes,
    }
}

// ---------------------------------------------------------------------------
// Revisions and colours

/// SplitMix64 finalizer.
fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The node's revision with its opacity (which an edit may change without
/// [`AnnotationNode::touch`]) and `extra` (pixel revisions) folded in.
fn revision(n: &AnnotationNode, extra: &[u64]) -> u64 {
    let mut h = mix(n.revision ^ u64::from(n.opacity.to_bits()).rotate_left(32));
    for e in extra {
        h = mix(h ^ *e);
    }
    h
}

/// `0..=1`; NaN counts as hidden.
fn node_opacity(n: &AnnotationNode) -> f32 {
    if n.opacity.is_finite() {
        n.opacity.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Straight u8 RGBA → scene colour, alpha scaled by the node opacity.
fn rgba(c: Color, alpha: f32) -> SColor {
    let mut s = SColor::rgba8(c.r, c.g, c.b, c.a);
    s.a *= alpha;
    s
}

/// A linear gradient has no scene equivalent: it fills with the mean of its
/// stops.
fn fill(f: &Fill, alpha: f32) -> Option<SColor> {
    match f {
        Fill::None => None,
        Fill::Solid { color } => Some(rgba(*color, alpha)),
        Fill::Linear { stops, .. } => {
            let n = stops.len() as f32;
            if n == 0.0 {
                return None;
            }
            let mut acc = [0.0f32; 4];
            for s in stops {
                let c = SColor::rgba8(s.color.r, s.color.g, s.color.b, s.color.a);
                for (a, v) in acc.iter_mut().zip([c.r, c.g, c.b, c.a]) {
                    *a += v / n;
                }
            }
            Some(SColor::new(acc[0], acc[1], acc[2], acc[3] * alpha))
        }
    }
}

fn pt(p: Point) -> PointF32 {
    PointF32::new(p.x as f32, p.y as f32)
}

fn rect(r: RectF) -> RectF32 {
    RectF32::new(r.x as f32, r.y as f32, r.w as f32, r.h as f32)
}

/// Dash pattern in multiples of the stroke width. Caps are not in the
/// scene: dashes end square.
fn dashes(d: DashStyle, width: f32) -> Vec<f32> {
    match d {
        DashStyle::Solid => Vec::new(),
        DashStyle::Dashed => vec![3.0 * width, 2.0 * width],
        DashStyle::Dotted => vec![width, width],
    }
}

fn stroke(s: &Stroke, alpha: f32) -> StrokeSpec {
    StrokeSpec {
        color: rgba(s.color, alpha),
        width: s.width,
        join: match s.join {
            LineJoin::Miter => SLineJoin::Miter,
            LineJoin::Round => SLineJoin::Round,
            LineJoin::Bevel => SLineJoin::Bevel,
        },
        dashes: dashes(s.dash, s.width),
    }
}

/// The format's blur is a radius of about 2σ.
fn shadow(s: &DropShadow, alpha: f32) -> DropShadowStyle {
    DropShadowStyle {
        color: rgba(s.color, alpha),
        offset: PointF32::new(s.offset_x, s.offset_y),
        blur_sigma: (s.blur * 0.5).max(0.0),
        spread: s.spread,
    }
}

fn region(s: &RegionShape) -> ShapeGeometry {
    match s {
        RegionShape::Rect { rect: r } => ShapeGeometry::Rect(rect(*r)),
        RegionShape::RoundedRect { rect: r, radius } => ShapeGeometry::RoundedRect {
            rect: rect(*r),
            radius: *radius,
        },
        RegionShape::Ellipse { rect: r } => ShapeGeometry::Ellipse(rect(*r)),
        RegionShape::Polygon { points } => {
            ShapeGeometry::Polygon(points.iter().map(|p| pt(*p)).collect())
        }
    }
}

// ---------------------------------------------------------------------------
// Rasters and cut outs

fn raster(p: &Pixels, origin: [i32; 2], opacity: f32) -> RenderNode {
    // Cloning a surface copies its tile map only; the tiles are shared.
    RenderNode::Raster(RasterSpec {
        surface: Arc::new(p.surface().clone()),
        origin,
        opacity,
    })
}

/// Smart objects are drawn unscaled at the rounded top-left of their rect:
/// the scene's rasters have no scale.
fn smart_object(o: &SmartObject, p: &Pixels, alpha: f32) -> RenderNode {
    let origin = [o.rect.x.round() as i32, o.rect.y.round() as i32];
    raster(p, origin, alpha)
}

/// Every effective cut out with its scene spec, in document order.
///
/// `visible` is ignored: the canvas size and every annotation position
/// already include the cut, so it is always applied. A strip partly above
/// or left of the canvas edge (negative `base_origin`) is cut only where it
/// is on the canvas. `gap_px` is drawn, so the frame comes out that much
/// larger than `canvas.size` (the format leaves the gap out of the size and
/// of annotation positions).
fn cut_specs(m: &Manifest) -> Vec<(&AnnotationNode, CutOutSeamSpec)> {
    m.annotations
        .iter()
        .filter_map(|n| match n.annotation() {
            Some(Annotation::CutOutSeam(c)) => cut_spec(m, c, node_opacity(n)).map(|s| (n, s)),
            _ => None,
        })
        .collect()
}

fn cut_spec(m: &Manifest, c: &CutOutSeam, alpha: f32) -> Option<CutOutSeamSpec> {
    let o = m.canvas.base_origin;
    let (axis, origin) = match c.axis {
        Axis::Horizontal => (CutAxis::Horizontal, o.y),
        Axis::Vertical => (CutAxis::Vertical, o.x),
    };
    // Same rounding as `canvas.cutOut` uses for the canvas size.
    let t0 = (c.start + f64::from(origin)).round();
    let t1 = t0 + c.removed().round();
    let clamp = |v: f64| v.clamp(0.0, f64::from(u32::MAX)) as u32;
    let (t0, t1) = (clamp(t0), clamp(t1));
    if t1 <= t0 {
        return None;
    }
    let seam = match c.edge {
        SeamEdge::Straight => SeamStyle::Straight,
        SeamEdge::Torn {
            seed,
            amplitude_px,
            roughness,
            ..
        } => SeamStyle::TornEdge {
            amplitude_px,
            roughness,
            wavelength_px: TORN_WAVELENGTH_PX,
            seed,
        },
    };
    Some(CutOutSeamSpec {
        axis,
        t0,
        t1,
        gap_px: clamp(f64::from(c.gap_px).round()),
        gap_fill: SColor::TRANSPARENT,
        seam,
        // The seam shadow is the edge stepped along the offset and
        // averaged: one sample per pixel of blur approximates the softness.
        shadow: c.shadow.map(|s| SeamShadow {
            color: rgba(s.color, alpha),
            offset: PointF32::new(s.offset_x, s.offset_y),
            samples: (s.blur.ceil().max(1.0) as u32).min(32),
        }),
    })
}

// ---------------------------------------------------------------------------
// Annotations

/// Edge-pinned tails become nearest-edge tails; `fillet_px` is not drawn.
fn callout(c: &Callout, alpha: f32) -> RenderNode {
    let r = rect(c.bounds);
    let body = match c.body {
        CalloutBody::Rect => SCalloutBody::RoundedRect {
            rect: r,
            radius: 0.0,
        },
        CalloutBody::RoundedRect { radius } => SCalloutBody::RoundedRect { rect: r, radius },
        CalloutBody::Ellipse => SCalloutBody::Ellipse(r),
        CalloutBody::Cloud { lobes } => SCalloutBody::Cloud {
            rect: r,
            puffs: lobes,
        },
        CalloutBody::Banner { fold } => SCalloutBody::Banner {
            rect: r,
            notch: fold,
        },
    };
    RenderNode::Callout(CalloutSpec {
        body,
        tails: c
            .tails
            .iter()
            .map(|t| SCalloutTail {
                tail_tip: pt(t.tip),
                base_width: t.base_width,
                curve_bias: t.curve_bias,
                anchor: match t.anchor {
                    TailAnchor::Center => STailAnchor::Center,
                    TailAnchor::Nearest | TailAnchor::Edge { .. } => STailAnchor::NearestEdge,
                },
            })
            .collect(),
        fill: fill(&c.fill, alpha),
        stroke: c.stroke.as_ref().map(|s| stroke(s, alpha)),
        shadow: c.shadow.as_ref().map(|s| shadow(s, alpha)),
    })
}

/// The badge body as a plain shape (the label needs a text node). The ring
/// is inset by half its width so the badge keeps its radius.
fn step_badge(b: &StepBadge, alpha: f32) -> RenderNode {
    let inset = b.ring.map_or(0.0, |s| (s.width * 0.5).clamp(0.0, b.radius));
    let r = f64::from(b.radius - inset);
    let c = b.center;
    let square = RectF32::new(
        (c.x - r) as f32,
        (c.y - r) as f32,
        (2.0 * r) as f32,
        (2.0 * r) as f32,
    );
    // Pointer direction, clockwise from up (y points down); down by default.
    let deg = f64::from(b.pointer_deg.unwrap_or(180.0)).to_radians();
    let geometry = match b.shape {
        BadgeShape::Circle => ShapeGeometry::Ellipse(square),
        BadgeShape::RoundedSquare => ShapeGeometry::RoundedRect {
            rect: square,
            radius: (r * 0.3) as f32,
        },
        BadgeShape::Teardrop => ShapeGeometry::Polygon(teardrop(c, r, deg)),
        BadgeShape::Shield => ShapeGeometry::Polygon(shield(c, r, deg)),
    };
    RenderNode::Shape(ShapeSpec {
        geometry,
        fill: Some(rgba(b.fill, alpha)),
        stroke: b.ring.as_ref().map(|s| stroke(s, alpha)),
        shadow: b.shadow.as_ref().map(|s| shadow(s, alpha)),
    })
}

/// `p` rotated clockwise (on screen) by `a` radians around `c`.
fn rotate(c: Point, p: (f64, f64), a: f64) -> PointF32 {
    let (s, k) = a.sin_cos();
    PointF32::new(
        (c.x + p.0 * k - p.1 * s) as f32,
        (c.y + p.0 * s + p.1 * k) as f32,
    )
}

/// A circle drawn to a corner `r·√2` from the centre: the tangents from the
/// corner touch the circle 45° either side of the pointer.
fn teardrop(c: Point, r: f64, deg: f64) -> Vec<PointF32> {
    const STEPS: u32 = 48;
    let mut pts = vec![rotate(c, (0.0, -r * std::f64::consts::SQRT_2), deg)];
    for i in 0..=STEPS {
        let a = std::f64::consts::FRAC_PI_4
            + std::f64::consts::PI * 1.5 * f64::from(i) / f64::from(STEPS);
        pts.push(rotate(c, (r * a.sin(), -r * a.cos()), deg));
    }
    pts
}

/// Flat top, straight sides, curved to a point; the point is the pointer.
fn shield(c: Point, r: f64, deg: f64) -> Vec<PointF32> {
    const STEPS: u32 = 12;
    // Built pointing down, then turned from down (180°) to the pointer.
    let turn = deg - std::f64::consts::PI;
    let mut right = Vec::new();
    // Quadratic from (r, 0.1r) through control (r, 0.8r) to the tip (0, 1.2r).
    for i in 0..=STEPS {
        let t = f64::from(i) / f64::from(STEPS);
        let u = 1.0 - t;
        let x = u * u * r + 2.0 * u * t * r;
        let y = u * u * 0.1 * r + 2.0 * u * t * 0.8 * r + t * t * 1.2 * r;
        right.push((x, y));
    }
    let mut pts = vec![rotate(c, (-r, -r), turn), rotate(c, (r, -r), turn)];
    pts.extend(right.iter().map(|p| rotate(c, *p, turn)));
    // The left side mirrors the right, tip excluded.
    pts.extend(
        right
            .iter()
            .rev()
            .skip(1)
            .map(|p| rotate(c, (-p.0, p.1), turn)),
    );
    pts
}

/// Line, Circle, Square and Diamond heads have no scene shape: line heads
/// draw as open chevrons, the filled ones as triangles. `taper` and the
/// stroke cap are not drawn.
fn head(h: &ArrowHead, width: f32) -> Option<ArrowHeadStyle> {
    let shape = match h.style {
        HeadStyle::None => return None,
        HeadStyle::OpenV | HeadStyle::Line => ArrowHeadShape::Open,
        HeadStyle::Triangle | HeadStyle::Circle | HeadStyle::Square | HeadStyle::Diamond => {
            ArrowHeadShape::Triangle
        }
    };
    (h.length > 0.0 && h.width > 0.0).then_some(ArrowHeadStyle {
        shape,
        length: h.length * width,
        width: h.width * width,
    })
}

fn arrow(a: &CurvedArrow, alpha: f32) -> RenderNode {
    let w = a.stroke.width;
    RenderNode::Arrow(ArrowSpec {
        start: pt(a.start),
        end: pt(a.end),
        curve: match a.curve {
            ArrowCurve::Straight => SArrowCurve::Straight,
            ArrowCurve::Quadratic { control } => SArrowCurve::Quadratic {
                control: pt(control),
            },
            ArrowCurve::Cubic { c0, c1 } => SArrowCurve::Cubic {
                control1: pt(c0),
                control2: pt(c1),
            },
        },
        color: rgba(a.stroke.color, alpha),
        width: w,
        head: head(&a.end_head, w),
        tail_head: head(&a.start_head, w),
        dashes: dashes(a.stroke.dash, w),
        shadow: a.shadow.as_ref().map(|s| shadow(s, alpha)),
    })
}

/// Backdrop effects cannot fade: node opacity only hides them at 0, and
/// `feather_px` is not drawn. Pixelate blocks start at the shape's top-left
/// rather than on the canvas grid.
fn blur_region(b: &BlurRegion) -> RenderNode {
    RenderNode::BackdropFilter(BackdropFilterSpec {
        shape: region(&b.shape),
        filter: match b.effect {
            BackdropEffect::Gaussian { sigma } => BackdropFilterKind::GaussianBlur { sigma },
            BackdropEffect::Pixelate { block_size_px } => {
                BackdropFilterKind::Pixelate { block_size_px }
            }
        },
    })
}

/// The lens content cannot fade (opacity applies to its strokes and
/// shadow); a cone connector draws as a line.
fn magnifier(l: &MagnifierLens, alpha: f32) -> RenderNode {
    RenderNode::Magnifier(MagnifierSpec {
        source_center: pt(l.source_center),
        lens_bounds: match l.lens {
            LensShape::Circle { center, radius } => ShapeBounds::Circle {
                center: pt(center),
                radius,
            },
            LensShape::RoundedRect { rect: r, radius } => ShapeBounds::RoundedRect {
                rect: rect(r),
                radius,
            },
        },
        zoom_factor: l.zoom,
        resample: match l.sampling {
            Sampling::Bicubic => Resample::CatmullRom,
            Sampling::Lanczos3 => Resample::Lanczos3,
            Sampling::Nearest => Resample::Nearest,
        },
        pixel_grid_mode: l.pixel_grid,
        grid_color: SColor {
            a: GRID_COLOR.a * alpha,
            ..GRID_COLOR
        },
        border_stroke: l.border.as_ref().map(|s| stroke(s, alpha)),
        inner_shadow: l.inner_shadow.map(|s| InnerShadowStyle {
            color: rgba(s.color, alpha),
            offset: PointF32::new(s.offset_x, s.offset_y),
            blur_sigma: (s.blur * 0.5).max(0.0),
        }),
        drop_shadow: None,
        connector: l.connector.as_ref().map(|c| stroke(&c.stroke, alpha)),
    })
}

/// Node opacity scales the dimming; the outside blur cannot fade.
fn spotlight(s: &Spotlight, alpha: f32) -> RenderNode {
    RenderNode::Spotlight(SpotlightSpec {
        targets: s.targets.iter().map(region).collect(),
        dim_opacity: s.dim_opacity * alpha,
        dim_color: rgba(s.dim_color, 1.0),
        feather_px: s.feather_px,
        blur_sigma: s.blur_sigma.unwrap_or(0.0),
    })
}

/// The Multiply blend (highlighter) has no scene equivalent: drawn Normal.
fn shape(s: &ShapeNode, alpha: f32) -> RenderNode {
    RenderNode::Shape(ShapeSpec {
        geometry: region(&s.shape),
        fill: fill(&s.fill, alpha),
        stroke: s.stroke.as_ref().map(|k| stroke(k, alpha)),
        shadow: s.shadow.as_ref().map(|k| shadow(k, alpha)),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::Session;
    use capturecraft_compositor::CpuCompositor;
    use capturecraft_format::{NodeId, Offset};
    use photocraft_geom::Size;
    use serde_json::json;

    fn session(w: u32, h: u32) -> Session {
        let mut s = Session::new();
        let px = Pixels::new(Size::new(w, h), vec![255; (w * h * 4) as usize]);
        s.open(Manifest::new(Size::new(w, h)), px).unwrap();
        s
    }

    fn scene(s: &Session) -> RenderScene {
        render_scene(s.active().unwrap())
    }

    fn id(v: serde_json::Value) -> u64 {
        v["id"].as_u64().unwrap()
    }

    #[test]
    fn base_then_annotations_in_document_order() {
        let mut s = session(400, 300);
        let callout = id(s
            .run(
                "annotation.addCallout",
                &json!({ "bounds": { "x": 20, "y": 30, "w": 160, "h": 80 }, "text": "Hi",
                         "tails": [{ "tip": { "x": 100, "y": 200 } }] }),
            )
            .unwrap());
        let step = id(s
            .run("annotation.addStep", &json!({ "at": { "x": 50, "y": 60 } }))
            .unwrap());
        let sc = scene(&s);
        assert_eq!((sc.width, sc.height), (400, 300));
        assert_eq!(sc.background, SColor::TRANSPARENT);
        let ids: Vec<u64> = sc.nodes.iter().map(|n| n.id.0).collect();
        assert_eq!(ids, [BASE_NODE_ID, callout, step]);
        let RenderNode::Raster(base) = &sc.nodes[0].node else {
            panic!("base raster expected");
        };
        assert_eq!(base.origin, [0, 0]);
        let RenderNode::Callout(c) = &sc.nodes[1].node else {
            panic!("callout expected");
        };
        assert_eq!(c.body.rect(), RectF32::new(20.0, 30.0, 160.0, 80.0));
        assert_eq!(c.tails[0].tail_tip, PointF32::new(100.0, 200.0));
        assert_eq!(c.fill, Some(SColor::WHITE));
        let RenderNode::Shape(b) = &sc.nodes[2].node else {
            panic!("badge shape expected");
        };
        // Default badge: radius 16, no ring.
        assert_eq!(
            b.geometry,
            ShapeGeometry::Ellipse(RectF32::new(34.0, 44.0, 32.0, 32.0))
        );
    }

    #[test]
    fn cut_outs_follow_the_base_and_annotations_come_after() {
        let mut s = session(200, 400);
        let step = id(s
            .run(
                "annotation.addStep",
                &json!({ "at": { "x": 50, "y": 300 } }),
            )
            .unwrap());
        let cut = id(s
            .run(
                "canvas.cutOut",
                &json!({ "axis": "horizontal", "start": 100, "end": 200, "edge": "torn" }),
            )
            .unwrap());
        let sc = scene(&s);
        // The scene starts at the uncut size; the cut brings it to the canvas.
        assert_eq!((sc.width, sc.height), (200, 400));
        let ids: Vec<u64> = sc.nodes.iter().map(|n| n.id.0).collect();
        assert_eq!(ids, [BASE_NODE_ID, cut, step]);
        let RenderNode::CutOut(c) = &sc.nodes[1].node else {
            panic!("cut out expected");
        };
        assert_eq!((c.axis, c.t0, c.t1), (CutAxis::Horizontal, 100, 200));
        assert!(matches!(c.seam, SeamStyle::TornEdge { .. }) && c.shadow.is_some());
        let RenderNode::Shape(b) = &sc.nodes[2].node else {
            panic!("badge expected");
        };
        // The badge moved with the far half: post-cut coordinates.
        assert_eq!(
            b.geometry,
            ShapeGeometry::Ellipse(RectF32::new(34.0, 184.0, 32.0, 32.0))
        );
        let frame = CpuCompositor::render(&sc).unwrap();
        assert_eq!((frame.width, frame.height), (200, 300));
    }

    #[test]
    fn cut_outs_are_offset_by_the_base_origin() {
        let mut s = session(100, 200);
        {
            let m = &mut s.active_mut().unwrap().manifest;
            m.canvas.size = Size::new(120, 240);
            m.canvas.base_origin = Offset { x: 10, y: 20 };
        }
        // Canvas rows 50..80 are base rows 30..60.
        s.run(
            "canvas.cutOut",
            &json!({ "axis": "horizontal", "start": 50, "end": 80 }),
        )
        .unwrap();
        // A second cut is in the space the first one left.
        s.run(
            "canvas.cutOut",
            &json!({ "axis": "horizontal", "start": 100, "end": 110 }),
        )
        .unwrap();
        s.run(
            "canvas.cutOut",
            &json!({ "axis": "vertical", "start": 30, "end": 40 }),
        )
        .unwrap();
        let sc = scene(&s);
        assert_eq!((sc.width, sc.height), (120, 240));
        let RenderNode::Raster(base) = &sc.nodes[0].node else {
            panic!("base raster expected");
        };
        assert_eq!(base.origin, [10, 20]);
        let cuts: Vec<(CutAxis, u32, u32)> = sc
            .nodes
            .iter()
            .filter_map(|n| match &n.node {
                RenderNode::CutOut(c) => Some((c.axis, c.t0, c.t1)),
                _ => None,
            })
            .collect();
        assert_eq!(
            cuts,
            [
                (CutAxis::Horizontal, 50, 80),
                (CutAxis::Horizontal, 100, 110),
                (CutAxis::Vertical, 30, 40),
            ]
        );
        let frame = CpuCompositor::render(&sc).unwrap();
        let m = &s.active().unwrap().manifest;
        assert_eq!(
            (frame.width, frame.height),
            (m.canvas.size.width, m.canvas.size.height)
        );
        // Base pixels end where the cut base ends: 200 - 40 rows from y = 20.
        assert_eq!(frame.pixel(50, 179)[3], 1.0);
        assert_eq!(frame.pixel(50, 181)[3], 0.0);
    }

    #[test]
    fn invisible_nodes_are_skipped_and_edits_change_revisions() {
        let mut s = session(300, 200);
        let c = id(s
            .run(
                "annotation.addCallout",
                &json!({ "bounds": { "x": 10, "y": 10, "w": 100, "h": 50 },
                         "tails": [{ "tip": { "x": 200, "y": 150 } }] }),
            )
            .unwrap());
        let rev = |s: &Session| {
            scene(s)
                .nodes
                .iter()
                .find(|n| n.id.0 == c)
                .map(|n| n.revision)
        };
        let r0 = rev(&s).unwrap();
        assert_eq!(rev(&s), Some(r0), "stable without edits");
        s.run(
            "annotation.moveCalloutTail",
            &json!({ "id": c, "index": 0, "tip": { "x": 220, "y": 160 } }),
        )
        .unwrap();
        let r1 = rev(&s).unwrap();
        assert_ne!(r0, r1);
        // An opacity change without a touch still invalidates the cache.
        fn node(s: &mut Session, id: u64) -> &mut AnnotationNode {
            s.active_mut()
                .unwrap()
                .manifest
                .node_mut(NodeId(id))
                .unwrap()
        }
        node(&mut s, c).opacity = 0.5;
        let r2 = rev(&s).unwrap();
        assert_ne!(r1, r2);
        let RenderNode::Callout(spec) = &scene(&s).nodes[1].node else {
            panic!("callout expected");
        };
        assert_eq!(spec.fill.map(|f| f.a), Some(0.5));
        node(&mut s, c).visible = false;
        assert_eq!(rev(&s), None);
        assert_eq!(scene(&s).nodes.len(), 1);
        // The base revision follows the pixels.
        let base = scene(&s).nodes[0].revision.0;
        assert_eq!(
            base,
            s.active().unwrap().display_pixels().unwrap().revision()
        );
    }

    #[test]
    fn renders_the_callout_over_the_base() {
        let mut s = session(120, 100);
        s.run(
            "annotation.addCallout",
            &json!({ "bounds": { "x": 20, "y": 20, "w": 60, "h": 40 },
                     "body": { "kind": "rect" },
                     "fill": { "kind": "solid", "color": "#ff0000" },
                     "stroke": null, "shadow": null }),
        )
        .unwrap();
        let frame = CpuCompositor::render(&scene(&s)).unwrap();
        assert_eq!((frame.width, frame.height), (120, 100));
        let px = |x, y| frame.pixel(x, y).map(|v| (v * 255.0).round() as u8);
        assert_eq!(px(50, 40), [255, 0, 0, 255], "callout fill");
        assert_eq!(px(100, 80), [255, 255, 255, 255], "base shows through");
    }
}
