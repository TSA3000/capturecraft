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
//! 4. The canvas backdrop (rounded drop-shadow frame), last: it grows the
//!    image by its margin, so it must follow everything it frames.
//!
//! A rotated node carries its [`AnnotationNode::transform`] (rotation about
//! its pivot) as the scene node's transform; the geometry stays unrotated.
//! Callout text and step badge labels are text specs on their nodes; the
//! compositor shapes them with photocraft-text. The cursor layer is not
//! drawn (its sprite is not loaded into the session).

use std::sync::Arc;

use capturecraft_compositor::{
    Affine, ArrowCurve as SArrowCurve, ArrowHeadShape, ArrowHeadStyle, ArrowSpec,
    BackdropFilterKind, BackdropFilterSpec, CalloutBody as SCalloutBody, CalloutSpec,
    CalloutTail as SCalloutTail, Color as SColor, CutAxis, CutOutSeamSpec, DropShadowStyle,
    GradientStop as SGradientStop, InnerShadowStyle, LineJoin as SLineJoin, MagnifierSpec, Paint,
    PointF32, RasterSpec, RectF32, RenderNode, RenderScene, Resample, SceneNode, SeamShadow,
    SeamStyle, ShadowFrameSpec, ShapeBounds, ShapeGeometry, ShapeSpec, SpotlightSpec, StrokeSpec,
    TailAnchor as STailAnchor, TextAlign as STextAlign, TextRunSpec, TextSpec, TextVAlign,
};
use capturecraft_format::{
    Annotation, AnnotationNode, ArrowCurve, ArrowHead, Axis, BackdropEffect, Background,
    BadgeShape, BlurRegion, Callout, CalloutBody, CanvasBackdrop, Color, CurvedArrow, CutOutSeam,
    DashStyle, DropShadow, Fill, GradientStop, HeadStyle, LensShape, LineJoin, MagnifierLens,
    Manifest, NodeKind, RectF, RegionShape, Sampling, SeamEdge, ShapeNode, SmartObject, Spotlight,
    StepBadge, Stroke, TailAnchor, TextAlign, TextBlock, VAlign,
};
use photocraft_geom::Point;

use crate::{DocState, Pixels};

/// Scene id of the base raster. [`Manifest::alloc_node_id`] never hands
/// out 0, so it cannot collide with an annotation.
pub const BASE_NODE_ID: u64 = 0;

/// Scene id of the canvas backdrop frame. Node ids count up from 1 and a
/// document holds at most [`capturecraft_format::MAX_NODES`], far below it.
pub const FRAME_NODE_ID: u64 = u64::MAX;

/// Feature size of a torn seam's first octave. The format has no
/// wavelength (it stores an octave count instead, which the compositor
/// fixes at its own value).
const TORN_WAVELENGTH_PX: f32 = 24.0;

/// Pixel grid colour of a magnifier in pixel grid mode (not in the format).
const GRID_COLOR: SColor = SColor::new(0.0, 0.0, 0.0, 0.25);

/// A document rendered for export or the clipboard: row-major RGBA8,
/// sRGB, straight (not premultiplied) alpha.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flattened {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Renders `doc` with the CPU compositor (no GPU: exports, the clipboard,
/// the CLI), frame and all.
pub fn flatten(doc: &DocState) -> Result<Flattened, crate::EngineError> {
    let frame = capturecraft_compositor::CpuCompositor::render(&render_scene(doc))
        .map_err(|e| crate::EngineError::Other(format!("render failed: {e}")))?;
    Ok(Flattened {
        width: frame.width,
        height: frame.height,
        rgba: straight_rgba8(&frame.pixels),
    })
}

