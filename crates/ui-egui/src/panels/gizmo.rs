//! The transform gizmo of the selected annotation, the yellow parameter
//! diamonds (callout neck, magnifier source) and the margin and corner
//! handles of the rounded shadow frame: pure geometry, unit-tested without
//! a display.
//!
//! An annotation's geometry is stored unrotated; the document turns it by
//! `rotation_rad` about [`Annotation::pivot`]. The gizmo works in that
//! *local* frame: the pointer is mapped back through the inverse rotation,
//! the eight resize handles sit on the local bounds, and a resize keeps
//! the handle opposite the dragged one fixed on screen (the pivot moves
//! with the bounds, so the new bounds are shifted to compensate before
//! they are sent as `node.resize`).

use capturecraft_format::{
    Annotation, AnnotationNode, CalloutBody, CanvasBackdrop, MAX_BACKDROP_MARGIN_PX, MagnifierLens,
    NodeId, RectF, RegionShape, normalize_angle, rotation_about,
};
use egui::CursorIcon;
use photocraft_geom::{Affine, Point};
use serde_json::json;

use crate::UiIntent;
use crate::state::ResizeHandle;

/// Length of the rotation stem above the top edge, points.
pub const STEM_PT: f32 = 24.0;
/// Shift-drag rotation step, degrees.
pub const ROTATE_SNAP_DEG: f32 = 15.0;
/// The corner-radius diamond stays at least this far inside the corner,
/// points, so it never hides under the corner handle.
pub const RADIUS_INSET_PT: f32 = 12.0;

/// A grabbable part of the gizmo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoHandle {
    Resize(ResizeHandle),
    Rotate,
    /// Yellow diamond setting a rounded body's corner radius.
    CornerRadius,
}

pub const RESIZE_HANDLES: [ResizeHandle; 8] = [
    ResizeHandle::NW,
    ResizeHandle::N,
    ResizeHandle::NE,
    ResizeHandle::E,
    ResizeHandle::SE,
    ResizeHandle::S,
    ResizeHandle::SW,
    ResizeHandle::W,
];

/// A node's local frame: unrotated bounds, the pivot and the rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeFrame {
    pub bounds: RectF,
    pub pivot: Point,
    pub rotation: f32,
}

impl NodeFrame {
    /// `None` for cut outs and unknown nodes.
    pub fn of(node: &AnnotationNode) -> Option<Self> {
        let a = node.annotation()?;
        Some(Self {
            bounds: a.bounds()?,
            pivot: a.pivot()?,
            rotation: finite(node.rotation_rad),
        })
    }

    fn affine(&self) -> Affine {
        rotation_about(self.pivot, self.rotation)
    }

    /// Local → canvas.
    pub fn to_canvas(&self, p: Point) -> Point {
        if self.rotation == 0.0 {
            return p;
        }
        self.affine().apply(p)
    }

    /// Canvas → local.
    pub fn to_local(&self, p: Point) -> Point {
        if self.rotation == 0.0 {
            return p;
        }
        self.affine().inverse().map_or(p, |i| i.apply(p))
    }

    /// The bounds' corners in canvas space: top-left, top-right,
    /// bottom-right, bottom-left.
    pub fn corners(&self) -> [Point; 4] {
        let b = self.bounds;
        [
            Point::new(b.x, b.y),
            Point::new(b.x + b.w, b.y),
            Point::new(b.x + b.w, b.y + b.h),
            Point::new(b.x, b.y + b.h),
        ]
        .map(|p| self.to_canvas(p))
    }
}

fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// Where `h` sits on `b` (local frame).
pub fn handle_point(b: RectF, h: ResizeHandle) -> Point {
    let (l, r, t, bo) = h.edges();
    let x = if l {
        b.x
    } else if r {
        b.x + b.w
    } else {
        b.x + b.w / 2.0
    };
    let y = if t {
        b.y
    } else if bo {
        b.y + b.h
    } else {
        b.y + b.h / 2.0
    };
    Point::new(x, y)
}

pub fn opposite(h: ResizeHandle) -> ResizeHandle {
    use ResizeHandle::*;
    match h {
        N => S,
        NE => SW,
        E => W,
        SE => NW,
        S => N,
        SW => NE,
        W => E,
        NW => SE,
    }
}

/// The rotation handle, `stem` canvas pixels above the top edge's middle.
pub fn rotate_handle_point(b: RectF, stem: f64) -> Point {
    Point::new(b.x + b.w / 2.0, b.y - stem)
}

