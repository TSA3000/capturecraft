//! Scene nodes → backend-neutral compositing ops.
//!
//! Preparing a node does all vector work once (outline union, strokes,
//! exact-area coverage) and leaves only per-pixel work: blending masks,
//! blurring masks, sampling the backdrop. Both compositors cache prepared
//! nodes by `(NodeId, RevisionId)` and run the ops in order; the CPU runs
//! them on rayon tiles, the GPU as compute dispatches.
//!
//! Each op also states its data flow for damage tracking: where it writes
//! ([`Op::write_rect`]), which input pixels an output region reads
//! ([`Op::read_rects`]) and which output pixels an input change reaches
//! ([`Op::forward`]).

use std::sync::Arc;

use photocraft_geom::Rect;
use photocraft_raster::Surface;
use photocraft_vector::{Polyline, flatten_path, shapes};

use crate::effects::backdrop::{LensMode, LensSampler};
use crate::effects::cutout::CutParams;
use crate::effects::{Kernel, Mask, Px};
use crate::primitives::{self, P, Silhouette, TOLERANCE};
use crate::scene::{
    BackdropFilterKind, BackdropFilterSpec, Color, DropShadowStyle, LineCap, MagnifierSpec,
    RasterSpec, RenderNode, Resample, ShapeBounds, SpotlightSpec, StrokeSpec,
};

/// Source-over of raster pixels.
#[derive(Clone, Debug)]
pub struct ImageOp {
    pub surface: Arc<Surface>,
    pub origin: [i32; 2],
    pub opacity: f32,
    /// Canvas rectangle of the surface's tiles (where the op draws).
    pub region: Rect,
}

/// Source-over of a colour through a coverage mask. The effective coverage
/// is `blur(mask)`, inverted if `invert`, times `clip` if present.
#[derive(Clone, Debug)]
pub struct FillOp {
    pub mask: Arc<Mask>,
    pub blur: Option<Arc<Kernel>>,
    pub clip: Option<Arc<Mask>>,
    pub invert: bool,
    pub color: Px,
}

impl FillOp {
    pub fn coverage_rect(&self) -> Rect {
        self.clip.as_ref().map_or(self.mask.rect, |c| c.rect)
    }
}

#[derive(Clone, Debug)]
pub struct BlurOp {
    pub mask: Arc<Mask>,
    pub kernel: Arc<Kernel>,
}

#[derive(Clone, Debug)]
pub struct PixelateOp {
    pub mask: Arc<Mask>,
    pub block: i32,
    /// Top-left of the block grid.
    pub origin: [i32; 2],
}

impl PixelateOp {
    /// Pixel rectangle of the blocks touching `r`, and the block index range
    /// `(bx0, by0, nbx, nby)`.
    pub fn blocks(&self, r: Rect) -> (Rect, [i32; 4]) {
        if r.is_empty() {
            return (Rect::EMPTY, [0; 4]);
        }
        let b = self.block;
        let bx0 = (r.x0 - self.origin[0]).div_euclid(b);
        let by0 = (r.y0 - self.origin[1]).div_euclid(b);
        let bx1 = (r.x1 - 1 - self.origin[0]).div_euclid(b) + 1;
        let by1 = (r.y1 - 1 - self.origin[1]).div_euclid(b) + 1;
        let px = Rect::new(
            self.origin[0] + bx0 * b,
            self.origin[1] + by0 * b,
            self.origin[0] + bx1 * b,
            self.origin[1] + by1 * b,
        );
        (px, [bx0, by0, bx1 - bx0, by1 - by0])
    }
}

#[derive(Clone, Debug)]
pub struct LensOp {
    pub mask: Arc<Mask>,
    pub sampler: LensSampler,
}

#[derive(Clone, Debug)]
pub struct SpotlightOp {
    /// Target coverage (before feathering); `None` = no targets, all dimmed.
    pub mask: Option<Arc<Mask>>,
    pub feather: Option<Arc<Kernel>>,
    pub blur: Option<Arc<Kernel>>,
    pub dim_rgb: [f32; 3],
    pub dim_a: f32,
}

#[derive(Clone, Debug)]
pub enum Op {
    Image(ImageOp),
    Fill(FillOp),
    Blur(BlurOp),
    Pixelate(PixelateOp),
    Lens(LensOp),
    Spotlight(SpotlightOp),
    Cut(CutParams),
}