/// Premultiplied, sRGB-encoded float pixels as straight RGBA8.
pub fn straight_rgba8(pixels: &[[f32; 4]]) -> Vec<u8> {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    pixels
        .iter()
        .flat_map(|p| {
            let a = if p[3].is_finite() {
                p[3].clamp(0.0, 1.0)
            } else {
                0.0
            };
            if a <= 0.0 {
                [0; 4]
            } else {
                [byte(p[0] / a), byte(p[1] / a), byte(p[2] / a), byte(a)]
            }
        })
        .collect()
}

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
            Annotation::SmartObject(_) => match doc.object_pixels(n.id) {
                Some(p) => (Some(raster(p, [0, 0], alpha)), Some(p.revision())),
                None => (None, None),
            },
            Annotation::CutOutSeam(_) => (None, None),
        };
        if let Some(node) = node {
            let extra: Vec<u64> = extra.into_iter().collect();
            let mut t = n.transform();
            if let Annotation::SmartObject(o) = a {
                t = smart_object_placement(o).then(&t);
            }
            nodes.push(SceneNode::new(n.id.0, revision(n, &extra), node).with_transform(t));
        }
    }
    if let Some((spec, rev)) = shadow_frame(&m.canvas.backdrop) {
        nodes.push(SceneNode::new(
            FRAME_NODE_ID,
            rev,
            RenderNode::ShadowFrame(spec),
        ));
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

/// The format's fill as a scene paint, stop colours scaled by the node
/// opacity. Gradient points are canvas space (frame space for a backdrop),
/// like the scene's.
fn fill(f: &Fill, alpha: f32) -> Option<Paint> {
    let stops = |s: &[GradientStop]| -> Vec<SGradientStop> {
        s.iter()
            .map(|g| SGradientStop {
                offset: g.offset,
                color: rgba(g.color, alpha),
            })
            .collect()
    };
    match f {
        Fill::None => None,
        Fill::Solid { color } => Some(Paint::Solid(rgba(*color, alpha))),
        Fill::Linear { from, to, stops: s } => Some(Paint::Linear {
            from: pt(*from),
            to: pt(*to),
            stops: stops(s),
        }),
        Fill::Radial {
            center,
            radius,
            stops: s,
        } => Some(Paint::Radial {
            center: pt(*center),
            radius: *radius,
            stops: stops(s),
        }),
    }
}

/// Callout text in `frame` (the body inset by the padding); `None` when
/// there is nothing to draw.
fn text_block(t: &TextBlock, frame: RectF32, alpha: f32) -> Option<TextSpec> {
    if t.runs.iter().all(|r| r.text.is_empty()) {
        return None;
    }
    Some(TextSpec {
        runs: t
            .runs
            .iter()
            .filter(|r| !r.text.is_empty())
            .map(|r| TextRunSpec {
                text: r.text.clone(),
                family: r.font.family.clone(),
                size_px: r.font.size,
                weight: r.font.weight,
                italic: r.font.italic,
                color: rgba(r.color, alpha),
                underline: r.underline,
                strike: r.strike,
            })
            .collect(),
        frame,
        align: match t.align {
            TextAlign::Left => STextAlign::Left,
            TextAlign::Center => STextAlign::Center,
            TextAlign::Right => STextAlign::Right,
        },
        valign: match t.valign {
            VAlign::Top => TextVAlign::Top,
            VAlign::Middle => TextVAlign::Middle,
            VAlign::Bottom => TextVAlign::Bottom,
        },
        wrap: true,
    })
}

/// `r` shrunk by `pad` on every side (never below zero size).
fn inset(r: RectF32, pad: f32) -> RectF32 {
    let pad = if pad.is_finite() { pad.max(0.0) } else { 0.0 };
    let px = pad.min(r.w * 0.5);
    let py = pad.min(r.h * 0.5);
    RectF32::new(r.x + px, r.y + py, r.w - 2.0 * px, r.h - 2.0 * py)
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

/// Smart object pixels (a surface from `(0, 0)` of `image_size`) → its
/// canvas `rect`: a scale and a translation, before the node's rotation.
fn smart_object_placement(o: &SmartObject) -> Affine {
    let k = |len: f64, px: u32| {
        if px > 0 && len.is_finite() && len > 0.0 {
            len / f64::from(px)
        } else {
            1.0
        }
    };
    Affine {
        m: [
            k(o.rect.w, o.image_size.width),
            0.0,
            0.0,
            k(o.rect.h, o.image_size.height),
            o.rect.x,
            o.rect.y,
        ],
    }
}

/// The rounded drop-shadow frame and a revision derived from its content
/// (the format keeps no revision for it).
fn shadow_frame(b: &CanvasBackdrop) -> Option<(ShadowFrameSpec, u64)> {
    let CanvasBackdrop::RoundedShadowFrame {
        corner_radius_px,
        backdrop_fill,
        shadow: sh,
        border,
        ..
    } = b
    else {
        return None;
    };
    let spec = ShadowFrameSpec {
        margin: b.margin(),
        corner_radius: *corner_radius_px,
        backdrop: fill(backdrop_fill, 1.0).unwrap_or(Paint::Solid(SColor::TRANSPARENT)),
        shadow: sh.as_ref().map(|s| shadow(s, 1.0)),
        border: border.as_ref().map(|s| stroke(s, 1.0)),
    };
    let json = serde_json::to_vec(b).unwrap_or_default();
    let rev = json.chunks(8).fold(mix(json.len() as u64), |h, c| {
        let mut w = [0u8; 8];
        w[..c.len()].copy_from_slice(c);
        mix(h ^ u64::from_le_bytes(w))
    });
    Some((spec, rev))
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
        text: text_block(&c.text, inset(r, c.text.padding), alpha),
    })
}