/// The rounded body a corner-radius diamond edits and its radius:
/// rectangular and rounded callout bodies and rectangle shapes.
pub fn corner_radius_of(a: &Annotation) -> Option<(RectF, f32)> {
    match a {
        Annotation::Callout(c) => match c.body {
            CalloutBody::RoundedRect { radius } => Some((c.bounds, radius)),
            CalloutBody::Rect => Some((c.bounds, 0.0)),
            _ => None,
        },
        Annotation::Shape(s) => match s.shape {
            RegionShape::RoundedRect { rect, radius } => Some((rect, radius)),
            RegionShape::Rect { rect } => Some((rect, 0.0)),
            _ => None,
        },
        _ => None,
    }
}

/// Largest radius a `rect` takes.
fn max_radius(rect: RectF) -> f32 {
    (rect.w.abs().min(rect.h.abs()) / 2.0) as f32
}

/// The diamond's local position: on the top-left corner's diagonal at
/// `radius`, at least `inset` canvas pixels in.
pub fn corner_radius_point(rect: RectF, radius: f32, inset: f64) -> Point {
    let d = f64::from(radius.clamp(0.0, max_radius(rect)))
        .max(inset)
        .min(rect.w.abs().min(rect.h.abs()) / 2.0);
    Point::new(rect.x + d, rect.y + d)
}

/// The radius a diamond dragged to `local` sets (its distance along the
/// diagonal), clamped to the body.
pub fn radius_from_drag(rect: RectF, local: Point) -> f32 {
    let d = ((local.x - rect.x) + (local.y - rect.y)) / 2.0;
    (d.max(0.0) as f32).min(max_radius(rect)).round()
}

/// The handle at canvas point `p`, if any. `scale` is points per canvas
/// pixel; distances are compared in the local frame (rotation keeps them).
pub fn hit_handle(
    frame: &NodeFrame,
    a: &Annotation,
    p: Point,
    scale: f32,
    handle_pt: f32,
) -> Option<GizmoHandle> {
    let s = f64::from(scale.max(f32::EPSILON));
    let local = frame.to_local(p);
    let tol = (f64::from(handle_pt) / 2.0 + 3.0) / s;
    let dist = |q: Point| (q.x - local.x).hypot(q.y - local.y);
    let mut best: Option<(f64, GizmoHandle)> = None;
    let mut consider = |q: Point, h: GizmoHandle| {
        let d = dist(q);
        if d <= tol && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, h));
        }
    };
    consider(
        rotate_handle_point(frame.bounds, f64::from(STEM_PT) / s),
        GizmoHandle::Rotate,
    );
    if let Some((rect, r)) = corner_radius_of(a) {
        consider(
            corner_radius_point(rect, r, f64::from(RADIUS_INSET_PT) / s),
            GizmoHandle::CornerRadius,
        );
    }
    for h in RESIZE_HANDLES {
        consider(handle_point(frame.bounds, h), GizmoHandle::Resize(h));
    }
    best.map(|(_, h)| h)
}

/// Bounds a resize drag of `h` to the local pointer `local` gives. The
/// opposite handle stays put (the centre with `from_center`); with
/// `keep_aspect` the start's aspect ratio is kept (an edge handle grows
/// the other side symmetrically). Dragging past the opposite edge flips.
pub fn resized_bounds(
    start: RectF,
    h: ResizeHandle,
    local: Point,
    keep_aspect: bool,
    from_center: bool,
) -> RectF {
    let (l, r, t, b) = h.edges();
    let c = start.center();
    let (mut x0, mut y0) = (start.x, start.y);
    let (mut x1, mut y1) = (start.x + start.w, start.y + start.h);
    if l {
        x0 = local.x;
    }
    if r {
        x1 = local.x;
    }
    if t {
        y0 = local.y;
    }
    if b {
        y1 = local.y;
    }
    if from_center {
        if l {
            x1 = 2.0 * c.x - x0;
        }
        if r {
            x0 = 2.0 * c.x - x1;
        }
        if t {
            y1 = 2.0 * c.y - y0;
        }
        if b {
            y0 = 2.0 * c.y - y1;
        }
    }
    let horizontal = l || r;
    let vertical = t || b;
    if keep_aspect && start.w > 0.0 && start.h > 0.0 {
        let sx = (x1 - x0) / start.w;
        let sy = (y1 - y0) / start.h;
        let k = match (horizontal, vertical) {
            (true, true) => sx.abs().max(sy.abs()),
            (true, false) => sx.abs(),
            _ => sy.abs(),
        };
        let (w, hgt) = (start.w * k * sx.signum(), start.h * k * sy.signum());
        // Re-place along each axis about its fixed side (or the centre).
        let place = |moved_lo: bool, moved_hi: bool, lo: f64, hi: f64, len: f64, mid: f64| {
            if from_center || !(moved_lo || moved_hi) {
                (mid - len / 2.0, mid + len / 2.0)
            } else if moved_lo {
                (hi - len, hi)
            } else {
                (lo, lo + len)
            }
        };
        (x0, x1) = place(l, r, start.x, start.x + start.w, w, c.x);
        (y0, y1) = place(t, b, start.y, start.y + start.h, hgt, c.y);
    }
    RectF::from_corners(Point::new(x0, y0), Point::new(x1, y1))
}