fn canvas_rect(size: (u32, u32)) -> Rect {
    Rect::new(0, 0, size.0 as i32, size.1 as i32)
}

/// `r` moved into the canvas, at least one pixel wide and high: the pixels
/// a read clamped to the canvas edge actually touches.
pub fn clamp_into(r: Rect, size: (u32, u32)) -> Rect {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let x0 = r.x0.clamp(0, w - 1);
    let y0 = r.y0.clamp(0, h - 1);
    Rect::new(x0, y0, r.x1.clamp(x0 + 1, w), r.y1.clamp(y0 + 1, h))
}

impl Op {
    /// Canvas size after the op.
    pub fn out_size(&self, size: (u32, u32)) -> (u32, u32) {
        match self {
            Op::Cut(c) => (c.dst_w, c.dst_h),
            _ => size,
        }
    }

    /// Output pixels the op may change (all others pass through), in the
    /// output canvas.
    pub fn write_rect(&self, size: (u32, u32)) -> Rect {
        let c = canvas_rect(self.out_size(size));
        match self {
            Op::Image(o) => o.region.intersect(&c),
            Op::Fill(o) => o.coverage_rect().intersect(&c),
            Op::Blur(o) => o.mask.rect.intersect(&c),
            Op::Pixelate(o) => o.mask.rect.intersect(&c),
            Op::Lens(o) => o.mask.rect.intersect(&c),
            Op::Spotlight(_) | Op::Cut(_) => c,
        }
    }

    /// Input pixels needed to compute the output inside `out` (already
    /// clipped to the write rectangle by the caller).
    pub fn read_rects(&self, out: Rect, size: (u32, u32)) -> Vec<Rect> {
        let c = canvas_rect(size);
        match self {
            Op::Image(_) | Op::Fill(_) => vec![out],
            Op::Blur(o) => vec![out.inflate(o.kernel.radius as i32).intersect(&c)],
            Op::Pixelate(o) => vec![o.blocks(out).0.intersect(&c), out],
            Op::Lens(o) => vec![clamp_into(o.sampler.footprint(out), size), out],
            Op::Spotlight(o) => vec![
                o.blur
                    .as_ref()
                    .map_or(out, |k| out.inflate(k.radius as i32).intersect(&c)),
            ],
            Op::Cut(p) => p.read_rects(out).to_vec(),
        }
    }

    /// Output pixels that change when input `r` changes.
    pub fn forward(&self, r: Rect, size: (u32, u32)) -> Vec<Rect> {
        let w = self.write_rect(size);
        let one = match self {
            Op::Image(_) | Op::Fill(_) => r,
            Op::Blur(o) => r.union(&r.inflate(o.kernel.radius as i32).intersect(&w)),
            Op::Pixelate(o) => r.union(&o.blocks(r).0.intersect(&w)),
            // Any change under the lens footprint may show anywhere in the
            // lens, which usually sits elsewhere: two rectangles.
            Op::Lens(o) => {
                if clamp_into(o.sampler.footprint(w), size)
                    .intersect(&r)
                    .is_empty()
                {
                    r
                } else {
                    return vec![r, w];
                }
            }
            Op::Spotlight(o) => o
                .blur
                .as_ref()
                .map_or(r, |k| r.inflate(k.radius as i32).intersect(&w)),
            Op::Cut(p) => p.forward(r),
        };
        vec![one]
    }

    /// Reads input pixels other than the one it writes.
    pub fn reads_neighbours(&self) -> bool {
        match self {
            Op::Image(_) | Op::Fill(_) => false,
            Op::Spotlight(o) => o.blur.is_some(),
            _ => true,
        }
    }
}

/// A prepared node: its ops, in order.
#[derive(Clone, Debug, Default)]
pub struct Prepared {
    pub ops: Vec<Op>,
}

impl Prepared {
    /// Union of the ops' write rectangles (the node's footprint).
    pub fn bounds(&self, size: (u32, u32)) -> Rect {
        let mut s = size;
        let mut r = Rect::EMPTY;
        for op in &self.ops {
            r = r.union(&op.write_rect(s));
            s = op.out_size(s);
        }
        r
    }

