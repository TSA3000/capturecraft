//! Scene nodes → backend-neutral compositing ops.
//!
//! Preparing a node does all vector work once (outline union, strokes,
//! text shaping, the node transform, exact-area coverage, feathered edges,
//! the shadow frame's blurred shadow) and leaves only per-pixel work:
//! blending masks with a colour or gradient, blurring masks, sampling the
//! backdrop. Both compositors cache prepared nodes by `(NodeId, RevisionId,
//! transform)` and run the ops in order; the CPU runs them on rayon tiles,
//! the GPU as compute dispatches.
//!
//! Each op also states its data flow for damage tracking: where it writes
//! ([`Op::write_rect`]), which input pixels an output region reads
//! ([`Op::read_rects`]) and which output pixels an input change reaches
//! ([`Op::forward`]).

use std::sync::Arc;

use photocraft_algo::transform::{Homography, Interp, warp_surface_clipped};
use photocraft_geom::{Affine, Point, Rect};
use photocraft_raster::Surface;
use photocraft_vector::{Polyline, flatten_path, shapes};

use crate::effects::backdrop::{LensMode, LensSampler};
use crate::effects::cutout::CutParams;
use crate::effects::{
    Gradient, Kernel, MAX_MASK_PX, MAX_SIGMA, MAX_STOPS, Mask, PaintPx, Px, feather_mask, over,
    scale,
};
use crate::primitives::{self, P, Silhouette, TOLERANCE};
use crate::scene::{
    BackdropFilterKind, BackdropFilterSpec, DropShadowStyle, LineCap, MagnifierSpec, Paint,
    RasterSpec, RenderNode, Resample, ShadowFrameSpec, ShapeBounds, SpotlightSpec, StrokeSpec,
    TextSpec,
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

/// Source-over of a paint through a coverage mask. The effective coverage
/// is `blur(mask)`, inverted if `invert`, times `clip` if present.
#[derive(Clone, Debug)]
pub struct FillOp {
    pub mask: Arc<Mask>,
    pub blur: Option<Arc<Kernel>>,
    pub clip: Option<Arc<Mask>>,
    pub invert: bool,
    pub paint: PaintPx,
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
    /// Target coverage, edge already feathered; `None` = no targets, all
    /// dimmed.
    pub mask: Option<Arc<Mask>>,
    pub blur: Option<Arc<Kernel>>,
    pub dim_rgb: [f32; 3],
    pub dim_a: f32,
}

/// The shadow frame (see [`ShadowFrameSpec`]): every output pixel is the
/// backdrop, the blurred shadow over it, the input canvas moved by
/// `margin` and clipped to the rounded rectangle over that, the border on
/// top. All masks are in output (frame) coordinates. Mirrors `frame`.
#[derive(Clone, Debug)]
pub struct FrameOp {
    pub margin: i32,
    /// Input canvas size.
    pub src: (u32, u32),
    /// Output size: the input plus the margin on every side.
    pub dst: (u32, u32),
    pub backdrop: PaintPx,
    /// Already blurred; premultiplied colour.
    pub shadow: Option<(Arc<Mask>, Px)>,
    pub clip: Arc<Mask>,
    pub border: Option<(Arc<Mask>, Px)>,
}

impl FrameOp {
    /// Output pixel `(x, y)`; `src` reads the input canvas (transparent
    /// outside it).
    #[inline]
    pub fn compose(&self, src: &dyn Fn(i32, i32) -> Px, x: i32, y: i32) -> Px {
        let mut out = self.backdrop.at(x, y);
        if let Some((m, c)) = &self.shadow {
            let s = m.at(x, y);
            if s > 0.0 {
                out = over(scale(*c, s), out);
            }
        }
        let k = self.clip.at(x, y);
        if k > 0.0 {
            out = over(scale(src(x - self.margin, y - self.margin), k), out);
        }
        if let Some((m, c)) = &self.border {
            let b = m.at(x, y);
            if b > 0.0 {
                out = over(scale(*c, b), out);
            }
        }
        out
    }
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
    Frame(FrameOp),
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
            Op::Frame(f) => f.dst,
            _ => size,
        }
    }

    /// Restructures the canvas (a new canvas of [`Op::out_size`]).
    pub fn resizes(&self) -> bool {
        matches!(self, Op::Cut(_) | Op::Frame(_))
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
            Op::Spotlight(_) | Op::Cut(_) | Op::Frame(_) => c,
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
            Op::Frame(f) => vec![out.translate(-f.margin, -f.margin).intersect(&c)],
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
            Op::Frame(f) => r.translate(f.margin, f.margin).intersect(&w),
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

/// `v` clamped to `0..=1` (non-finite values become 0).
fn unit(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The paint the compositors evaluate, or `None` when it shows nothing.
/// A degenerate gradient (zero length or radius) paints its last stop.
pub fn paint_px(p: &Paint) -> Option<PaintPx> {
    let out = match p {
        Paint::Solid(c) => PaintPx::Solid(c.premultiplied()),
        Paint::Linear { stops, .. } | Paint::Radial { stops, .. } => {
            let mut s: Vec<(f32, [f32; 4])> = stops
                .iter()
                .take(MAX_STOPS)
                .map(|g| {
                    let c = g.color;
                    (unit(g.offset), [unit(c.r), unit(c.g), unit(c.b), unit(c.a)])
                })
                .collect();
            s.sort_by(|a, b| a.0.total_cmp(&b.0));
            let last = s.last()?.1;
            let solid = PaintPx::Solid([
                last[0] * last[3],
                last[1] * last[3],
                last[2] * last[3],
                last[3],
            ]);
            let geo = match *p {
                Paint::Linear { from, to, .. } => {
                    let (dx, dy) = (f64::from(to.x - from.x), f64::from(to.y - from.y));
                    let l2 = dx * dx + dy * dy;
                    if !(l2.is_finite() && l2 > 1e-12 && primitives::point_ok(from)) {
                        return Some(solid).filter(|p| !p.is_clear());
                    }
                    Some((false, [from.x, from.y, (dx / l2) as f32, (dy / l2) as f32]))
                }
                Paint::Radial { center, radius, .. } => {
                    if !(radius.is_finite() && radius > 1e-6 && primitives::point_ok(center)) {
                        return Some(solid).filter(|p| !p.is_clear());
                    }
                    Some((true, [center.x, center.y, 1.0 / radius, 0.0]))
                }
                Paint::Solid(_) => None,
            };
            match geo {
                Some((radial, geo)) => PaintPx::Gradient(Gradient {
                    radial,
                    geo,
                    stops: s,
                }),
                None => solid,
            }
        }
    };
    (!out.is_clear()).then_some(out)
}

fn fill_op(components: &[Vec<Vec<P>>], paint: PaintPx, size: (u32, u32)) -> Option<Op> {
    if paint.is_clear() {
        return None;
    }
    let mask = Mask::rasterize(components, canvas_rect(size), 0)?;
    Some(Op::Fill(FillOp {
        mask: Arc::new(mask),
        blur: None,
        clip: None,
        invert: false,
        paint,
    }))
}

fn solid(c: crate::scene::Color) -> PaintPx {
    PaintPx::Solid(c.premultiplied())
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
        paint: PaintPx::Solid(color),
    }))
}