/// `new` shifted so the anchor of the drag (the handle opposite `h`, or
/// the centre with `from_center`) stays on the same canvas point although
/// the pivot moves with the resize. Unrotated nodes need no shift.
pub fn anchored_bounds(
    a: &Annotation,
    rotation: f32,
    start: RectF,
    new: RectF,
    h: ResizeHandle,
    from_center: bool,
) -> RectF {
    let rotation = finite(rotation);
    let Some(old_pivot) = a.pivot().filter(|_| rotation != 0.0) else {
        return new;
    };
    // `resized_bounds` keeps the anchor's local coordinates.
    let anchor = if from_center {
        start.center()
    } else {
        handle_point(start, opposite(h))
    };
    let world = rotation_about(old_pivot, rotation).apply(anchor);
    let mut b = a.clone();
    b.resize(start, new);
    let Some(new_pivot) = b.pivot() else {
        return new;
    };
    let moved = rotation_about(new_pivot, rotation).apply(anchor);
    let mut out = new;
    out.translate(world.x - moved.x, world.y - moved.y);
    out
}

/// What a resize drag shows and commits: the bounds for `node.resize` and
/// the frame to draw the preview in.
pub fn resize_preview(
    a: &Annotation,
    rotation: f32,
    h: ResizeHandle,
    local: Point,
    keep_aspect: bool,
    from_center: bool,
) -> Option<(RectF, NodeFrame)> {
    let start = a.bounds()?;
    let new = resized_bounds(start, h, local, keep_aspect, from_center);
    let sent = anchored_bounds(a, rotation, start, new, h, from_center);
    let mut b = a.clone();
    b.resize(start, sent);
    Some((
        sent,
        NodeFrame {
            bounds: sent,
            pivot: b.pivot()?,
            rotation: finite(rotation),
        },
    ))
}

/// Rotation after dragging the rotation handle from `from` to `to` (canvas
/// space) about `pivot`; `snap` rounds to [`ROTATE_SNAP_DEG`].
pub fn rotation_from_drag(pivot: Point, from: Point, to: Point, start: f32, snap: bool) -> f32 {
    let angle = |p: Point| (p.y - pivot.y).atan2(p.x - pivot.x);
    let mut r = finite(start) + (angle(to) - angle(from)) as f32;
    if snap {
        let step = ROTATE_SNAP_DEG.to_radians();
        r = (r / step).round() * step;
    }
    normalize_angle(r)
}

fn rect_json(b: RectF) -> serde_json::Value {
    json!({ "x": b.x, "y": b.y, "w": b.w, "h": b.h })
}

/// `node.resize` with new unrotated bounds.
pub fn resize_intent(id: NodeId, bounds: RectF) -> UiIntent {
    UiIntent::command(
        "node.resize",
        json!({ "id": id.0, "bounds": rect_json(bounds) }),
    )
}

/// `node.rotate` to an absolute angle.
pub fn rotate_intent(id: NodeId, rotation_rad: f32) -> UiIntent {
    UiIntent::command(
        "node.rotate",
        json!({ "id": id.0, "rotation_rad": normalize_angle(rotation_rad) }),
    )
}

/// The corner radius of one node, through `annotation.setStyle`.
pub fn radius_intent(id: NodeId, radius: f32) -> UiIntent {
    UiIntent::command(
        "annotation.setStyle",
        json!({ "ids": [id.0], "corner_radius": radius }),
    )
}

// ---------------------------------------------------------------------------
// Parameter diamonds