/// The badge body as a plain shape with its label. The ring is inset by
/// half its width so the badge keeps its radius.
///
/// The label is centred in the badge square at the badge font's size, but
/// never larger than fits: at most 1.1 × the radius, and for long labels
/// (`XVIII`, `AB`…) small enough that its glyphs (about 0.6 em wide each)
/// span at most 1.7 × the radius.
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
    let label = b.label();
    let glyphs = label.chars().count().max(1) as f32;
    let fit = (1.1 * b.radius).min(1.7 * b.radius / (0.6 * glyphs));
    let text = TextSpec {
        runs: vec![TextRunSpec {
            text: label,
            family: b.font.family.clone(),
            size_px: b.font.size.min(fit).max(1.0),
            weight: b.font.weight,
            italic: b.font.italic,
            color: rgba(b.text_color, alpha),
            underline: false,
            strike: false,
        }],
        frame: square,
        align: STextAlign::Center,
        valign: TextVAlign::Middle,
        wrap: false,
    };
    RenderNode::Shape(ShapeSpec {
        geometry,
        fill: Some(rgba(b.fill, alpha).into()),
        stroke: b.ring.as_ref().map(|s| stroke(s, alpha)),
        shadow: b.shadow.as_ref().map(|s| shadow(s, alpha)),
        text: Some(text),
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
/// Pixelate blocks start at the shape's top-left rather than on the canvas
/// grid.
fn blur_region(b: &BlurRegion) -> RenderNode {
    RenderNode::BackdropFilter(BackdropFilterSpec {
        shape: region(&b.shape),
        filter: match b.effect {
            BackdropEffect::Gaussian { sigma } => BackdropFilterKind::GaussianBlur { sigma },
            BackdropEffect::Pixelate { block_size_px } => {
                BackdropFilterKind::Pixelate { block_size_px }
            }
        },
        feather_px: b.feather_px,
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
        text: None,
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
    fn flatten_gives_straight_rgba_of_the_whole_canvas() {
        let mut s = session(40, 30);
        let flat = flatten(s.active().unwrap()).unwrap();
        assert_eq!((flat.width, flat.height), (40, 30));
        assert_eq!(flat.rgba.len(), 40 * 30 * 4);
        assert!(flat.rgba.iter().all(|b| *b == 255), "white capture");
        // The frame grows the export by its margin on every side.
        let frame = capturecraft_format::CanvasBackdrop::RoundedShadowFrame {
            margin_px: 10.0,
            corner_radius_px: 4.0,
            backdrop_fill: capturecraft_format::Fill::Solid {
                color: capturecraft_format::Color::rgb(0xE2, 0xE8, 0xF0),
            },
            shadow: None,
            border: None,
        };
        s.run(
            "canvas.setBackdrop",
            &json!({ "backdrop": serde_json::to_value(&frame).unwrap() }),
        )
        .unwrap();
        let framed = flatten(s.active().unwrap()).unwrap();
        assert_eq!((framed.width, framed.height), (60, 50));
        assert_eq!(&framed.rgba[..4], &[0xE2, 0xE8, 0xF0, 0xFF]);
    }

    #[test]
    fn premultiplied_pixels_are_unpremultiplied() {
        let px = [
            [0.25, 0.0, 0.5, 0.5],
            [0.0; 4],
            [f32::NAN, 0.0, 0.0, f32::NAN],
        ];
        assert_eq!(
            straight_rgba8(&px),
            [128, 0, 255, 128, 0, 0, 0, 0, 0, 0, 0, 0]
        );
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
        assert_eq!(c.fill, Some(SColor::WHITE.into()));
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
        assert!(matches!(spec.fill, Some(capturecraft_compositor::Paint::Solid(c)) if c.a == 0.5));
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

    fn push(s: &mut Session, a: Annotation) -> u64 {
        let m = &mut s.active_mut().unwrap().manifest;
        let id = m.alloc_node_id();
        m.annotations.push(AnnotationNode::new(id, a));
        id.0
    }

    #[test]
    fn rotated_nodes_carry_their_transform() {
        let mut s = session(200, 200);
        let c = id(s
            .run(
                "annotation.addCallout",
                &json!({ "bounds": { "x": 20, "y": 30, "w": 100, "h": 40 }, "text": "Hi" }),
            )
            .unwrap());
        assert_eq!(scene(&s).nodes[1].transform, None);
        s.run("node.rotate", &json!({ "id": c, "rotation_rad": 0.5 }))
            .unwrap();
        let sc = scene(&s);
        let n = &sc.nodes[1];
        let want = s
            .active()
            .unwrap()
            .manifest
            .node(NodeId(c))
            .unwrap()
            .transform();
        assert_eq!(n.transform, Some(want));
        // The pivot (body centre) stays put.
        let p = want.apply(Point::new(70.0, 50.0));
        assert!((p.x - 70.0).abs() < 1e-9 && (p.y - 50.0).abs() < 1e-9);
    }

    #[test]
    fn smart_objects_scale_into_their_rect() {
        let o = SmartObject {
            rect: RectF::new(10.0, 20.0, 80.0, 30.0),
            image_size: Size::new(40, 60),
            ..SmartObject::default()
        };
        let t = smart_object_placement(&o);
        let a = t.apply(Point::new(0.0, 0.0));
        let b = t.apply(Point::new(40.0, 60.0));
        assert_eq!((a.x, a.y), (10.0, 20.0));
        assert_eq!((b.x, b.y), (90.0, 50.0));
    }

    #[test]
    fn gradients_become_paints() {
        let mut s = session(100, 100);
        let stops = vec![
            GradientStop {
                offset: 0.0,
                color: Color::rgb(255, 0, 0),
            },
            GradientStop {
                offset: 1.0,
                color: Color::rgb(0, 0, 255),
            },
        ];
        push(
            &mut s,
            Annotation::Shape(ShapeNode {
                fill: Fill::Linear {
                    from: Point::new(0.0, 0.0),
                    to: Point::new(100.0, 0.0),
                    stops: stops.clone(),
                },
                ..ShapeNode::default()
            }),
        );
        push(
            &mut s,
            Annotation::Shape(ShapeNode {
                fill: Fill::Radial {
                    center: Point::new(50.0, 50.0),
                    radius: 25.0,
                    stops,
                },
                ..ShapeNode::default()
            }),
        );
        s.active_mut().unwrap().manifest.annotations[0].opacity = 0.5;
        let sc = scene(&s);
        let RenderNode::Shape(lin) = &sc.nodes[1].node else {
            panic!("shape expected");
        };
        let Some(Paint::Linear { from, to, stops }) = &lin.fill else {
            panic!("linear paint expected: {:?}", lin.fill);
        };
        assert_eq!(
            (*from, *to),
            (PointF32::new(0.0, 0.0), PointF32::new(100.0, 0.0))
        );
        assert_eq!(stops[0].color, SColor::new(1.0, 0.0, 0.0, 0.5));
        assert_eq!(stops[1].offset, 1.0);
        let RenderNode::Shape(rad) = &sc.nodes[2].node else {
            panic!("shape expected");
        };
        assert!(matches!(
            &rad.fill,
            Some(Paint::Radial { center, radius, stops })
                if *center == PointF32::new(50.0, 50.0) && *radius == 25.0 && stops.len() == 2
        ));
    }

    #[test]
    fn callout_text_fills_the_padded_body() {
        let mut s = session(300, 200);
        let c = id(s
            .run(
                "annotation.addCallout",
                &json!({ "bounds": { "x": 20, "y": 30, "w": 160, "h": 80 }, "text": "Hello" }),
            )
            .unwrap());
        let sc = scene(&s);
        let RenderNode::Callout(spec) = &sc.nodes[1].node else {
            panic!("callout expected");
        };
        let t = spec.text.as_ref().unwrap();
        let pad = s
            .active()
            .unwrap()
            .manifest
            .node(NodeId(c))
            .and_then(|n| match n.annotation() {
                Some(Annotation::Callout(c)) => Some(c.text.padding),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            t.frame,
            RectF32::new(20.0 + pad, 30.0 + pad, 160.0 - 2.0 * pad, 80.0 - 2.0 * pad)
        );
        assert_eq!(t.runs.len(), 1);
        assert_eq!(t.runs[0].text, "Hello");
        assert!(t.wrap);
        // Empty text draws nothing.
        let Some(Annotation::Callout(cc)) = s
            .active_mut()
            .unwrap()
            .manifest
            .node_mut(NodeId(c))
            .unwrap()
            .annotation_mut()
        else {
            panic!("callout expected");
        };
        cc.text.runs.clear();
        let RenderNode::Callout(spec) = &scene(&s).nodes[1].node else {
            panic!("callout expected");
        };
        assert_eq!(spec.text, None);
    }

    #[test]
    fn step_badges_draw_their_label() {
        let mut s = session(200, 200);
        s.run(
            "annotation.addStep",
            &json!({ "at": { "x": 50, "y": 60 }, "numbering": "upper_alpha" }),
        )
        .unwrap();
        s.run(
            "annotation.addStep",
            &json!({ "at": { "x": 100, "y": 60 }, "numbering": "upper_roman", "value": 18 }),
        )
        .unwrap();
        let sc = scene(&s);
        let RenderNode::Shape(a) = &sc.nodes[1].node else {
            panic!("badge expected");
        };
        let t = a.text.as_ref().unwrap();
        assert_eq!(t.runs[0].text, "A");
        assert_eq!(t.frame, RectF32::new(34.0, 44.0, 32.0, 32.0));
        assert_eq!(
            (t.align, t.valign, t.wrap),
            (STextAlign::Center, TextVAlign::Middle, false)
        );
        assert_eq!(t.runs[0].color, SColor::WHITE);
        let short = t.runs[0].size_px;
        let RenderNode::Shape(b) = &sc.nodes[2].node else {
            panic!("badge expected");
        };
        let long = &b.text.as_ref().unwrap().runs[0];
        assert_eq!(long.text, "XVIII");
        assert!(long.size_px < short, "{} vs {short}", long.size_px);
    }

    #[test]
    fn blur_feather_reaches_the_scene() {
        let mut s = session(100, 100);
        push(
            &mut s,
            Annotation::BlurRegion(BlurRegion {
                feather_px: 6.0,
                ..BlurRegion::default()
            }),
        );
        let RenderNode::BackdropFilter(b) = &scene(&s).nodes[1].node else {
            panic!("blur expected");
        };
        assert_eq!(b.feather_px, 6.0);
    }

    #[test]
    fn the_shadow_frame_comes_last_with_its_margin() {
        let mut s = session(120, 80);
        s.run("annotation.addStep", &json!({ "at": { "x": 50, "y": 60 } }))
            .unwrap();
        assert!(
            !scene(&s)
                .nodes
                .iter()
                .any(|n| matches!(n.node, RenderNode::ShadowFrame(_)))
        );
        let frame = serde_json::to_value(CanvasBackdrop::rounded_shadow_frame()).unwrap();
        s.run("canvas.setBackdrop", &json!({ "backdrop": frame }))
            .unwrap();
        let sc = scene(&s);
        let last = sc.nodes.last().unwrap();
        assert_eq!(last.id.0, FRAME_NODE_ID);
        let RenderNode::ShadowFrame(f) = &last.node else {
            panic!("frame expected");
        };
        assert_eq!(f.margin, 48);
        assert_eq!(f.corner_radius, 12.0);
        assert!(f.shadow.is_some() && f.border.is_none());
        assert_eq!(
            f.backdrop,
            Paint::Solid(SColor::rgba8(0xE2, 0xE8, 0xF0, 255))
        );
        // The revision follows the content.
        let r1 = last.revision;
        let mut wider = CanvasBackdrop::rounded_shadow_frame();
        if let CanvasBackdrop::RoundedShadowFrame { margin_px, .. } = &mut wider {
            *margin_px = 64.0;
        }
        s.run(
            "canvas.setBackdrop",
            &json!({ "backdrop": serde_json::to_value(wider).unwrap() }),
        )
        .unwrap();
        let sc = scene(&s);
        let last = sc.nodes.last().unwrap();
        assert_ne!(last.revision, r1);
        let RenderNode::ShadowFrame(f) = &last.node else {
            panic!("frame expected");
        };
        assert_eq!(f.margin, 64);
    }
}