    pub fn out_size(&self, size: (u32, u32)) -> (u32, u32) {
        self.ops.iter().fold(size, |s, op| op.out_size(s))
    }
}

fn fill_op(components: &[Vec<Vec<P>>], color: Color, size: (u32, u32)) -> Option<Op> {
    let c = color.premultiplied();
    if c[3] <= 0.0 {
        return None;
    }
    let mask = Mask::rasterize(components, canvas_rect(size), 0)?;
    Some(Op::Fill(FillOp {
        mask: Arc::new(mask),
        blur: None,
        clip: None,
        invert: false,
        color: c,
    }))
}

fn shadow_op(sil: &Silhouette, s: &DropShadowStyle, size: (u32, u32)) -> Option<Op> {
    let color = s.color.premultiplied();
    if color[3] <= 0.0 {
        return None;
    }
    let kernel = Kernel::gaussian(s.blur_sigma).map(Arc::new);
    let pad = kernel.as_ref().map_or(0, |k| k.radius);
    // Geometry outside the canvas only matters within one blur radius.
    let clip = canvas_rect(size).inflate(pad as i32);
    let mask = Mask::rasterize(&sil.shadow_components(s), clip, pad)?;
    Some(Op::Fill(FillOp {
        mask: Arc::new(mask),
        blur: kernel,
        clip: None,
        invert: false,
        color,
    }))
}

/// Shadow, fill, stroke of a vector silhouette.
fn vector_ops(
    sil: &Silhouette,
    fill: Option<Color>,
    stroke: Option<&StrokeSpec>,
    shadow: Option<&DropShadowStyle>,
    size: (u32, u32),
) -> Vec<Op> {
    if sil.is_empty() {
        return Vec::new();
    }
    let mut ops = Vec::new();
    ops.extend(shadow.and_then(|s| shadow_op(sil, s, size)));
    if let Some(f) = fill {
        ops.extend(fill_op(std::slice::from_ref(&sil.fill), f, size));
    }
    if let Some(s) = stroke {
        ops.extend(fill_op(std::slice::from_ref(&sil.stroke), s.color, size));
    }
    ops
}

fn image_op(r: &RasterSpec, size: (u32, u32)) -> Option<Op> {
    let opacity = if r.opacity.is_finite() {
        r.opacity.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let tb = r.surface.tile_bounds();
    if opacity <= 0.0 || tb.is_empty() {
        return None;
    }
    let region = tb.translate(r.origin[0], r.origin[1]);
    if region.intersect(&canvas_rect(size)).is_empty() {
        return None;
    }
    Some(Op::Image(ImageOp {
        surface: r.surface.clone(),
        origin: r.origin,
        opacity,
        region,
    }))
}

fn lens_loops(lens: &ShapeBounds) -> Vec<Polyline> {
    let path = match *lens {
        ShapeBounds::Circle { center, radius }
            if primitives::point_ok(center) && radius.is_finite() && radius > 0.0 =>
        {
            let (cx, cy, r) = (
                f64::from(center.x),
                f64::from(center.y),
                f64::from(radius.min(1e5)),
            );
            shapes::ellipse(cx - r, cy - r, r * 2.0, r * 2.0)
        }
        ShapeBounds::RoundedRect { rect, radius } if rect.is_valid() => {
            let rad = if radius.is_finite() {
                f64::from(radius.max(0.0))
            } else {
                0.0
            };
            shapes::rounded_rect(
                rect.x.into(),
                rect.y.into(),
                rect.w.into(),
                rect.h.into(),
                [rad; 4],
            )
        }
        _ => return Vec::new(),
    };
    flatten_path(&path, TOLERANCE)
}

/// First point where the segment `a → b` crosses `loops` (from `a`).
fn first_crossing(loops: &[Polyline], a: P, b: P) -> Option<P> {
    let d = (b.0 - a.0, b.1 - a.1);
    let mut best: Option<f64> = None;
    for l in loops {
        let n = l.pts.len();
        for i in 0..n {
            let (p, q) = (l.pts[i], l.pts[(i + 1) % n]);
            let e = (q.0 - p.0, q.1 - p.1);
            let den = d.0 * e.1 - d.1 * e.0;
            if den.abs() < 1e-12 {
                continue;
            }
            let w = (p.0 - a.0, p.1 - a.1);
            let t = (w.0 * e.1 - w.1 * e.0) / den;
            let u = (w.0 * d.1 - w.1 * d.0) / den;
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) && best.is_none_or(|bt| t < bt)
            {
                best = Some(t);
            }
        }
    }
    best.map(|t| (a.0 + d.0 * t, a.1 + d.1 * t))
}