/// Where the ray from `body`'s centre towards `tip` leaves the body, and
/// the ray's unit direction; `None` when the tip is at the centre.
fn tail_axis(body: RectF, tip: Point) -> Option<(Point, (f64, f64))> {
    let c = body.center();
    let (dx, dy) = (tip.x - c.x, tip.y - c.y);
    let len = dx.hypot(dy);
    if !len.is_finite() || len < 1e-6 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    // Distance along the ray to the nearer of the box's edges.
    let reach = |half: f64, u: f64| {
        if u.abs() < 1e-12 {
            f64::INFINITY
        } else {
            half.abs() / u.abs()
        }
    };
    let t = reach(body.w / 2.0, ux)
        .min(reach(body.h / 2.0, uy))
        .min(len);
    Some((Point::new(c.x + ux * t, c.y + uy * t), (ux, uy)))
}

/// The neck diamond of a callout tail: where the tail meets the body,
/// half the base width to the side of the tail's axis.
pub fn tail_neck_point(body: RectF, tip: Point, base_width: f32) -> Option<Point> {
    let (e, (ux, uy)) = tail_axis(body, tip)?;
    let half = f64::from(finite(base_width).max(0.0)) / 2.0;
    Some(Point::new(e.x - uy * half, e.y + ux * half))
}

/// The base width a neck diamond dragged to `p` sets: twice its distance
/// from the tail's axis, whole pixels, at least 2.
pub fn neck_width_from_drag(body: RectF, tip: Point, p: Point) -> Option<f32> {
    let (e, (ux, uy)) = tail_axis(body, tip)?;
    let side = ((p.x - e.x) * -uy + (p.y - e.y) * ux).abs();
    let limit = 2.0 * body.w.abs().max(body.h.abs()).max(1.0);
    Some((2.0 * side).clamp(2.0, limit).round() as f32)
}

/// `annotation.moveCalloutTail` changing only the neck.
pub fn tail_neck_intent(id: NodeId, index: usize, tip: Point, base_width: f32) -> UiIntent {
    UiIntent::command(
        "annotation.moveCalloutTail",
        json!({ "id": id.0, "index": index, "tip": { "x": tip.x, "y": tip.y },
                "base_width": base_width }),
    )
}

/// Radius of the area a magnifier enlarges: the lens over the zoom.
pub fn magnifier_source_radius(m: &MagnifierLens) -> f32 {
    let b = m.lens.bounds();
    let r = (b.w.abs().min(b.h.abs()) / 2.0) as f32;
    let zoom = if m.zoom.is_finite() && m.zoom > 0.0 {
        m.zoom
    } else {
        1.0
    };
    finite(r / zoom)
}

/// `annotation.setMagnifierSource`.
pub fn magnifier_source_intent(id: NodeId, source_center: Point) -> UiIntent {
    UiIntent::command(
        "annotation.setMagnifierSource",
        json!({ "id": id.0, "source_center": { "x": source_center.x, "y": source_center.y } }),
    )
}

/// Resize cursor for a handle of a node turned by `rotation`: the handle's
/// direction from the centre, bucketed to the four resize cursors.
pub fn handle_cursor(h: GizmoHandle, rotation: f32) -> CursorIcon {
    use ResizeHandle::*;
    let base: f32 = match h {
        GizmoHandle::Resize(E | W) => 0.0,
        GizmoHandle::Resize(SE | NW) => 45.0,
        GizmoHandle::Resize(S | N) => 90.0,
        GizmoHandle::Resize(SW | NE) => 135.0,
        GizmoHandle::Rotate => return CursorIcon::Grab,
        GizmoHandle::CornerRadius => return CursorIcon::PointingHand,
    };
    let deg = base + finite(rotation).to_degrees();
    match ((deg / 45.0).round() as i64).rem_euclid(4) {
        0 => CursorIcon::ResizeHorizontal,
        1 => CursorIcon::ResizeNwSe,
        2 => CursorIcon::ResizeVertical,
        _ => CursorIcon::ResizeNeSw,
    }
}

// ---------------------------------------------------------------------------
// Rounded shadow frame

/// The frame margin, `None` without a frame.
pub fn frame_margin(b: &CanvasBackdrop) -> Option<f32> {
    match b {
        CanvasBackdrop::None => None,
        CanvasBackdrop::RoundedShadowFrame { margin_px, .. } => Some(*margin_px),
    }
}

/// The four margin handles: one per side.
pub const MARGIN_HANDLES: [ResizeHandle; 4] = [
    ResizeHandle::N,
    ResizeHandle::E,
    ResizeHandle::S,
    ResizeHandle::W,
];