/// Text above a node, one fill per run colour.
fn text_ops(text: Option<&TextSpec>, t: &Affine, size: (u32, u32)) -> Vec<Op> {
    let Some(text) = text else {
        return Vec::new();
    };
    crate::text::text_components(text, t)
        .into_iter()
        .filter_map(|(color, comps)| fill_op(&comps, solid(color), size))
        .collect()
}

/// Shadow, fill, stroke of a vector silhouette.
fn vector_ops(
    sil: &Silhouette,
    fill: Option<&Paint>,
    stroke: Option<&StrokeSpec>,
    shadow: Option<&DropShadowStyle>,
    size: (u32, u32),
) -> Vec<Op> {
    if sil.is_empty() {
        return Vec::new();
    }
    let mut ops = Vec::new();
    ops.extend(shadow.and_then(|s| shadow_op(sil, s, size)));
    if let Some(p) = fill.and_then(paint_px) {
        ops.extend(fill_op(std::slice::from_ref(&sil.fill), p, size));
    }
    if let Some(s) = stroke {
        ops.extend(fill_op(
            std::slice::from_ref(&sil.stroke),
            solid(s.color),
            size,
        ));
    }
    ops
}

/// A whole-pixel translation, if `t` is one.
fn integer_translation(t: &Affine) -> Option<[i32; 2]> {
    let [a, b, c, d, e, f] = t.m;
    let whole = |v: f64| v.is_finite() && v.fract() == 0.0 && v.abs() < 1e7;
    (a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0 && whole(e) && whole(f))
        .then_some([e as i32, f as i32])
}