fn magnifier_ops(m: &MagnifierSpec, size: (u32, u32)) -> Vec<Op> {
    if !primitives::point_ok(m.source_center) {
        return Vec::new();
    }
    let lens = lens_loops(&m.lens_bounds);
    if lens.is_empty() {
        return Vec::new();
    }
    let zoom = if m.zoom_factor.is_finite() {
        m.zoom_factor.clamp(1.5, 8.0)
    } else {
        2.0
    };
    let lens_fill: Vec<Vec<P>> = lens.iter().map(|l| l.pts.clone()).collect();
    let border = m
        .border_stroke
        .as_ref()
        .map(|s| primitives::stroke(&lens, s, LineCap::Butt))
        .unwrap_or_default();
    let canvas = canvas_rect(size);
    let mut ops = Vec::new();

    if let Some(s) = &m.drop_shadow {
        let sil = Silhouette {
            fill: lens_fill.clone(),
            stroke: border.clone(),
            outline: lens.clone(),
        };
        ops.extend(shadow_op(&sil, s, size));
    }
    // Connector line to the rim and a ring around the magnified area.
    if let Some(c) = &m.connector {
        let center = m.lens_bounds.center().f64();
        let src = m.source_center.f64();
        let mut lines = Vec::new();
        if let Some(rim) = first_crossing(&lens, src, center) {
            lines.push(Polyline {
                pts: vec![src, rim],
                knot: vec![true; 2],
                closed: false,
            });
        }
        let ring = match m.lens_bounds {
            ShapeBounds::Circle { radius, .. } => {
                let r = f64::from(radius) / f64::from(zoom);
                flatten_path(
                    &shapes::ellipse(src.0 - r, src.1 - r, r * 2.0, r * 2.0),
                    TOLERANCE,
                )
            }
            ShapeBounds::RoundedRect { rect, radius } => {
                let (w, h) = (
                    f64::from(rect.w) / f64::from(zoom),
                    f64::from(rect.h) / f64::from(zoom),
                );
                let rad = f64::from(radius.max(0.0)) / f64::from(zoom);
                flatten_path(
                    &shapes::rounded_rect(src.0 - w * 0.5, src.1 - h * 0.5, w, h, [rad; 4]),
                    TOLERANCE,
                )
            }
        };
        lines.extend(ring);
        let polys = primitives::stroke(&lines, c, LineCap::Round);
        ops.extend(fill_op(&[polys], c.color, size));
    }
    let Some(lens_mask) =
        Mask::rasterize(std::slice::from_ref(&lens_fill), canvas, 0).map(Arc::new)
    else {
        return ops;
    };
    let center = m.lens_bounds.center();
    let mode = if m.pixel_grid_mode && zoom >= 4.0 {
        LensMode::PixelGrid
    } else {
        match m.resample {
            Resample::CatmullRom => LensMode::CatmullRom,
            Resample::Lanczos3 => LensMode::Lanczos3,
            Resample::Nearest => LensMode::Nearest,
        }
    };
    let sampler = LensSampler {
        mode,
        center: [center.x, center.y],
        source: [m.source_center.x, m.source_center.y],
        inv_zoom: 1.0 / zoom,
        grid: m.grid_color.premultiplied(),
        width: size.0 as i32,
        height: size.1 as i32,
    };
    ops.push(Op::Lens(LensOp {
        mask: lens_mask.clone(),
        sampler,
    }));
    if let Some(s) = &m.inner_shadow {
        let color = s.color.premultiplied();
        let kernel = Kernel::gaussian(s.blur_sigma).map(Arc::new);
        let pad = kernel.as_ref().map_or(0, |k| k.radius);
        let d = (f64::from(s.offset.x), f64::from(s.offset.y));
        if color[3] > 0.0 && d.0.is_finite() && d.1.is_finite() {
            let moved = primitives::translate(&lens_fill, d);
            // Inside the lens: 1 - blur(lens moved by the offset).
            if let Some(mask) = Mask::rasterize(&[moved], canvas.inflate(pad as i32), pad) {
                ops.push(Op::Fill(FillOp {
                    mask: Arc::new(mask),
                    blur: kernel,
                    clip: Some(lens_mask.clone()),
                    invert: true,
                    color,
                }));
            }
        }
    }
    if let Some(s) = &m.border_stroke {
        ops.extend(fill_op(&[border], s.color, size));
    }
    ops
}