/// Margin after dragging `side`'s handle by `delta` canvas pixels: out
/// grows it. Whole pixels within `0..=MAX_BACKDROP_MARGIN_PX`.
pub fn margin_after_drag(start: f32, side: ResizeHandle, delta: [f32; 2]) -> f32 {
    let (l, r, t, b) = side.edges();
    let d = if l {
        -delta[0]
    } else if r {
        delta[0]
    } else if t {
        -delta[1]
    } else if b {
        delta[1]
    } else {
        0.0
    };
    let m = finite(start) + finite(d);
    m.round().clamp(0.0, MAX_BACKDROP_MARGIN_PX)
}

/// `b` with its margin set (unchanged without a frame).
pub fn with_margin(b: &CanvasBackdrop, margin: f32) -> CanvasBackdrop {
    let mut out = b.clone();
    if let CanvasBackdrop::RoundedShadowFrame { margin_px, .. } = &mut out {
        *margin_px = margin.clamp(0.0, MAX_BACKDROP_MARGIN_PX);
    }
    out
}

/// The frame's corner radius, `None` without a frame.
pub fn frame_corner_radius(b: &CanvasBackdrop) -> Option<f32> {
    match b {
        CanvasBackdrop::None => None,
        CanvasBackdrop::RoundedShadowFrame {
            corner_radius_px, ..
        } => Some(*corner_radius_px),
    }
}

/// The frame's corner radius after dragging its diamond to `local`
/// (canvas pixels): along the top-left diagonal, at most half the shorter
/// side of `canvas`.
pub fn frame_radius_from_drag(canvas: [u32; 2], local: Point) -> f32 {
    radius_from_drag(canvas_rect(canvas), local)
}

/// The canvas as a rectangle at the origin.
pub fn canvas_rect(canvas: [u32; 2]) -> RectF {
    RectF::new(0.0, 0.0, f64::from(canvas[0]), f64::from(canvas[1]))
}

/// `b` with its corner radius set (unchanged without a frame).
pub fn with_corner_radius(b: &CanvasBackdrop, radius: f32) -> CanvasBackdrop {
    let mut out = b.clone();
    if let CanvasBackdrop::RoundedShadowFrame {
        corner_radius_px, ..
    } = &mut out
    {
        *corner_radius_px = finite(radius).max(0.0).round();
    }
    out
}

/// `canvas.setBackdrop`.
pub fn backdrop_intent(b: &CanvasBackdrop) -> UiIntent {
    UiIntent::command(
        "canvas.setBackdrop",
        json!({ "backdrop": serde_json::to_value(b).unwrap_or_default() }),
    )
}