fn image_op(r: &RasterSpec, t: Option<&Affine>, size: (u32, u32)) -> Option<Op> {
    let opacity = unit(r.opacity);
    let tb = r.surface.tile_bounds();
    if opacity <= 0.0 || tb.is_empty() {
        return None;
    }
    let (surface, origin) = match t.map(|t| (t, integer_translation(t))) {
        None => (r.surface.clone(), r.origin),
        Some((_, Some(d))) => (
            r.surface.clone(),
            [
                r.origin[0].saturating_add(d[0]),
                r.origin[1].saturating_add(d[1]),
            ],
        ),
        // Scaled or rotated: resample once into canvas space (bicubic,
        // premultiplied, clipped to the canvas).
        Some((t, None)) => {
            let place = Affine::translate(f64::from(r.origin[0]), f64::from(r.origin[1])).then(t);
            let [a, b, c, d, e, f] = place.m;
            if !place.m.iter().all(|v| v.is_finite()) || place.inverse().is_none() {
                return None;
            }
            let h = Homography([a, c, e, b, d, f, 0.0, 0.0, 1.0]);
            let warped =
                warp_surface_clipped(&r.surface, tb, &h, Interp::Bicubic, canvas_rect(size))
                    .ok()?;
            (Arc::new(warped), [0, 0])
        }
    };
    let tb = surface.tile_bounds();
    if tb.is_empty() {
        return None;
    }
    let region = tb.translate(origin[0], origin[1]);
    if region.intersect(&canvas_rect(size)).is_empty() {
        return None;
    }
    Some(Op::Image(ImageOp {
        surface,
        origin,
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

/// The magnifier: the lens outline (and so its mask, border and rim) is
/// turned by `t`; the content is sampled axis-aligned around the turned
/// lens centre, and the ring around the source turns with the lens.
fn magnifier_ops(m: &MagnifierSpec, t: &Affine, size: (u32, u32)) -> Vec<Op> {
    if !primitives::point_ok(m.source_center) {
        return Vec::new();
    }
    let lens = primitives::transform_lines(lens_loops(&m.lens_bounds), t);
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
    let c0 = m.lens_bounds.center();
    let center = t.apply(Point::new(f64::from(c0.x), f64::from(c0.y)));
    if !(primitives::point_ok(crate::scene::PointF32::new(
        center.x as f32,
        center.y as f32,
    ))) {
        return Vec::new();
    }
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
        let src = m.source_center.f64();
        let mut lines = Vec::new();
        if let Some(rim) = first_crossing(&lens, src, (center.x, center.y)) {
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
        // The linear part of `t`, about the source.
        let [a, b, cc, d, _, _] = t.m;
        let about_src = Affine {
            m: [
                a,
                b,
                cc,
                d,
                src.0 - (a * src.0 + cc * src.1),
                src.1 - (b * src.0 + d * src.1),
            ],
        };
        lines.extend(primitives::transform_lines(ring, &about_src));
        let polys = primitives::stroke(&lines, c, LineCap::Round);
        ops.extend(fill_op(&[polys], solid(c.color), size));
    }
    let Some(lens_mask) =
        Mask::rasterize(std::slice::from_ref(&lens_fill), canvas, 0).map(Arc::new)
    else {
        return ops;
    };
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
        center: [center.x as f32, center.y as f32],
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
                    paint: PaintPx::Solid(color),
                }));
            }
        }
    }
    if let Some(s) = &m.border_stroke {
        ops.extend(fill_op(&[border], solid(s.color), size));
    }
    ops
}