fn backdrop_ops(b: &BackdropFilterSpec, size: (u32, u32)) -> Vec<Op> {
    let outline = primitives::geometry_outline(&b.shape);
    let polys: Vec<Vec<P>> = outline.iter().map(|l| l.pts.clone()).collect();
    let Some(mask) = Mask::rasterize(&[polys], canvas_rect(size), 0).map(Arc::new) else {
        return Vec::new();
    };
    match b.filter {
        BackdropFilterKind::GaussianBlur { sigma } => Kernel::gaussian(sigma)
            .map(|k| {
                Op::Blur(BlurOp {
                    mask,
                    kernel: Arc::new(k),
                })
            })
            .into_iter()
            .collect(),
        BackdropFilterKind::Pixelate { block_size_px } => {
            let block = block_size_px.clamp(1, 512) as i32;
            if block == 1 {
                return Vec::new();
            }
            let origin = [mask.rect.x0, mask.rect.y0];
            vec![Op::Pixelate(PixelateOp {
                mask,
                block,
                origin,
            })]
        }
    }
}

fn spotlight_ops(s: &SpotlightSpec, size: (u32, u32)) -> Vec<Op> {
    let dim_a = if s.dim_opacity.is_finite() {
        s.dim_opacity.clamp(0.0, 1.0)
    } else {
        0.0
    } * s.dim_color.premultiplied()[3];
    let blur = Kernel::gaussian(s.blur_sigma).map(Arc::new);
    if dim_a <= 0.0 && blur.is_none() {
        return Vec::new();
    }
    let feather = Kernel::gaussian(s.feather_px * 0.5).map(Arc::new);
    let pad = feather.as_ref().map_or(0, |k| k.radius);
    let comps: Vec<Vec<Vec<P>>> = s
        .targets
        .iter()
        .map(|t| {
            primitives::geometry_outline(t)
                .into_iter()
                .map(|l| l.pts)
                .collect()
        })
        .collect();
    let mask = Mask::rasterize(&comps, canvas_rect(size).inflate(pad as i32), pad).map(Arc::new);
    let c = s.dim_color;
    let ch = |v: f32| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    vec![Op::Spotlight(SpotlightOp {
        mask,
        feather,
        blur,
        dim_rgb: [ch(c.r), ch(c.g), ch(c.b)],
        dim_a,
    })]
}

/// Prepares one node for a canvas of `size` (the size at its z position).
pub fn prepare(node: &RenderNode, size: (u32, u32)) -> Prepared {
    let ops = match node {
        RenderNode::Raster(r) => image_op(r, size).into_iter().collect(),
        RenderNode::Shape(s) => vector_ops(
            &primitives::shape_silhouette(s),
            s.fill,
            s.stroke.as_ref(),
            s.shadow.as_ref(),
            size,
        ),
        RenderNode::Callout(c) => vector_ops(
            &primitives::callout::silhouette(c),
            c.fill,
            c.stroke.as_ref(),
            c.shadow.as_ref(),
            size,
        ),
        RenderNode::Arrow(a) => {
            let sil = primitives::arrow_step::silhouette(a);
            let mut ops = Vec::new();
            if !sil.is_empty() {
                ops.extend(a.shadow.as_ref().and_then(|s| shadow_op(&sil, s, size)));
                // Shaft and heads in one mask: overlaps are covered once.
                ops.extend(fill_op(
                    &[sil.fill.clone(), sil.stroke.clone()],
                    a.color,
                    size,
                ));
            }
            ops
        }
        RenderNode::Magnifier(m) => magnifier_ops(m, size),
        RenderNode::BackdropFilter(b) => backdrop_ops(b, size),
        RenderNode::Spotlight(s) => spotlight_ops(s, size),
        RenderNode::CutOut(c) => CutParams::new(c, size.0, size.1)
            .map(Op::Cut)
            .into_iter()
            .collect(),
    };
    Prepared { ops }
}