/// Canvas-pixel offset of the rendered image's top-left: a framed render
/// is the canvas plus the margin on every side.
pub fn image_offset(b: &CanvasBackdrop, canvas: [u32; 2], image: [u32; 2]) -> f64 {
    let m = b.margin();
    let framed = [
        canvas[0].saturating_add(2 * m),
        canvas[1].saturating_add(2 * m),
    ];
    if m > 0 && image == framed {
        -f64::from(m)
    } else {
        0.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use capturecraft_format::{Callout, CalloutTail, ShapeNode};
    use std::f32::consts::FRAC_PI_2;

    fn shape(rect: RectF) -> Annotation {
        Annotation::Shape(ShapeNode {
            shape: RegionShape::Rect { rect },
            ..ShapeNode::default()
        })
    }

    fn near(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6
    }

    #[test]
    fn local_and_canvas_frames_round_trip() {
        let mut n = AnnotationNode::new(NodeId(1), shape(RectF::new(0.0, 0.0, 100.0, 50.0)));
        n.rotation_rad = 0.7;
        let f = NodeFrame::of(&n).unwrap();
        let p = Point::new(13.0, -42.0);
        assert!(near(f.to_local(f.to_canvas(p)), p));
        n.rotation_rad = FRAC_PI_2;
        let f = NodeFrame::of(&n).unwrap();
        // Top-right corner turns clockwise to the bottom right of the pivot.
        // (`rotation_rad` is f32, so π/2 is a few ulps off.)
        let c = f.corners()[1];
        assert!(
            (c.x - 75.0).abs() < 1e-4 && (c.y - 75.0).abs() < 1e-4,
            "{c:?}"
        );
    }

    #[test]
    fn hits_handles_in_the_rotated_frame() {
        let a = shape(RectF::new(100.0, 100.0, 200.0, 100.0));
        let mut n = AnnotationNode::new(NodeId(1), a.clone());
        n.rotation_rad = FRAC_PI_2;
        let f = NodeFrame::of(&n).unwrap();
        let se = f.to_canvas(Point::new(300.0, 200.0));
        assert_eq!(
            hit_handle(&f, &a, se, 1.0, 8.0),
            Some(GizmoHandle::Resize(ResizeHandle::SE))
        );
        let rot = f.to_canvas(rotate_handle_point(f.bounds, f64::from(STEM_PT)));
        assert_eq!(hit_handle(&f, &a, rot, 1.0, 8.0), Some(GizmoHandle::Rotate));
        // At 2× zoom the stem is half as long in canvas pixels.
        let rot2 = f.to_canvas(rotate_handle_point(f.bounds, f64::from(STEM_PT) / 2.0));
        assert_eq!(
            hit_handle(&f, &a, rot2, 2.0, 8.0),
            Some(GizmoHandle::Rotate)
        );
        let diamond = f.to_canvas(corner_radius_point(
            RectF::new(100.0, 100.0, 200.0, 100.0),
            0.0,
            f64::from(RADIUS_INSET_PT),
        ));
        assert_eq!(
            hit_handle(&f, &a, diamond, 1.0, 8.0),
            Some(GizmoHandle::CornerRadius)
        );
        assert_eq!(hit_handle(&f, &a, Point::new(200.0, 150.0), 1.0, 8.0), None);
    }

    #[test]
    fn corner_handles_keep_the_opposite_corner() {
        let s = RectF::new(10.0, 10.0, 100.0, 50.0);
        let b = resized_bounds(s, ResizeHandle::SE, Point::new(150.0, 100.0), false, false);
        assert_eq!(b, RectF::new(10.0, 10.0, 140.0, 90.0));
        let b = resized_bounds(s, ResizeHandle::NW, Point::new(0.0, 0.0), false, false);
        assert_eq!(b, RectF::new(0.0, 0.0, 110.0, 60.0));
        // Past the opposite edge: flips, still anchored there.
        let b = resized_bounds(s, ResizeHandle::E, Point::new(0.0, 999.0), false, false);
        assert_eq!(b, RectF::new(0.0, 10.0, 10.0, 50.0));
    }

    #[test]
    fn shift_keeps_aspect_and_alt_resizes_from_the_centre() {
        let s = RectF::new(0.0, 0.0, 100.0, 50.0);
        let b = resized_bounds(s, ResizeHandle::SE, Point::new(200.0, 60.0), true, false);
        assert_eq!(b, RectF::new(0.0, 0.0, 200.0, 100.0));
        let b = resized_bounds(s, ResizeHandle::E, Point::new(200.0, 0.0), true, false);
        assert_eq!(b, RectF::new(0.0, -25.0, 200.0, 100.0));
        let b = resized_bounds(s, ResizeHandle::E, Point::new(150.0, 0.0), false, true);
        assert_eq!(b, RectF::new(-50.0, 0.0, 200.0, 50.0));
        let b = resized_bounds(s, ResizeHandle::NW, Point::new(-50.0, 0.0), true, true);
        assert_eq!(b, RectF::new(-50.0, -25.0, 200.0, 100.0));
    }

    #[test]
    fn rotated_resizes_keep_the_anchor_on_screen() {
        let a = shape(RectF::new(0.0, 0.0, 100.0, 50.0));
        for rot in [0.3f32, -1.2, FRAC_PI_2, 3.0] {
            for h in RESIZE_HANDLES {
                let start = a.bounds().unwrap();
                let anchor = handle_point(start, opposite(h));
                let world = rotation_about(a.pivot().unwrap(), rot).apply(anchor);
                let local = handle_point(start, h);
                let local = Point::new(local.x + 30.0, local.y + 20.0);
                let (sent, frame) = resize_preview(&a, rot, h, local, false, false).unwrap();
                let mut b = a.clone();
                b.resize(start, sent);
                let got = b.bounds().unwrap();
                assert!(near(Point::new(got.x, got.y), Point::new(sent.x, sent.y)));
                assert!(near(Point::new(got.w, got.h), Point::new(sent.w, sent.h)));
                // The anchor sits at the same relative spot of the new bounds.
                let new_anchor = handle_point(sent, opposite(h));
                let after = rotation_about(b.pivot().unwrap(), rot).apply(new_anchor);
                assert!(near(after, world), "{rot} {h:?}: {after:?} vs {world:?}");
                assert!(near(frame.to_canvas(new_anchor), world));
            }
        }
    }

    #[test]
    fn callout_anchoring_uses_the_body_pivot() {
        let a = Annotation::Callout(Callout {
            bounds: RectF::new(0.0, 0.0, 100.0, 60.0),
            tails: vec![CalloutTail {
                tip: Point::new(50.0, 160.0),
                ..CalloutTail::default()
            }],
            ..Callout::default()
        });
        let start = a.bounds().unwrap();
        let world = rotation_about(a.pivot().unwrap(), 0.5).apply(Point::new(0.0, 0.0));
        let (sent, _) = resize_preview(
            &a,
            0.5,
            ResizeHandle::SE,
            Point::new(200.0, 320.0),
            false,
            false,
        )
        .unwrap();
        let mut b = a.clone();
        b.resize(start, sent);
        let after = rotation_about(b.pivot().unwrap(), 0.5).apply(Point::new(sent.x, sent.y));
        assert!(near(after, world));
    }

    #[test]
    fn rotation_drags_snap_and_wrap() {
        let p = Point::new(0.0, 0.0);
        let r = rotation_from_drag(p, Point::new(0.0, -10.0), Point::new(10.0, 0.0), 0.0, false);
        assert!((r - FRAC_PI_2).abs() < 1e-6);
        let r = rotation_from_drag(p, Point::new(0.0, -10.0), Point::new(10.0, -9.0), 0.0, true);
        assert!((r.to_degrees() - 45.0).abs() < 1e-4, "{}", r.to_degrees());
        let r = rotation_from_drag(p, Point::new(0.0, -10.0), Point::new(0.0, 10.0), 3.0, false);
        assert!((-std::f32::consts::PI..=std::f32::consts::PI).contains(&r));
    }

    #[test]
    fn radius_diamond_follows_the_diagonal() {
        let rect = RectF::new(0.0, 0.0, 100.0, 40.0);
        assert_eq!(radius_from_drag(rect, Point::new(10.0, 10.0)), 10.0);
        assert_eq!(radius_from_drag(rect, Point::new(90.0, 90.0)), 20.0);
        assert_eq!(radius_from_drag(rect, Point::new(-5.0, -5.0)), 0.0);
        assert!(near(
            corner_radius_point(rect, 0.0, 12.0),
            Point::new(12.0, 12.0)
        ));
        assert!(near(
            corner_radius_point(rect, 15.0, 12.0),
            Point::new(15.0, 15.0)
        ));
        assert!(corner_radius_of(&shape(rect)).is_some());
    }

    #[test]
    fn cursors_turn_with_the_node() {
        let h = GizmoHandle::Resize(ResizeHandle::E);
        assert_eq!(handle_cursor(h, 0.0), CursorIcon::ResizeHorizontal);
        assert_eq!(handle_cursor(h, FRAC_PI_2), CursorIcon::ResizeVertical);
        assert_eq!(
            handle_cursor(h, std::f32::consts::FRAC_PI_4),
            CursorIcon::ResizeNwSe
        );
        assert_eq!(
            handle_cursor(GizmoHandle::Resize(ResizeHandle::NE), 0.0),
            CursorIcon::ResizeNeSw
        );
    }

    #[test]
    fn intents_carry_the_engine_parameters() {
        let UiIntent::Command { id, params } =
            resize_intent(NodeId(4), RectF::new(1.0, 2.0, 3.0, 4.0))
        else {
            panic!()
        };
        assert_eq!(id, "node.resize");
        assert_eq!(
            params,
            json!({ "id": 4, "bounds": { "x": 1.0, "y": 2.0, "w": 3.0, "h": 4.0 } })
        );
        let UiIntent::Command { id, params } = rotate_intent(NodeId(4), 7.0) else {
            panic!()
        };
        assert_eq!(id, "node.rotate");
        let r = params["rotation_rad"].as_f64().unwrap();
        assert!((r - f64::from(normalize_angle(7.0))).abs() < 1e-6);
        let UiIntent::Command { params, .. } = radius_intent(NodeId(4), 9.0) else {
            panic!()
        };
        assert_eq!(params, json!({ "ids": [4], "corner_radius": 9.0 }));
    }

    #[test]
    fn margin_handles_grow_outward_and_clamp() {
        assert_eq!(margin_after_drag(48.0, ResizeHandle::E, [10.4, 0.0]), 58.0);
        assert_eq!(margin_after_drag(48.0, ResizeHandle::W, [10.0, 0.0]), 38.0);
        assert_eq!(margin_after_drag(48.0, ResizeHandle::N, [0.0, -20.0]), 68.0);
        assert_eq!(margin_after_drag(48.0, ResizeHandle::S, [0.0, -100.0]), 0.0);
        assert_eq!(
            margin_after_drag(48.0, ResizeHandle::S, [0.0, 1e9]),
            MAX_BACKDROP_MARGIN_PX
        );
        let f = CanvasBackdrop::rounded_shadow_frame();
        assert_eq!(frame_margin(&with_margin(&f, 10.0)), Some(10.0));
        assert_eq!(
            frame_margin(&with_margin(&CanvasBackdrop::None, 10.0)),
            None
        );
        let UiIntent::Command { id, params } = backdrop_intent(&with_margin(&f, 10.0)) else {
            panic!()
        };
        assert_eq!(id, "canvas.setBackdrop");
        assert_eq!(params["backdrop"]["kind"], "rounded_shadow_frame");
        assert_eq!(params["backdrop"]["margin_px"], 10.0);
        let back: CanvasBackdrop = serde_json::from_value(params["backdrop"].clone()).unwrap();
        assert_eq!(frame_margin(&back), Some(10.0));
    }

    #[test]
    fn neck_diamond_sits_on_the_body_edge_beside_the_axis() {
        let body = RectF::new(0.0, 0.0, 100.0, 50.0);
        // Tip straight below: the tail leaves the bottom edge at x = 50.
        let tip = Point::new(50.0, 150.0);
        let n = tail_neck_point(body, tip, 24.0).unwrap();
        assert!(near(n, Point::new(38.0, 50.0)), "{n:?}");
        // Dragging the diamond 20 px off the axis makes the neck 40 wide.
        assert_eq!(
            neck_width_from_drag(body, tip, Point::new(70.0, 60.0)),
            Some(40.0)
        );
        // Never thinner than 2 px, never wider than twice the body.
        assert_eq!(neck_width_from_drag(body, tip, n), Some(24.0));
        assert_eq!(
            neck_width_from_drag(body, tip, Point::new(50.0, 80.0)),
            Some(2.0)
        );
        assert_eq!(
            neck_width_from_drag(body, tip, Point::new(1e6, 50.0)),
            Some(200.0)
        );
        // A tip at the centre has no axis.
        assert_eq!(tail_neck_point(body, body.center(), 24.0), None);
        let UiIntent::Command { id, params } = tail_neck_intent(NodeId(3), 1, tip, 40.0) else {
            panic!()
        };
        assert_eq!(id, "annotation.moveCalloutTail");
        assert_eq!(params["base_width"], 40.0);
        assert_eq!(params["tip"]["y"], 150.0);
    }

    #[test]
    fn magnifier_source_ring_is_the_lens_over_the_zoom() {
        let m = MagnifierLens::default();
        assert_eq!(magnifier_source_radius(&m), 40.0);
        let broken = MagnifierLens {
            zoom: 0.0,
            ..MagnifierLens::default()
        };
        assert_eq!(magnifier_source_radius(&broken), 80.0);
        let UiIntent::Command { id, params } =
            magnifier_source_intent(NodeId(7), Point::new(12.0, 34.0))
        else {
            panic!()
        };
        assert_eq!(id, "annotation.setMagnifierSource");
        assert_eq!(params["source_center"], json!({ "x": 12.0, "y": 34.0 }));
    }

    #[test]
    fn frame_corner_diamond_sets_the_radius() {
        let f = CanvasBackdrop::rounded_shadow_frame();
        assert_eq!(frame_corner_radius(&f), Some(12.0));
        assert_eq!(frame_corner_radius(&CanvasBackdrop::None), None);
        assert_eq!(
            frame_radius_from_drag([200, 100], Point::new(30.0, 30.0)),
            30.0
        );
        // Clamped to half the shorter side, and never negative.
        assert_eq!(
            frame_radius_from_drag([200, 100], Point::new(400.0, 400.0)),
            50.0
        );
        assert_eq!(
            frame_radius_from_drag([200, 100], Point::new(-9.0, -9.0)),
            0.0
        );
        let r = with_corner_radius(&f, 20.4);
        assert_eq!(frame_corner_radius(&r), Some(20.0));
        assert_eq!(frame_margin(&r), frame_margin(&f));
        assert_eq!(
            with_corner_radius(&CanvasBackdrop::None, 5.0),
            CanvasBackdrop::None
        );
    }

    #[test]
    fn framed_images_are_offset_by_the_margin() {
        let f = with_margin(&CanvasBackdrop::rounded_shadow_frame(), 20.0);
        assert_eq!(image_offset(&f, [100, 50], [140, 90]), -20.0);
        // The frame is not rendered yet (or a Cut Out gap): no offset.
        assert_eq!(image_offset(&f, [100, 50], [100, 50]), 0.0);
        assert_eq!(
            image_offset(&CanvasBackdrop::None, [100, 50], [100, 50]),
            0.0
        );
    }
}