/// Coverage of `components` with a soft edge of `feather` pixels (hard
/// exact-area coverage below half a pixel), clipped to the canvas plus the
/// feather's reach.
fn feathered(components: &[Vec<Vec<P>>], feather: f32, size: (u32, u32)) -> Option<Mask> {
    let f = if feather.is_finite() {
        feather.clamp(0.0, 2.0 * MAX_SIGMA)
    } else {
        0.0
    };
    if f < 0.5 {
        return Mask::rasterize(components, canvas_rect(size), 0);
    }
    let pad = (f * 0.5).ceil() as u32 + 1;
    let hard = Mask::rasterize(components, canvas_rect(size).inflate(pad as i32), pad)?;
    let soft = feather_mask(&hard, f);
    soft.data.iter().any(|v| *v > 0.0).then_some(soft)
}

fn outline_components(g: &crate::scene::ShapeGeometry, t: &Affine) -> Vec<Vec<P>> {
    primitives::transform_lines(primitives::geometry_outline(g), t)
        .into_iter()
        .map(|l| l.pts)
        .collect()
}

fn backdrop_ops(b: &BackdropFilterSpec, t: &Affine, size: (u32, u32)) -> Vec<Op> {
    let polys = outline_components(&b.shape, t);
    // The pixelate grid starts at the hard shape's top-left, whatever the
    // feather.
    let Some(hard_rect) =
        Mask::rasterize(std::slice::from_ref(&polys), canvas_rect(size), 0).map(|m| m.rect)
    else {
        return Vec::new();
    };
    let Some(mask) = feathered(&[polys], b.feather_px, size).map(Arc::new) else {
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
            let origin = [hard_rect.x0, hard_rect.y0];
            vec![Op::Pixelate(PixelateOp {
                mask,
                block,
                origin,
            })]
        }
    }
}

fn spotlight_ops(s: &SpotlightSpec, t: &Affine, size: (u32, u32)) -> Vec<Op> {
    let dim_a = unit(s.dim_opacity) * s.dim_color.premultiplied()[3];
    let blur = Kernel::gaussian(s.blur_sigma).map(Arc::new);
    if dim_a <= 0.0 && blur.is_none() {
        return Vec::new();
    }
    let comps: Vec<Vec<Vec<P>>> = s.targets.iter().map(|g| outline_components(g, t)).collect();
    let mask = feathered(&comps, s.feather_px, size).map(Arc::new);
    let c = s.dim_color;
    vec![Op::Spotlight(SpotlightOp {
        mask,
        blur,
        dim_rgb: [unit(c.r), unit(c.g), unit(c.b)],
        dim_a,
    })]
}

/// The shadow frame around a canvas of `size`.
fn frame_ops(f: &ShadowFrameSpec, size: (u32, u32)) -> Vec<Op> {
    let m = f.margin.min(crate::canvas::MAX_EDGE);
    let dst = (
        size.0.saturating_add(m.saturating_mul(2)),
        size.1.saturating_add(m.saturating_mul(2)),
    );
    if dst.0 > crate::canvas::MAX_EDGE
        || dst.1 > crate::canvas::MAX_EDGE
        || u64::from(dst.0) * u64::from(dst.1) > MAX_MASK_PX
    {
        return Vec::new();
    }
    let out = canvas_rect(dst);
    let radius = if f.corner_radius.is_finite() {
        f64::from(f.corner_radius.max(0.0))
    } else {
        0.0
    };
    let (x, y, w, h) = (
        f64::from(m),
        f64::from(m),
        f64::from(size.0),
        f64::from(size.1),
    );
    let outline = flatten_path(&shapes::rounded_rect(x, y, w, h, [radius; 4]), TOLERANCE);
    let fill: Vec<Vec<P>> = outline.iter().map(|l| l.pts.clone()).collect();
    let Some(clip) = Mask::rasterize(std::slice::from_ref(&fill), out, 0) else {
        return Vec::new();
    };
    let backdrop = paint_px(&f.backdrop).unwrap_or(PaintPx::Solid([0.0; 4]));
    // The shadow is blurred once, here, with photocraft-algo's Gaussian;
    // both compositors read the result.
    let shadow = f.shadow.as_ref().and_then(|s| {
        let color = s.color.premultiplied();
        if color[3] <= 0.0 {
            return None;
        }
        let sil = Silhouette {
            fill: fill.clone(),
            stroke: Vec::new(),
            outline: outline.clone(),
        };
        let sigma = if s.blur_sigma.is_finite() {
            s.blur_sigma.clamp(0.0, MAX_SIGMA)
        } else {
            0.0
        };
        let pad = (3.0 * sigma).ceil() as u32 + 1;
        let hard = Mask::rasterize(&sil.shadow_components(s), out.inflate(pad as i32), pad)?;
        let (mw, mh) = (hard.rect.width() as usize, hard.rect.height() as usize);
        let data = photocraft_algo::matting::gaussian_blur(&hard.data, mw, mh, sigma);
        (data.len() == mw * mh).then(|| {
            (
                Arc::new(Mask {
                    rect: hard.rect,
                    data,
                }),
                color,
            )
        })
    });
    let border = f.border.as_ref().and_then(|s| {
        let polys = primitives::stroke(&outline, s, LineCap::Butt);
        let mask = Mask::rasterize(&[polys], out, 0)?;
        let c = s.color.premultiplied();
        (c[3] > 0.0).then(|| (Arc::new(mask), c))
    });
    vec![Op::Frame(FrameOp {
        margin: m as i32,
        src: size,
        dst,
        backdrop,
        shadow,
        clip: Arc::new(clip),
        border,
    })]
}

/// Prepares one node for a canvas of `size` (the size at its z position),
/// drawn through `transform` (node space → canvas space).
pub fn prepare(node: &RenderNode, transform: Option<&Affine>, size: (u32, u32)) -> Prepared {
    let t = transform.copied().unwrap_or(Affine::IDENTITY);
    let ops = match node {
        RenderNode::Raster(r) => image_op(r, transform, size).into_iter().collect(),
        RenderNode::Shape(s) => {
            let mut ops = vector_ops(
                &primitives::shape_silhouette(s).transformed(&t),
                s.fill.as_ref(),
                s.stroke.as_ref(),
                s.shadow.as_ref(),
                size,
            );
            ops.extend(text_ops(s.text.as_ref(), &t, size));
            ops
        }
        RenderNode::Callout(c) => {
            let mut ops = vector_ops(
                &primitives::callout::silhouette(c).transformed(&t),
                c.fill.as_ref(),
                c.stroke.as_ref(),
                c.shadow.as_ref(),
                size,
            );
            ops.extend(text_ops(c.text.as_ref(), &t, size));
            ops
        }
        RenderNode::Arrow(a) => {
            let sil = primitives::arrow_step::silhouette(a).transformed(&t);
            let mut ops = Vec::new();
            if !sil.is_empty() {
                ops.extend(a.shadow.as_ref().and_then(|s| shadow_op(&sil, s, size)));
                // Shaft and heads in one mask: overlaps are covered once.
                ops.extend(fill_op(
                    &[sil.fill.clone(), sil.stroke.clone()],
                    solid(a.color),
                    size,
                ));
            }
            ops
        }
        RenderNode::Magnifier(m) => magnifier_ops(m, &t, size),
        RenderNode::BackdropFilter(b) => backdrop_ops(b, &t, size),
        RenderNode::Spotlight(s) => spotlight_ops(s, &t, size),
        RenderNode::CutOut(c) => CutParams::new(c, size.0, size.1)
            .map(Op::Cut)
            .into_iter()
            .collect(),
        RenderNode::ShadowFrame(f) => frame_ops(f, size),
    };
    Prepared { ops }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use crate::scene::{Color, GradientStop, PointF32};

    fn frame_spec() -> ShadowFrameSpec {
        ShadowFrameSpec {
            margin: 20,
            corner_radius: 8.0,
            backdrop: Paint::Solid(Color::WHITE),
            shadow: Some(DropShadowStyle::default()),
            border: Some(StrokeSpec::solid(Color::BLACK, 2.0)),
        }
    }

    #[test]
    fn the_frame_grows_the_canvas_and_maps_damage_by_its_margin() {
        let p = prepare(&RenderNode::ShadowFrame(frame_spec()), None, (300, 200));
        assert_eq!(p.out_size((300, 200)), (340, 240));
        let [op] = p.ops.as_slice() else {
            panic!("one op expected, got {}", p.ops.len())
        };
        assert!(op.resizes() && op.reads_neighbours());
        assert_eq!(op.write_rect((300, 200)), Rect::new(0, 0, 340, 240));
        // An input change moves by the margin.
        assert_eq!(
            op.forward(Rect::new(10, 10, 30, 40), (300, 200)),
            vec![Rect::new(30, 30, 50, 60)]
        );
        // An output region reads the input under it, inside the input.
        assert_eq!(
            op.read_rects(Rect::new(0, 0, 50, 60), (300, 200)),
            vec![Rect::new(0, 0, 30, 40)]
        );
        assert!(op.read_rects(Rect::new(0, 0, 10, 10), (300, 200))[0].is_empty());
    }

    #[test]
    fn the_frame_composes_backdrop_shadow_clip_and_border() {
        let p = prepare(&RenderNode::ShadowFrame(frame_spec()), None, (300, 200));
        let Some(Op::Frame(f)) = p.ops.first() else {
            panic!("no frame op")
        };
        let red = |_: i32, _: i32| [1.0, 0.0, 0.0, 1.0];
        // The margin shows the backdrop.
        assert_eq!(f.compose(&red, 2, 2), [1.0, 1.0, 1.0, 1.0]);
        // Inside: the canvas.
        assert_eq!(f.compose(&red, 170, 120), [1.0, 0.0, 0.0, 1.0]);
        // The rounded corner leaves the backdrop (darkened by the shadow).
        let corner = f.compose(&red, 20, 20);
        assert!(corner[0] < 1.0 && corner[1] > 0.0, "{corner:?}");
        // Below the canvas the shadow darkens the backdrop.
        let below = f.compose(&red, 170, 224);
        assert!(below[1] < 1.0 && below[1] > 0.3, "{below:?}");
    }

    #[test]
    fn gradients_interpolate_straight_stops() {
        let stops = vec![
            GradientStop {
                offset: 1.0,
                color: Color::new(0.0, 0.0, 1.0, 1.0),
            },
            GradientStop {
                offset: 0.0,
                color: Color::new(1.0, 0.0, 0.0, 0.5),
            },
        ];
        let Some(PaintPx::Gradient(g)) = paint_px(&Paint::Linear {
            from: PointF32::new(0.0, 0.0),
            to: PointF32::new(99.0, 0.0),
            stops: stops.clone(),
        }) else {
            panic!("not a gradient")
        };
        // Sorted, clamped at both ends, premultiplied after mixing.
        assert_eq!(g.at(-50, 0), [0.5, 0.0, 0.0, 0.5]);
        assert_eq!(g.at(500, 0), [0.0, 0.0, 1.0, 1.0]);
        let mid = g.at(49, 7);
        assert!(
            (mid[3] - 0.75).abs() < 1e-6 && (mid[0] - 0.375).abs() < 1e-6,
            "{mid:?}"
        );
        let Some(PaintPx::Gradient(r)) = paint_px(&Paint::Radial {
            center: PointF32::new(49.5, 49.5),
            radius: 10.0,
            stops,
        }) else {
            panic!("not a gradient")
        };
        assert_eq!(r.t(49, 49), 0.0);
        assert_eq!(r.t(80, 50), 1.0);
        // Degenerate gradients paint their last stop.
        assert!(matches!(
            paint_px(&Paint::Radial {
                center: PointF32::new(0.0, 0.0),
                radius: 0.0,
                stops: vec![GradientStop {
                    offset: 0.0,
                    color: Color::WHITE
                }],
            }),
            Some(PaintPx::Solid(_))
        ));
    }

    #[test]
    fn feathering_softens_around_the_outline() {
        let g =
            crate::scene::ShapeGeometry::Rect(crate::scene::RectF32::new(40.0, 40.0, 60.0, 60.0));
        let comps = vec![outline_components(&g, &Affine::IDENTITY)];
        let hard = feathered(&comps, 0.0, (200, 200)).unwrap_or_else(|| panic!("empty"));
        let soft = feathered(&comps, 10.0, (200, 200)).unwrap_or_else(|| panic!("empty"));
        assert_eq!(hard.at(39, 70), 0.0);
        assert!(soft.rect.x0 < hard.rect.x0);
        // Half coverage on the outline, ramping over the feather width.
        let edge = soft.at(40, 70);
        assert!(edge > 0.35 && edge < 0.75, "{edge}");
        assert!(soft.at(34, 70) < 0.05 && soft.at(46, 70) > 0.95);
        assert!(soft.at(37, 70) > 0.0 && soft.at(37, 70) < edge);
    }

    #[test]
    fn rotation_turns_shapes_and_integer_moves_keep_rasters_unresampled() {
        let s = crate::scene::ShapeSpec {
            geometry: crate::scene::ShapeGeometry::Rect(crate::scene::RectF32::new(
                0.0, 40.0, 100.0, 20.0,
            )),
            fill: Some(Color::BLACK.into()),
            stroke: None,
            shadow: None,
            text: None,
        };
        let turn = Affine::translate(-50.0, -50.0)
            .then(&Affine::rotate(std::f64::consts::FRAC_PI_2))
            .then(&Affine::translate(50.0, 50.0));
        let p = prepare(&RenderNode::Shape(s), Some(&turn), (100, 100));
        let Some(Op::Fill(f)) = p.ops.first() else {
            panic!("no fill")
        };
        // The 100 × 20 bar now stands upright through the centre.
        assert!(
            f.mask.rect.x0 >= 39 && f.mask.rect.x1 <= 62,
            "{:?}",
            f.mask.rect
        );
        assert!(f.mask.at(50, 5) > 0.99 && f.mask.at(50, 95) > 0.99);
        assert_eq!(f.mask.at(30, 50), 0.0);

        let mut surface = Surface::new(photocraft_color::PixelFormat::RGBA8);
        surface.fill_rect(Rect::new(0, 0, 10, 10), &[1.0, 0.0, 0.0, 1.0]);
        let r = RasterSpec {
            surface: Arc::new(surface),
            origin: [5, 5],
            opacity: 1.0,
        };
        let moved = prepare(
            &RenderNode::Raster(r.clone()),
            Some(&Affine::translate(3.0, 4.0)),
            (300, 300),
        );
        let Some(Op::Image(i)) = moved.ops.first() else {
            panic!("no image")
        };
        assert!(Arc::ptr_eq(&i.surface, &r.surface) && i.origin == [8, 9]);
        let scaled = prepare(
            &RenderNode::Raster(r),
            Some(&Affine::scale(2.0)),
            (300, 300),
        );
        let Some(Op::Image(i)) = scaled.ops.first() else {
            panic!("no image")
        };
        let mut px = [[0.0f32; 4]; 1];
        i.surface.read_rgba_into(Rect::new(20, 20, 21, 21), &mut px);
        assert!(px[0][0] > 0.9 && px[0][3] > 0.9, "{px:?}");
        i.surface.read_rgba_into(Rect::new(40, 40, 41, 41), &mut px);
        assert!(px[0][3] < 0.1, "{px:?}");
    }
}
