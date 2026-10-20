//! Zone 3: the interactive canvas.
//!
//! The document is rendered by `capturecraft-compositor` from
//! `capturecraft_engine::render::render_scene`, on the GPU into a wgpu
//! texture egui samples directly ([`GpuCanvas`]), or by the CPU oracle into
//! an egui texture where no wgpu device exists ([`CpuCanvas`]). The widget
//! draws it over a transparency checkerboard with pan (middle mouse or
//! Space + drag, the wheel) and cursor-centred zoom (Ctrl + wheel, pinch),
//! and on top of it:
//!
//! - eight small dark squares on the canvas border (corners and edge
//!   midpoints); dragging one out adds padding (inward trims), previews
//!   the new bounds and commits `canvas.expandBounds`;
//! - the Selection tool's marquee: drag to select a region of the capture,
//!   drag inside it to move its pixels (`smart.moveObject`), Delete to
//!   remove them (`smart.deleteObject`), with the hole left transparent or
//!   filled by the analyzer (`ToolOptions::selection.background`);
//! - selection outlines turned with the node's rotation and, for a single
//!   selection with the Move tool, the transform gizmo ([`super::gizmo`]):
//!   eight resize handles (Shift keeps the aspect, Alt resizes from the
//!   centre; `node.resize`), the rotation stem (Shift snaps to 15°;
//!   `node.rotate`) and the yellow corner-radius diamond
//!   (`annotation.setStyle`);
//! - a selected text box or callout (also with the Text and Callout tools)
//!   gets the text gizmo: a dashed border, round handles, a green rotation
//!   knob and a yellow padding diamond in the top-right corner
//!   (`annotation.setTextFormat`), plus the floating text toolbar
//!   ([`super::text_format`]);
//! - the eyedropper: the colour under the pointer (sampled from the
//!   composited document) in a tooltip; a click takes it, Esc cancels;
//! - yellow diamond parameter handles, all in the node's rotated frame:
//!   callout tail tips (drag to move, drag the `+` out of a callout to add
//!   a tail) and necks (drag sideways for the base width), the arrow bend
//!   handle (drag to curve, double-click to straighten) and the magnifier's
//!   source ring (moves the magnified area, the lens stays);
//! - with a rounded shadow frame, the frame margin around the canvas, four
//!   pill margin handles on its edges and a corner-radius diamond inside
//!   the top-left corner (`canvas.setBackdrop`);
//! - the Cut Out guide: a full-width or full-height band with seam lines
//!   showing the strip the drag would remove.
//!
//! Every drag changes the document once, by one command, when it ends.
//! Geometry questions (which node is under the cursor, where an arrow's
//! middle is) are answered by the engine.

use capturecraft_compositor::{CpuCompositor, GpuCompositor, GpuError, RenderScene};
use capturecraft_engine::DocState;
use capturecraft_engine::edit_commands::{arrow_midpoint, node_at};
use capturecraft_engine::node_commands::{expand_groups, is_text_box};
use capturecraft_format::{Annotation, Axis, Color, NodeId, RectF, TextAlign};
use eframe::egui_wgpu::{self, wgpu};
use egui::{
    Align2, Color32, ColorImage, CursorIcon, FontId, Id, Key, PointerButton, Pos2, Rect as ERect,
    Response, Sense, Shape, StrokeKind, TextureHandle, TextureId, TextureOptions, Ui, Vec2, pos2,
    vec2,
};
use photocraft_geom::{Point, Size};
use serde_json::{Value, json};

use super::gizmo::{
    self, GizmoHandle, NodeFrame, RADIUS_INSET_PT, RESIZE_HANDLES, STEM_PT, corner_radius_of,
    corner_radius_point, handle_point, padding_of, padding_point, rotate_handle_point,
};
use super::{canvas_tools, context_menu, right_sidebar, text_format};
use crate::UiIntent;
use crate::state::{
    CanvasGesture, ColorTarget, CropHit, EditorState, ExpandBackdrop, Eyedropper, ResizeDrag,
    ResizeHandle, ShapeKind, StylePreset, ToolId, Zoom,
};
use crate::theme::{Tokens, stroke};

/// Zoom limits, percent.
pub const MIN_ZOOM: f32 = 5.0;
pub const MAX_ZOOM: f32 = 3200.0;
/// Space kept around the canvas in Fit, points (room for the handles).
const FIT_MARGIN: f32 = 48.0;
/// Pick tolerance around annotations, points.
const PICK_SLOP: f32 = 4.0;

// ---------------------------------------------------------------------------
// Presentation

/// A rendered document ready for egui.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasImage {
    pub texture: TextureId,
    /// Rendered size in canvas pixels.
    pub size: [u32; 2],
    /// Part of the texture holding it (textures are allocated at the
    /// pre-Cut-Out size and reused).
    pub uv: ERect,
}

/// The wgpu compositor writing into a texture egui shows, sharing eframe's
/// device and queue.
pub struct GpuCanvas {
    compositor: GpuCompositor,
    target: Option<Target>,
    last: Option<(RenderScene, CanvasImage)>,
}

struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    id: TextureId,
    size: (u32, u32),
}

impl GpuCanvas {
    pub fn new(rs: &egui_wgpu::RenderState) -> Result<Self, GpuError> {
        Ok(Self {
            compositor: GpuCompositor::new(&rs.device, &rs.queue)?,
            target: None,
            last: None,
        })
    }

    /// Renders `scene` (skipped when it equals the last one; the compositor
    /// also caches per node revision).
    pub fn render(
        &mut self,
        rs: &egui_wgpu::RenderState,
        scene: &RenderScene,
    ) -> Result<CanvasImage, GpuError> {
        if let Some((s, img)) = &self.last
            && s == scene
        {
            return Ok(*img);
        }
        let need = (scene.width.max(1), scene.height.max(1));
        let max = rs.device.limits().max_texture_dimension_2d;
        if need.0 > max || need.1 > max {
            return Err(GpuError::Validation(format!(
                "{}×{} is larger than this GPU's {max} px textures",
                need.0, need.1
            )));
        }
        let fits = self
            .target
            .as_ref()
            .is_some_and(|t| t.size.0 >= need.0 && t.size.1 >= need.1);
        if !fits {
            let texture = rs.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("capturecraft canvas"),
                size: wgpu::Extent3d {
                    width: need.0,
                    height: need.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // Premultiplied, sRGB-encoded values: what egui-wgpu's
                // shader expects from a texture sample.
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mut renderer = rs.renderer.write();
            let id = match self.target.take() {
                Some(old) => {
                    renderer.update_egui_texture_from_wgpu_texture(
                        &rs.device,
                        &view,
                        wgpu::FilterMode::Linear,
                        old.id,
                    );
                    old.texture.destroy();
                    old.id
                }
                None => {
                    renderer.register_native_texture(&rs.device, &view, wgpu::FilterMode::Linear)
                }
            };
            self.target = Some(Target {
                texture,
                view,
                id,
                size: need,
            });
        }
        let Some(t) = &self.target else {
            return Err(GpuError::Validation("no canvas texture".into()));
        };
        let (w, h) = self.compositor.render_to_texture(scene, &t.view)?;
        let img = CanvasImage {
            texture: t.id,
            size: [w, h],
            uv: ERect::from_min_max(
                Pos2::ZERO,
                pos2(w as f32 / t.size.0 as f32, h as f32 / t.size.1 as f32),
            ),
        };
        self.last = Some((scene.clone(), img));
        Ok(img)
    }

    /// Releases the texture (call before dropping with the render state).
    pub fn free(&mut self, rs: &egui_wgpu::RenderState) {
        if let Some(t) = self.target.take() {
            rs.renderer.write().free_texture(&t.id);
        }
        self.last = None;
    }
}

/// The CPU oracle into an egui texture (glow backend, no adapter, headless
/// snapshots).
#[derive(Default)]
pub struct CpuCanvas {
    texture: Option<TextureHandle>,
    last: Option<(RenderScene, CanvasImage)>,
}

impl CpuCanvas {
    pub fn render(&mut self, ctx: &egui::Context, scene: &RenderScene) -> Option<CanvasImage> {
        if let Some((s, img)) = &self.last
            && s == scene
        {
            return Some(*img);
        }
        let frame = CpuCompositor::render(scene).ok()?;
        let pixels = frame
            .pixels
            .iter()
            .map(|p| {
                let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                Color32::from_rgba_premultiplied(c(p[0]), c(p[1]), c(p[2]), c(p[3]))
            })
            .collect();
        let image = ColorImage::new([frame.width as usize, frame.height as usize], pixels);
        let tex = match &mut self.texture {
            Some(t) => {
                t.set(image, TextureOptions::LINEAR);
                t.clone()
            }
            None => {
                let t = ctx.load_texture("capturecraft.canvas", image, TextureOptions::LINEAR);
                self.texture = Some(t.clone());
                t
            }
        };
        let img = CanvasImage {
            texture: tex.id(),
            size: [frame.width, frame.height],
            uv: ERect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
        };
        self.last = Some((scene.clone(), img));
        Some(img)
    }
}

// ---------------------------------------------------------------------------
// View transform

/// `screen = origin + canvas × scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewTransform {
    pub origin: Pos2,
    /// Points per canvas pixel.
    pub scale: f32,
}

impl ViewTransform {
    /// Where `zoom` and `scroll` put a `canvas`-sized image in `view`.
    /// Percent zoom is relative to physical pixels (100 % = 1:1 on screen);
    /// Fit never enlarges past 100 %.
    pub fn new(zoom: Zoom, scroll: [f32; 2], view: ERect, canvas: Size, ppp: f32) -> Self {
        let ppp = if ppp.is_finite() && ppp > 0.0 {
            ppp
        } else {
            1.0
        };
        let (w, h) = (canvas.width.max(1) as f32, canvas.height.max(1) as f32);
        let scale = match zoom {
            Zoom::Fit => {
                let avail = (view.size() - Vec2::splat(2.0 * FIT_MARGIN)).max(Vec2::splat(1.0));
                (avail.x / w).min(avail.y / h).min(1.0 / ppp)
            }
            Zoom::Percent { value } => clamp_zoom(value) / 100.0 / ppp,
        };
        let centred = view.center() - vec2(w, h) * scale / 2.0;
        Self {
            origin: centred + Vec2::from(scroll),
            scale,
        }
    }

    pub fn to_screen(&self, p: Point) -> Pos2 {
        self.origin + vec2(p.x as f32, p.y as f32) * self.scale
    }

    pub fn to_canvas(&self, p: Pos2) -> Point {
        let s = self.scale.max(f32::EPSILON);
        let v = (p - self.origin) / s;
        Point::new(f64::from(v.x), f64::from(v.y))
    }

    pub fn rect_to_screen(&self, r: RectF) -> ERect {
        ERect::from_min_max(
            self.to_screen(Point::new(r.x, r.y)),
            self.to_screen(Point::new(r.x + r.w, r.y + r.h)),
        )
    }

    /// Zoom percent this transform shows.
    pub fn percent(&self, ppp: f32) -> f32 {
        self.scale * ppp * 100.0
    }
}

fn clamp_zoom(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        100.0
    }
}

/// Zoom by `factor` keeping the canvas point under `cursor` still; returns
/// the new zoom and scroll.
pub fn zoom_about(
    current: &ViewTransform,
    factor: f32,
    cursor: Pos2,
    view: ERect,
    canvas: Size,
    ppp: f32,
) -> (Zoom, [f32; 2]) {
    let pct = clamp_zoom(current.percent(ppp) * factor);
    let zoom = Zoom::Percent { value: pct };
    let fresh = ViewTransform::new(zoom, [0.0, 0.0], view, canvas, ppp);
    let k = fresh.scale / current.scale.max(f32::EPSILON);
    let origin = cursor - (cursor - current.origin) * k;
    let scroll = origin - fresh.origin;
    (zoom, [scroll.x, scroll.y])
}

// ---------------------------------------------------------------------------
// Expansion handles

/// The eight resize squares of `canvas` (screen rect), centred on its
/// corners and edge midpoints.
pub fn expand_handle_rects(canvas: ERect, t: &Tokens) -> [(ResizeHandle, ERect); 8] {
    let side = Vec2::splat(t.canvas_handle_size);
    let (c, r) = (canvas.center(), canvas);
    let at = |x: f32, y: f32| ERect::from_center_size(pos2(x, y), side);
    [
        (ResizeHandle::N, at(c.x, r.min.y)),
        (ResizeHandle::S, at(c.x, r.max.y)),
        (ResizeHandle::W, at(r.min.x, c.y)),
        (ResizeHandle::E, at(r.max.x, c.y)),
        (ResizeHandle::NW, at(r.min.x, r.min.y)),
        (ResizeHandle::NE, at(r.max.x, r.min.y)),
        (ResizeHandle::SW, at(r.min.x, r.max.y)),
        (ResizeHandle::SE, at(r.max.x, r.max.y)),
    ]
}

fn inside(r: RectF, p: Point) -> bool {
    p.x >= r.x && p.y >= r.y && p.x <= r.x + r.w && p.y <= r.y + r.h
}

/// The marquee's base-space rect as `smart.*Object` wants it.
fn marquee_base(m: &capturecraft_format::Manifest, r: RectF) -> Option<Value> {
    let b = capturecraft_engine::commands::canvas_rect_to_base(m, r)?;
    Some(json!({ "x0": b.x0, "y0": b.y0, "x1": b.x1, "y1": b.y1 }))
}

/// `smart.moveObject` for a marquee dragged by `(dx, dy)` canvas pixels.
pub fn move_selection_intent(
    editor: &EditorState,
    m: &capturecraft_format::Manifest,
    dx: f64,
    dy: f64,
) -> Option<UiIntent> {
    let rect = marquee_base(m, editor.canvas.marquee?)?;
    (dx != 0.0 || dy != 0.0).then(|| {
        UiIntent::command(
            "smart.moveObject",
            json!({
                "rect": rect,
                "by": { "x": dx, "y": dy },
                "matte": false,
                "background": editor.tools.selection.background.param(),
            }),
        )
    })
}

/// `smart.deleteObject` for the marquee.
pub fn delete_selection_intent(
    editor: &EditorState,
    m: &capturecraft_format::Manifest,
) -> Option<UiIntent> {
    let rect = marquee_base(m, editor.canvas.marquee?)?;
    Some(UiIntent::command(
        "smart.deleteObject",
        json!({ "rect": rect, "background": editor.tools.selection.background.param() }),
    ))
}

/// Margins `[left, top, right, bottom]` a finished handle drag adds.
pub fn expand_margins(d: &ResizeDrag) -> [i32; 4] {
    let (size, origin) = d.preview();
    let left = origin.x.saturating_sub(d.start_origin.x);
    let top = origin.y.saturating_sub(d.start_origin.y);
    let grow = |new: u32, old: u32, near: i32| {
        (i64::from(new) - i64::from(old) - i64::from(near))
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    };
    [
        left,
        top,
        grow(size.width, d.start_size.width, left),
        grow(size.height, d.start_size.height, top),
    ]
}

/// `canvas.expandBounds` parameters, `None` when nothing changes.
pub fn expand_params(d: &ResizeDrag, backdrop: ExpandBackdrop) -> Option<Value> {
    let [left, top, right, bottom] = expand_margins(d);
    (left != 0 || top != 0 || right != 0 || bottom != 0).then(|| {
        json!({
            "left": left, "top": top, "right": right, "bottom": bottom,
            "background": backdrop.param(),
        })
    })
}

// ---------------------------------------------------------------------------
// Tool parameters

fn pt(p: Point) -> Value {
    json!({ "x": p.x, "y": p.y })
}

/// The selected style as creation parameters of `tool`'s command.
fn creation_params(tool: ToolId, style: &StylePreset) -> serde_json::Map<String, Value> {
    let all = style.style_params();
    let keep: &[&str] = match tool {
        ToolId::Arrow => &["stroke", "shadow"],
        ToolId::Callout => &["fill", "stroke", "shadow", "text_color", "font"],
        ToolId::Step => &["text_color", "shadow"],
        ToolId::Shape => &["fill", "stroke", "shadow"],
        ToolId::Highlighter => &["fill"],
        _ => &[],
    };
    let mut p: serde_json::Map<String, Value> = all
        .into_iter()
        .filter(|(k, _)| keep.contains(&k.as_str()))
        .collect();
    match tool {
        ToolId::Arrow => {
            if let Some(h) = style.arrow_start {
                p.insert("start_head".into(), json!(h));
            }
            if let Some(h) = style.arrow_end {
                p.insert("end_head".into(), json!(h));
            }
        }
        ToolId::Step => {
            if let Some(c) = style.fill_color() {
                p.insert("fill".into(), json!(c));
            }
            if let Some(s) = &style.stroke {
                p.insert("ring".into(), json!(s));
            }
        }
        ToolId::Callout => {
            if let Some(r) = style.corner_radius {
                p.insert(
                    "body".into(),
                    json!({ "kind": "rounded_rect", "radius": r }),
                );
            }
        }
        ToolId::Shape => {
            if let Some(r) = style.corner_radius {
                p.insert("corner_radius".into(), json!(r));
            }
            // A preset without a stroke draws a filled shape only.
            if style.stroke.is_none() {
                p.insert("stroke".into(), Value::Null);
            }
        }
        ToolId::Highlighter => {
            p.insert("stroke".into(), Value::Null);
            p.insert("blend".into(), json!("multiply"));
        }
        _ => {}
    }
    p
}

/// Smallest side a region tool's drag must span, canvas pixels; shorter
/// drags (and clicks) add nothing.
const MIN_REGION: f64 = 3.0;

fn normalized(a: Point, b: Point) -> RectF {
    RectF::new(
        a.x.min(b.x),
        a.y.min(b.y),
        (a.x - b.x).abs(),
        (a.y - b.y).abs(),
    )
}

/// The command a finished drawing gesture runs.
pub fn draw_intent(editor: &EditorState, from: Point, to: Point) -> Option<UiIntent> {
    let tool = editor.toolbar.active;
    let style = editor.sidebar.effective_style(tool).unwrap_or_default();
    let mut p = creation_params(tool, &style);
    let dist = ((to.x - from.x).powi(2) + (to.y - from.y).powi(2)).sqrt();
    match tool {
        ToolId::Arrow if dist >= 3.0 => {
            p.insert("start".into(), pt(from));
            p.insert("end".into(), pt(to));
            Some(UiIntent::command("annotation.addArrow", Value::Object(p)))
        }
        ToolId::Callout => {
            let mut b = normalized(from, to);
            if b.w < 8.0 || b.h < 8.0 {
                // A click drops a default-sized bubble centred on it.
                b = RectF::new(from.x - 100.0, from.y - 45.0, 200.0, 90.0);
            }
            let o = editor.tools.callout;
            let tip = Point::new(b.x + b.w * 0.25, b.y + b.h + (b.h * 0.6).max(30.0));
            p.insert(
                "bounds".into(),
                json!({ "x": b.x, "y": b.y, "w": b.w, "h": b.h }),
            );
            p.insert(
                "tails".into(),
                json!([{ "tip": pt(tip), "base_width": o.tail_width, "curve_bias": o.tail_curve }]),
            );
            Some(UiIntent::command("annotation.addCallout", Value::Object(p)))
        }
        ToolId::Step => {
            let o = &editor.tools.step;
            p.insert("at".into(), pt(to));
            p.insert("sequence".into(), json!(o.sequence));
            p.insert("numbering".into(), json!(o.numbering));
            Some(UiIntent::command("annotation.addStep", Value::Object(p)))
        }
        ToolId::Magnify => {
            let o = editor.tools.magnify;
            // A click drops the default lens; a drag sets its radius.
            let radius = if dist >= 8.0 {
                dist
            } else {
                f64::from(o.radius)
            };
            let mut q = json!({ "center": pt(from), "radius": radius, "zoom": o.zoom });
            if let Some(s) = &style.stroke {
                q["border"] = json!(s);
            }
            Some(UiIntent::command("annotation.addMagnifier", q))
        }
        ToolId::Spotlight => {
            let b = normalized(from, to);
            if b.w < MIN_REGION || b.h < MIN_REGION {
                return None;
            }
            let o = editor.tools.spotlight;
            Some(UiIntent::command(
                "annotation.addSpotlight",
                json!({
                    "bounds": { "x": b.x, "y": b.y, "w": b.w, "h": b.h },
                    "kind": if o.ellipse { "ellipse" } else { "rect" },
                    "dim_opacity": o.dim_opacity,
                }),
            ))
        }
        ToolId::Shape if editor.tools.shape.kind == ShapeKind::Polygon => None,
        ToolId::Shape | ToolId::Highlighter | ToolId::Blur => {
            let b = normalized(from, to);
            if b.w < MIN_REGION || b.h < MIN_REGION {
                return None;
            }
            p.insert(
                "bounds".into(),
                json!({ "x": b.x, "y": b.y, "w": b.w, "h": b.h }),
            );
            let id = match tool {
                ToolId::Shape => {
                    p.insert("kind".into(), json!(editor.tools.shape.kind()));
                    "annotation.addShape"
                }
                ToolId::Highlighter => "annotation.addShape",
                _ => {
                    p.insert("effect".into(), json!(editor.tools.blur.effect()));
                    "annotation.addBlur"
                }
            };
            Some(UiIntent::command(id, Value::Object(p)))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The widget

pub struct CanvasViewport<'a> {
    pub doc: Option<&'a DocState>,
    pub image: Option<CanvasImage>,
    pub tokens: &'a Tokens,
}

impl CanvasViewport<'_> {
    pub fn show(
        self,
        ui: &mut Ui,
        editor: &mut EditorState,
        intents: &mut Vec<UiIntent>,
    ) -> Response {
        let t = self.tokens;
        let view = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(view, Sense::click_and_drag());
        let painter = ui.painter_at(view);
        painter.rect_filled(view, 0.0, t.canvas_surround);
        let Some(doc) = self.doc else {
            painter.text(
                view.center(),
                Align2::CENTER_CENTER,
                "No capture open — press the capture hotkey or New Capture",
                FontId::proportional(14.0),
                t.text_faint,
            );
            return resp;
        };
        let m = &doc.manifest;
        let canvas = m.canvas.size;
        let ppp = ui.ctx().pixels_per_point();
        let state = &mut editor.canvas;
        let mut xf = ViewTransform::new(state.zoom, state.scroll, view, canvas, ppp);

        // --- Pan and zoom -------------------------------------------------
        let (space, middle, primary, zoom_delta, wheel, pointer) = ui.input(|i| {
            (
                i.key_down(Key::Space),
                i.pointer.button_down(PointerButton::Middle),
                i.pointer.primary_down(),
                i.zoom_delta(),
                i.smooth_scroll_delta,
                i.pointer.hover_pos(),
            )
        });
        let hovered = resp.hovered() || resp.dragged();
        let panning = hovered && (middle || (space && primary));
        if panning || matches!(state.gesture, Some(CanvasGesture::Pan)) {
            let d = ui.input(|i| i.pointer.delta());
            if d != Vec2::ZERO {
                if matches!(state.zoom, Zoom::Fit) {
                    state.zoom = Zoom::Percent {
                        value: xf.percent(ppp),
                    };
                }
                state.scroll = [state.scroll[0] + d.x, state.scroll[1] + d.y];
            }
            state.gesture = (middle || primary).then_some(CanvasGesture::Pan);
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if space && hovered {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        if hovered && let Some(cur) = pointer {
            if zoom_delta != 1.0 {
                (state.zoom, state.scroll) = zoom_about(&xf, zoom_delta, cur, view, canvas, ppp);
            } else if wheel != Vec2::ZERO {
                if matches!(state.zoom, Zoom::Fit) {
                    state.zoom = Zoom::Percent {
                        value: xf.percent(ppp),
                    };
                }
                state.scroll = [state.scroll[0] + wheel.x, state.scroll[1] + wheel.y];
            }
        }
        xf = ViewTransform::new(state.zoom, state.scroll, view, canvas, ppp);
        let canvas_rect = xf.rect_to_screen(RectF::new(
            0.0,
            0.0,
            f64::from(canvas.width),
            f64::from(canvas.height),
        ));

        // --- Pixels -------------------------------------------------------
        if state.checkerboard {
            let checker = checker_texture(ui.ctx(), t);
            let n = 2.0 * t.checker_size;
            painter.image(
                checker.id(),
                canvas_rect,
                ERect::from_min_max(
                    Pos2::ZERO,
                    pos2(canvas_rect.width() / n, canvas_rect.height() / n),
                ),
                Color32::WHITE,
            );
        }
        if let Some(img) = self.image {
            let off =
                gizmo::image_offset(&m.canvas.backdrop, [canvas.width, canvas.height], img.size);
            let r = xf.rect_to_screen(RectF::new(
                off,
                off,
                f64::from(img.size[0]),
                f64::from(img.size[1]),
            ));
            painter.image(img.texture, r, img.uv, Color32::WHITE);
        }
        painter.rect_stroke(
            canvas_rect,
            0.0,
            stroke(1.0, t.canvas_edge),
            StrokeKind::Outside,
        );
        if let Some(radius) = gizmo::frame_corner_radius(&m.canvas.backdrop) {
            frame_radius_handle(
                ui,
                &painter,
                &xf,
                canvas,
                &m.canvas.backdrop,
                radius,
                editor,
                intents,
                t,
            );
        }
        if let Some(margin) = gizmo::frame_margin(&m.canvas.backdrop) {
            frame_margin_handles(
                ui,
                &painter,
                &xf,
                canvas,
                &m.canvas.backdrop,
                margin,
                editor,
                intents,
                t,
            );
        }

        let tool = editor.toolbar.active;
        let to_canvas = |p: Pos2| xf.to_canvas(p);
        let slop = f64::from(PICK_SLOP / xf.scale.max(f32::EPSILON));

        // --- Expansion handles -------------------------------------------
        if editor.canvas.resize.handles_visible {
            for (handle, rect) in expand_handle_rects(canvas_rect, t) {
                let h = ui.interact(rect, Id::new(("canvas.expand", handle)), Sense::drag());
                let hot = h.hovered() || h.dragged();
                let fill = if hot {
                    t.expand_preview
                } else {
                    t.canvas_handle_fill
                };
                painter.rect(
                    rect,
                    0.0,
                    fill,
                    stroke(1.0, t.canvas_handle_stroke),
                    StrokeKind::Outside,
                );
                if hot {
                    ui.ctx().set_cursor_icon(resize_cursor(handle));
                }
                let r = &mut editor.canvas.resize;
                if h.drag_started() {
                    r.drag = Some(ResizeDrag {
                        handle,
                        start_size: canvas,
                        start_origin: m.canvas.base_origin,
                        delta: [0.0, 0.0],
                    });
                }
                if let Some(d) = r.drag.as_mut().filter(|d| d.handle == handle) {
                    if h.dragged() {
                        let dd = h.drag_delta() / xf.scale.max(f32::EPSILON);
                        d.delta = [d.delta[0] + dd.x, d.delta[1] + dd.y];
                    }
                    if h.drag_stopped() {
                        if let Some(p) = expand_params(d, editor.canvas.expand_backdrop) {
                            intents.push(UiIntent::command("canvas.expandBounds", p));
                        }
                        r.drag = None;
                    }
                }
            }
            if let Some(d) = &editor.canvas.resize.drag {
                let [l, tp, _, _] = expand_margins(d);
                let (size, _) = d.preview();
                let preview = xf.rect_to_screen(RectF::new(
                    -f64::from(l),
                    -f64::from(tp),
                    f64::from(size.width),
                    f64::from(size.height),
                ));
                dashed_rect(&painter, preview, t.expand_preview);
                badge(
                    &painter,
                    preview.center_bottom() + vec2(0.0, t.space_lg),
                    &format!("{} × {} px", size.width, size.height),
                    t,
                );
            }
        }

        // --- Selection and gizmos -----------------------------------------
        let selection = editor.canvas.selection.clone();
        let moving = match &editor.canvas.gesture {
            Some(CanvasGesture::MoveSelection { from, to }) => (to.x - from.x, to.y - from.y),
            _ => (0.0, 0.0),
        };
        let mods = ui.input(|i| i.modifiers);
        if let Some(h) = editor.canvas.hovered.filter(|h| !selection.contains(h))
            && let Some(f) = m.node(h).and_then(NodeFrame::of)
        {
            frame_outline(&painter, &xf, &f, stroke(1.0, t.gizmo_stroke));
        }
        // The Text and Callout tools also transform a selected text.
        let gizmo_tool = tool == ToolId::Move
            || (matches!(tool, ToolId::Text | ToolId::Callout)
                && text_format::selected_text(m, &selection).is_some());
        let single = (selection.len() == 1 && gizmo_tool)
            .then(|| selection.first().copied())
            .flatten();
        for &id in &selection {
            let Some(node) = m.node(id) else {
                continue;
            };
            let Some(a) = node.annotation() else {
                continue;
            };
            let base = NodeFrame::of(node);
            if let Some(mut f) = base {
                let mut radius = None;
                let mut padding = None;
                match editor.canvas.gesture {
                    Some(CanvasGesture::Resize {
                        id: g,
                        handle,
                        local,
                    }) if g == id => {
                        if let Some((_, p)) = gizmo::resize_preview(
                            a, f.rotation, handle, local, mods.shift, mods.alt,
                        ) {
                            f = p;
                        }
                    }
                    Some(CanvasGesture::Rotate { id: g, from, to }) if g == id => {
                        f.rotation =
                            gizmo::rotation_from_drag(f.pivot, from, to, f.rotation, mods.shift);
                    }
                    Some(CanvasGesture::CornerRadius { id: g, radius: r }) if g == id => {
                        radius = Some(r);
                    }
                    Some(CanvasGesture::Padding { id: g, padding: p }) if g == id => {
                        padding = Some(p);
                    }
                    _ => {}
                }
                f.bounds.translate(moving.0, moving.1);
                f.pivot = Point::new(f.pivot.x + moving.0, f.pivot.y + moving.1);
                if gizmo::is_text_node(a) {
                    dashed_frame(&painter, &xf, &f, t);
                } else {
                    frame_outline(&painter, &xf, &f, stroke(3.0, t.gizmo_outline));
                    frame_outline(&painter, &xf, &f, stroke(1.5, t.gizmo_stroke));
                }
                if single == Some(id) {
                    let resizing = matches!(
                        editor.canvas.gesture,
                        Some(CanvasGesture::Resize { .. } | CanvasGesture::MoveSelection { .. })
                    );
                    let previews = Previews {
                        diamonds: !resizing,
                        radius,
                        padding,
                    };
                    draw_gizmo(&painter, &xf, &f, a, previews, t);
                }
            }
            // Handles below live in the node's (unrotated) geometry space.
            let to_scr = |p: Point| xf.to_screen(base.map_or(p, |f| f.to_canvas(p)));
            let to_local = |p: Pos2| {
                let c = to_canvas(p);
                base.map_or(c, |f| f.to_local(c))
            };
            match a {
                Annotation::Callout(c) => {
                    for (index, tail) in c.tails.iter().enumerate() {
                        let preview = match &editor.canvas.gesture {
                            Some(CanvasGesture::CalloutTail {
                                id: g,
                                index: gi,
                                to,
                            }) if *g == id && *gi == index => Some(*to),
                            _ => None,
                        };
                        let at = to_scr(preview.unwrap_or(tail.tip));
                        let h = ui.interact(
                            ERect::from_center_size(to_scr(tail.tip), Vec2::splat(16.0)),
                            Id::new(("canvas.tail", id.0, index)),
                            Sense::drag(),
                        );
                        diamond(
                            &painter,
                            at,
                            if h.hovered() { 8.0 } else { 6.5 },
                            t.tail_handle,
                            t,
                        );
                        if let Some(p) = h.interact_pointer_pos() {
                            if h.dragged() {
                                editor.canvas.gesture = Some(CanvasGesture::CalloutTail {
                                    id,
                                    index,
                                    to: to_local(p),
                                });
                            }
                            if h.drag_stopped() {
                                intents.push(UiIntent::command(
                                    "annotation.moveCalloutTail",
                                    json!({ "id": id.0, "index": index, "tip": pt(to_local(p)) }),
                                ));
                                editor.canvas.gesture = None;
                            }
                        }
                        let width = match &editor.canvas.gesture {
                            Some(CanvasGesture::CalloutNeck {
                                id: g,
                                index: gi,
                                width,
                            }) if *g == id && *gi == index => *width,
                            _ => tail.base_width,
                        };
                        let (Some(rest), Some(shown)) = (
                            gizmo::tail_neck_point(c.bounds, tail.tip, tail.base_width),
                            gizmo::tail_neck_point(c.bounds, tail.tip, width),
                        ) else {
                            continue;
                        };
                        let h = ui.interact(
                            ERect::from_center_size(to_scr(rest), Vec2::splat(14.0)),
                            Id::new(("canvas.neck", id.0, index)),
                            Sense::drag(),
                        );
                        diamond(
                            &painter,
                            to_scr(shown),
                            if h.hovered() { 6.5 } else { 5.0 },
                            t.tail_handle,
                            t,
                        );
                        if let Some(p) = h.interact_pointer_pos() {
                            let w = gizmo::neck_width_from_drag(c.bounds, tail.tip, to_local(p));
                            if h.dragged()
                                && let Some(width) = w
                            {
                                editor.canvas.gesture =
                                    Some(CanvasGesture::CalloutNeck { id, index, width });
                            }
                            if h.drag_stopped() {
                                if let Some(w) = w.filter(|w| *w != tail.base_width) {
                                    intents.push(gizmo::tail_neck_intent(id, index, tail.tip, w));
                                }
                                editor.canvas.gesture = None;
                            }
                        }
                        h.on_hover_text("Drag to change the tail's width");
                    }
                    // "+" below the body: drag out a new tail.
                    let below = f64::from(18.0 / xf.scale.max(f32::EPSILON));
                    let bottom = Point::new(
                        c.bounds.x + c.bounds.w / 2.0,
                        c.bounds.y + c.bounds.h + below,
                    );
                    let plus = to_scr(bottom);
                    let body_center = to_scr(c.bounds.center());
                    let h = ui.interact(
                        ERect::from_center_size(plus, Vec2::splat(16.0)),
                        Id::new(("canvas.newtail", id.0)),
                        Sense::drag(),
                    );
                    let new_tip = match &editor.canvas.gesture {
                        Some(CanvasGesture::NewTail { id: g, to }) if *g == id => Some(*to),
                        _ => None,
                    };
                    if let Some(tip) = new_tip {
                        let s = to_scr(tip);
                        painter.line_segment([body_center, s], stroke(1.0, t.tail_handle));
                        diamond(&painter, s, 6.5, t.tail_handle, t);
                    }
                    diamond(&painter, plus, 6.5, t.handle_fill, t);
                    painter.text(
                        plus,
                        Align2::CENTER_CENTER,
                        "+",
                        FontId::proportional(11.0),
                        t.handle_stroke,
                    );
                    if let Some(p) = h.interact_pointer_pos() {
                        if h.dragged() {
                            editor.canvas.gesture = Some(CanvasGesture::NewTail {
                                id,
                                to: to_local(p),
                            });
                        }
                        if h.drag_stopped() {
                            let o = editor.tools.callout;
                            intents.push(UiIntent::command(
                                "annotation.addCalloutTail",
                                json!({ "id": id.0, "tail": {
                                    "tip": pt(to_local(p)),
                                    "base_width": o.tail_width,
                                    "curve_bias": o.tail_curve,
                                } }),
                            ));
                            editor.canvas.gesture = None;
                        }
                    }
                }
                Annotation::CurvedArrow(arrow) => {
                    let mid = arrow_midpoint(arrow);
                    let preview = match &editor.canvas.gesture {
                        Some(CanvasGesture::ArrowBend { id: g, to }) if *g == id => Some(*to),
                        _ => None,
                    };
                    if let Some(to) = preview {
                        let pts: Vec<Pos2> = [arrow.start, to, arrow.end]
                            .into_iter()
                            .map(to_scr)
                            .collect();
                        painter.extend(Shape::dashed_line(
                            &pts,
                            stroke(1.0, t.bend_handle),
                            5.0,
                            4.0,
                        ));
                    }
                    let at = to_scr(preview.unwrap_or(mid));
                    let h = ui.interact(
                        ERect::from_center_size(to_scr(mid), Vec2::splat(16.0)),
                        Id::new(("canvas.bend", id.0)),
                        Sense::click_and_drag(),
                    );
                    diamond(
                        &painter,
                        at,
                        if h.hovered() { 8.0 } else { 6.5 },
                        t.tail_handle,
                        t,
                    );
                    h.clone()
                        .on_hover_text("Drag to bend · double-click to straighten");
                    if h.double_clicked() {
                        intents.push(UiIntent::command(
                            "annotation.setArrowCurve",
                            json!({ "id": id.0 }),
                        ));
                    }
                    if let Some(p) = h.interact_pointer_pos() {
                        if h.dragged() {
                            editor.canvas.gesture = Some(CanvasGesture::ArrowBend {
                                id,
                                to: to_local(p),
                            });
                        }
                        if h.drag_stopped() {
                            intents.push(UiIntent::command(
                                "annotation.setArrowCurve",
                                json!({ "id": id.0, "through": pt(to_local(p)) }),
                            ));
                            editor.canvas.gesture = None;
                        }
                    }
                }
                Annotation::MagnifierLens(lens) => {
                    let preview = match &editor.canvas.gesture {
                        Some(CanvasGesture::MagnifierSource { id: g, to }) if *g == id => Some(*to),
                        _ => None,
                    };
                    let src = to_scr(preview.unwrap_or(lens.source_center));
                    let lens_center = to_scr(lens.lens.bounds().center());
                    let ring = gizmo::magnifier_source_radius(lens) * xf.scale;
                    painter.extend(Shape::dashed_line(
                        &[lens_center, src],
                        stroke(1.0, t.tail_handle),
                        5.0,
                        4.0,
                    ));
                    painter.circle_stroke(src, ring, stroke(3.0, t.gizmo_outline));
                    painter.circle_stroke(src, ring, stroke(1.5, t.tail_handle));
                    let h = ui.interact(
                        ERect::from_center_size(to_scr(lens.source_center), Vec2::splat(16.0)),
                        Id::new(("canvas.magnifier_source", id.0)),
                        Sense::drag(),
                    );
                    diamond(
                        &painter,
                        src,
                        if h.hovered() { 8.0 } else { 6.5 },
                        t.tail_handle,
                        t,
                    );
                    if let Some(p) = h.interact_pointer_pos() {
                        if h.dragged() {
                            editor.canvas.gesture = Some(CanvasGesture::MagnifierSource {
                                id,
                                to: to_local(p),
                            });
                        }
                        if h.drag_stopped() {
                            let to = to_local(p);
                            if to != lens.source_center {
                                intents.push(gizmo::magnifier_source_intent(id, to));
                            }
                            editor.canvas.gesture = None;
                        }
                    }
                    h.on_hover_text("Drag to move the magnified area");
                }
                _ => {}
            }
        }

        // --- Cut Out guide -------------------------------------------------
        let axis = editor.tools.cut_out.axis;
        if tool == ToolId::CutOut {
            let band = match (&editor.canvas.gesture, resp.hover_pos()) {
                (Some(CanvasGesture::CutOut { from, to }), _) => Some((*from, *to)),
                (None, Some(p)) => {
                    let c = to_canvas(p);
                    let v = if axis == Axis::Horizontal { c.y } else { c.x };
                    Some((v, v))
                }
                _ => None,
            };
            if let Some((a, b)) = band {
                draw_cut_band(
                    &painter,
                    &xf,
                    canvas,
                    axis,
                    a.min(b),
                    a.max(b),
                    editor.tools.cut_out.torn,
                    t,
                );
            }
        }

        // --- Canvas gestures (tools) ---------------------------------------
        if panning || matches!(editor.canvas.gesture, Some(CanvasGesture::Pan)) {
            return resp;
        }
        let along = |p: Point| if axis == Axis::Horizontal { p.y } else { p.x };
        // The gizmo of a single selection, with its node.
        let gizmo_node = single.and_then(|id| {
            let n = m.node(id)?;
            Some((id, NodeFrame::of(n)?, n.annotation()?))
        });
        let gizmo_hit = |p: Pos2| {
            gizmo_node.and_then(|(_, f, a)| {
                gizmo::hit_handle(&f, a, to_canvas(p), xf.scale, t.handle_size)
            })
        };
        let mut over_handle = false;
        let picking = editor.canvas.eyedropper.is_some();
        if gizmo_tool
            && !picking
            && let Some(p) = resp.hover_pos()
        {
            match (gizmo_hit(p), &gizmo_node) {
                (Some(h), Some((_, f, _))) => {
                    over_handle = true;
                    editor.canvas.hovered = None;
                    ui.ctx()
                        .set_cursor_icon(gizmo::handle_cursor(h, f.rotation));
                }
                _ if tool == ToolId::Move => {
                    editor.canvas.hovered = node_at(m, to_canvas(p), slop);
                }
                _ => editor.canvas.hovered = None,
            }
        } else {
            editor.canvas.hovered = None;
        }

        let full = RectF::new(0.0, 0.0, f64::from(canvas.width), f64::from(canvas.height));
        if tool == ToolId::Crop {
            editor.canvas.crop.get_or_insert(full);
        } else {
            editor.canvas.crop = None;
        }
        if tool != ToolId::Shape || editor.tools.shape.kind != ShapeKind::Polygon {
            editor.canvas.polygon.clear();
        }
        // A click while typing only ends the typing (the text field's
        // lost focus commits it below); one while the menu is open only
        // closes the menu.
        let typing_before = editor.canvas.text_edit.is_some() || context_menu::is_open(ui.ctx());
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            context_menu::select_for_menu(editor, doc, node_at(m, to_canvas(p), slop));
            context_menu::open(ui.ctx(), p);
        }
        if resp.double_clicked()
            && !typing_before
            && matches!(tool, ToolId::Move | ToolId::Text | ToolId::Callout)
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(e) = canvas_tools::text_box_at(m, to_canvas(p))
                .and_then(|id| canvas_tools::begin_edit_text(m, id))
        {
            editor.canvas.text_edit = Some(e);
        }
        // The eyedropper also picks while a text is typed (for its colour).
        let mut picked = false;
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(eye) = editor.canvas.eyedropper.take()
        {
            pick_color(ui.ctx(), doc, to_canvas(p), eye, editor, intents);
            picked = true;
        }
        if resp.clicked()
            && !picked
            && !over_handle
            && !typing_before
            && let Some(p) = resp.interact_pointer_pos()
        {
            let c = to_canvas(p);
            if tool == ToolId::Text {
                let e = canvas_tools::text_box_at(m, c)
                    .and_then(|id| canvas_tools::begin_edit_text(m, id))
                    .unwrap_or_else(|| canvas_tools::begin_new_text(editor, c, None));
                // The text being typed is the selection (none for a new one
                // until it is added).
                editor.canvas.selection = e.node.into_iter().collect();
                editor.canvas.text_edit = Some(e);
            } else if tool == ToolId::Callout
                && let Some(e) = canvas_tools::text_box_at(m, c)
                    .filter(|id| {
                        matches!(
                            m.node(*id).and_then(|n| n.annotation()),
                            Some(Annotation::Callout(cl)) if !is_text_box(cl)
                        )
                    })
                    .and_then(|id| canvas_tools::begin_edit_text(m, id))
            {
                // The Callout tool clicks into a callout's text, like the
                // Text tool into a text box.
                editor.canvas.selection = e.node.into_iter().collect();
                editor.canvas.text_edit = Some(e);
            } else if tool == ToolId::Shape && editor.tools.shape.kind == ShapeKind::Polygon {
                if resp.double_clicked() {
                    let corners = std::mem::take(&mut editor.canvas.polygon);
                    intents.extend(canvas_tools::polygon_intent(editor, &corners));
                } else {
                    editor.canvas.polygon.push(c);
                }
            } else if tool == ToolId::Eraser {
                intents.extend(canvas_tools::stroke_intent(editor, &[c]));
            } else if tool == ToolId::Crop {
                if resp.double_clicked()
                    && let Some(r) = editor.canvas.crop
                    && let Some(i) = canvas_tools::crop_intent(m, r)
                {
                    intents.push(i);
                    editor.canvas.crop = None;
                }
            } else if tool == ToolId::GrabText {
                // A click reads the whole capture.
                intents.push(grab_text_intent(m, None));
            } else if tool == ToolId::Pen {
            } else if tool == ToolId::Move {
                let shift = ui.input(|i| i.modifiers.shift);
                match node_at(m, c, slop) {
                    Some(id) if shift => {
                        for g in expand_groups(m, &[id]) {
                            toggle(&mut editor.canvas.selection, g);
                        }
                    }
                    Some(id) => editor.canvas.selection = expand_groups(m, &[id]),
                    None if !shift => editor.canvas.selection.clear(),
                    None => {}
                }
            } else if tool == ToolId::Selection {
                if !editor.canvas.marquee.is_some_and(|r| inside(r, c)) {
                    editor.canvas.marquee = None;
                }
            } else if let Some(i) = draw_intent(editor, c, c) {
                intents.push(i);
            }
        }
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(o) = ui.input(|i| i.pointer.press_origin())
        {
            let c = to_canvas(o);
            let handle = gizmo_tool.then(|| gizmo_hit(o)).flatten();
            editor.canvas.gesture = match (tool, handle, gizmo_node) {
                _ if typing_before || picking => None,
                (_, Some(h), Some((id, f, a))) => Some(match h {
                    GizmoHandle::Resize(handle) => CanvasGesture::Resize {
                        id,
                        handle,
                        local: f.to_local(c),
                    },
                    GizmoHandle::Rotate => CanvasGesture::Rotate { id, from: c, to: c },
                    GizmoHandle::CornerRadius => CanvasGesture::CornerRadius {
                        id,
                        radius: corner_radius_of(a).map_or(0.0, |(_, r)| r),
                    },
                    GizmoHandle::Padding => CanvasGesture::Padding {
                        id,
                        padding: padding_of(a).map_or(0.0, |(_, p)| p),
                    },
                }),
                (ToolId::Move, _, _) => {
                    let hit = node_at(m, c, slop);
                    if let Some(id) = hit
                        && !editor.canvas.selection.contains(&id)
                    {
                        editor.canvas.selection = expand_groups(m, &[id]);
                    }
                    hit.map(|_| CanvasGesture::MoveSelection { from: c, to: c })
                }
                (ToolId::Pen | ToolId::Eraser, _, _) => {
                    Some(CanvasGesture::Stroke { points: vec![c] })
                }
                (ToolId::Crop, _, _) => {
                    let r = editor.canvas.crop.unwrap_or(full);
                    let tol = f64::from(CROP_REACH / xf.scale.max(f32::EPSILON));
                    Some(CanvasGesture::Crop {
                        hit: canvas_tools::crop_hit(r, c, tol),
                        start: r,
                        from: c,
                        to: c,
                    })
                }
                (ToolId::Shape, _, _) if editor.tools.shape.kind == ShapeKind::Polygon => None,
                (ToolId::CutOut, _, _) => Some(CanvasGesture::CutOut {
                    from: along(c),
                    to: along(c),
                }),
                (ToolId::Selection, _, _) => Some(match editor.canvas.marquee {
                    Some(r) if inside(r, c) => CanvasGesture::MoveMarquee { from: c, to: c },
                    _ => CanvasGesture::Marquee { from: c, to: c },
                }),
                _ => Some(CanvasGesture::Draw { from: c, to: c }),
            };
        }
        if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.dragged()) {
            let c = to_canvas(p);
            match &mut editor.canvas.gesture {
                Some(
                    CanvasGesture::Draw { to, .. }
                    | CanvasGesture::MoveSelection { to, .. }
                    | CanvasGesture::Marquee { to, .. }
                    | CanvasGesture::MoveMarquee { to, .. },
                ) => {
                    *to = c;
                }
                Some(CanvasGesture::CutOut { to, .. }) => *to = along(c),
                Some(CanvasGesture::Stroke { points }) => canvas_tools::push_trace(points, c),
                Some(CanvasGesture::Crop { to, .. }) => *to = c,
                Some(CanvasGesture::Resize { local, .. }) => {
                    if let Some((_, f, _)) = &gizmo_node {
                        *local = f.to_local(c);
                    }
                }
                Some(CanvasGesture::Rotate { to, .. }) => *to = c,
                Some(CanvasGesture::CornerRadius { radius, .. }) => {
                    if let Some((_, f, a)) = &gizmo_node
                        && let Some((rect, _)) = corner_radius_of(a)
                    {
                        *radius = gizmo::radius_from_drag(rect, f.to_local(c));
                    }
                }
                Some(CanvasGesture::Padding { padding, .. }) => {
                    if let Some((_, f, a)) = &gizmo_node
                        && let Some((rect, _)) = padding_of(a)
                    {
                        *padding = gizmo::padding_from_drag(rect, f.to_local(c));
                    }
                }
                _ => {}
            }
        }
        if let Some(CanvasGesture::Stroke { points }) = &editor.canvas.gesture {
            draw_trace(&painter, &xf, editor, points);
        }
        if tool == ToolId::Crop
            && let Some(r) = editor.canvas.crop
        {
            let shown = match &editor.canvas.gesture {
                Some(CanvasGesture::Crop {
                    hit,
                    start,
                    from,
                    to,
                }) => canvas_tools::crop_drag(*start, *hit, *from, *to, mods.alt),
                _ => r,
            };
            draw_crop(&painter, &xf, view, shown, t);
            if let Some(p) = resp.hover_pos() {
                let tol = f64::from(CROP_REACH / xf.scale.max(f32::EPSILON));
                ui.ctx()
                    .set_cursor_icon(match canvas_tools::crop_hit(shown, to_canvas(p), tol) {
                        CropHit::Handle(hx, hy) => crop_cursor(hx, hy),
                        CropHit::Inside => CursorIcon::Move,
                        CropHit::Outside => CursorIcon::Crosshair,
                    });
            }
        }
        if !editor.canvas.polygon.is_empty() {
            let mut line: Vec<Pos2> = editor
                .canvas
                .polygon
                .iter()
                .map(|q| xf.to_screen(*q))
                .collect();
            if let Some(h) = resp.hover_pos() {
                line.push(h);
            }
            painter.add(Shape::line(line, stroke(2.0, t.gizmo_stroke)));
            for q in &editor.canvas.polygon {
                painter.circle_filled(xf.to_screen(*q), 3.0, t.handle_fill);
            }
        }
        if tool == ToolId::Eraser
            && let Some(h) = resp.hover_pos()
        {
            let r = editor.tools.eraser.radius * xf.scale;
            painter.circle_stroke(h, r, stroke(1.5, t.gizmo_outline));
            painter.circle_stroke(h, r, stroke(0.75, t.handle_fill));
        }
        if let Some(CanvasGesture::Draw { from, to }) = &editor.canvas.gesture {
            let ellipse = (tool == ToolId::Shape && editor.tools.shape.kind == ShapeKind::Ellipse)
                || (tool == ToolId::Spotlight && editor.tools.spotlight.ellipse);
            draw_preview(&painter, &xf, tool, *from, *to, ellipse, t);
        }
        if tool == ToolId::Selection {
            let shown = match &editor.canvas.gesture {
                Some(CanvasGesture::Marquee { from, to }) => Some(normalized(*from, *to)),
                Some(CanvasGesture::MoveMarquee { from, to }) => {
                    editor.canvas.marquee.map(|mut r| {
                        r.translate(to.x - from.x, to.y - from.y);
                        r
                    })
                }
                _ => editor.canvas.marquee,
            };
            if let Some(r) = shown {
                let sr = xf.rect_to_screen(r);
                painter.rect_stroke(sr, 0.0, stroke(1.0, t.gizmo_outline), StrokeKind::Middle);
                dashed_rect(&painter, sr, t.handle_fill);
                if let Some(b) = capturecraft_engine::commands::canvas_rect_to_base(m, r) {
                    badge(
                        &painter,
                        sr.center_bottom() + vec2(0.0, t.space_lg),
                        &format!("{} × {} px", b.width(), b.height()),
                        t,
                    );
                }
            }
            if let Some(p) = resp.hover_pos()
                && editor
                    .canvas
                    .marquee
                    .is_some_and(|r| inside(r, to_canvas(p)))
            {
                ui.ctx().set_cursor_icon(CursorIcon::Move);
            }
            // Delete is a shortcut (`actions::EditAction::Delete`).
            let escape = ui.input(|i| i.key_pressed(Key::Escape));
            let typing = ui.ctx().memory(|mem| mem.focused().is_some());
            if !typing && escape {
                editor.canvas.marquee = None;
            }
        }
        if resp.drag_stopped() {
            match editor.canvas.gesture.take() {
                Some(CanvasGesture::Draw { from, to }) if tool == ToolId::GrabText => {
                    intents.push(grab_text_intent(m, Some(normalized(from, to))));
                }
                Some(CanvasGesture::Draw { from, to }) if tool == ToolId::Text => {
                    editor.canvas.selection.clear();
                    editor.canvas.text_edit = Some(canvas_tools::begin_new_text(
                        editor,
                        from,
                        Some(normalized(from, to)),
                    ));
                }
                Some(CanvasGesture::Draw { from, to }) => {
                    if let Some(i) = draw_intent(editor, from, to) {
                        intents.push(i);
                    }
                }
                Some(CanvasGesture::Stroke { points }) => {
                    intents.extend(canvas_tools::stroke_intent(editor, &points));
                }
                Some(CanvasGesture::Crop {
                    hit,
                    start,
                    from,
                    to,
                }) => {
                    editor.canvas.crop =
                        Some(canvas_tools::crop_drag(start, hit, from, to, mods.alt));
                }
                Some(CanvasGesture::MoveSelection { from, to }) => {
                    let (dx, dy) = (to.x - from.x, to.y - from.y);
                    if dx != 0.0 || dy != 0.0 {
                        let ids: Vec<u64> = editor.canvas.selection.iter().map(|n| n.0).collect();
                        intents.push(UiIntent::command(
                            "annotation.move",
                            json!({ "ids": ids, "dx": dx, "dy": dy }),
                        ));
                    }
                }
                Some(CanvasGesture::Marquee { from, to }) => {
                    let r = normalized(from, to);
                    editor.canvas.marquee = (r.w >= 2.0 && r.h >= 2.0).then_some(r);
                }
                Some(CanvasGesture::MoveMarquee { from, to }) => {
                    if let Some(i) = move_selection_intent(editor, m, to.x - from.x, to.y - from.y)
                    {
                        intents.push(i);
                        // The pixels are a smart object now; the hole is
                        // not a selection.
                        editor.canvas.marquee = None;
                    }
                }
                Some(CanvasGesture::CutOut { from, to }) if (to - from).abs() >= 1.0 => {
                    intents.push(UiIntent::command(
                        "canvas.cutOut",
                        editor.tools.cut_out.params(from, to),
                    ));
                }
                Some(
                    g @ (CanvasGesture::Resize { .. }
                    | CanvasGesture::Rotate { .. }
                    | CanvasGesture::CornerRadius { .. }
                    | CanvasGesture::Padding { .. }),
                ) => {
                    if let Some(i) = gizmo_node
                        .and_then(|(_, f, a)| finish_gizmo(&g, &f, a, mods.shift, mods.alt))
                    {
                        intents.push(i);
                    }
                }
                _ => {}
            }
        }
        let typing = ui.ctx().text_edit_focused();
        if !typing {
            let (enter, escape) =
                ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if tool == ToolId::Shape && !editor.canvas.polygon.is_empty() {
                if enter {
                    let corners = std::mem::take(&mut editor.canvas.polygon);
                    intents.extend(canvas_tools::polygon_intent(editor, &corners));
                } else if escape {
                    editor.canvas.polygon.clear();
                }
            }
            if tool == ToolId::Crop {
                if enter
                    && let Some(r) = editor.canvas.crop
                    && let Some(i) = canvas_tools::crop_intent(m, r)
                {
                    intents.push(i);
                    editor.canvas.crop = None;
                } else if escape {
                    editor.canvas.crop = None;
                }
            }
        }
        // A node being typed in shows its current font and colour (the
        // toolbar may have just changed them).
        if let Some(e) = &mut editor.canvas.text_edit
            && let Some(id) = e.node
            && let Some(Annotation::Callout(c)) = m.node(id).and_then(|n| n.annotation())
        {
            let run = c.text.runs.first().cloned().unwrap_or_default();
            e.style = Some((run.font, run.color, c.text.align));
        }
        inline_text(ui, &xf, editor, intents, t);
        text_toolbar(ui, &xf, view, m, gizmo_tool, editor, intents, t);
        intents.extend(context_menu::show(ui.ctx(), editor, Some(doc), t));
        if editor.canvas.eyedropper.is_some() {
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                editor.canvas.eyedropper = None;
            } else if let Some(p) = resp.hover_pos() {
                let read = text_format::sampler(ui.ctx(), doc);
                let grid = text_format::loupe_grid(&read, to_canvas(p), text_format::LOUPE_CELLS);
                text_format::eyedropper_loupe(ui.ctx(), p, view, &grid, t);
                // The drop is the pointer: its tip on the sampled pixel.
                let middle = grid.get(grid.len() / 2).copied().flatten();
                text_format::eyedropper_drop(ui.ctx(), p, middle);
            }
        }
        if editor.canvas.eyedropper.is_none() {
            text_format::drop_composite(ui.ctx());
        }
        if resp.hovered() && editor.canvas.eyedropper.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::None);
        } else if resp.hovered() && !matches!(tool, ToolId::Move | ToolId::Selection) {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        if let Some(p) = resp.hover_pos()
            && editor.canvas.eyedropper.is_none()
            && tool != ToolId::Move
        {
            pointer_badge(&painter, p, tool, t);
        }
        resp
    }
}

/// Grab Text over canvas rect `r` (`None`: the whole capture).
pub fn grab_text_intent(m: &capturecraft_format::Manifest, r: Option<RectF>) -> UiIntent {
    let region = r
        .filter(|r| r.w >= 2.0 && r.h >= 2.0)
        .and_then(|r| marquee_base(m, r));
    UiIntent::GrabText { region }
}

/// Reach of the crop frame's handles, points.
const CROP_REACH: f32 = 8.0;

fn crop_cursor(hx: i8, hy: i8) -> CursorIcon {
    match (hx, hy) {
        (0, _) => CursorIcon::ResizeVertical,
        (_, 0) => CursorIcon::ResizeHorizontal,
        (-1, -1) | (1, 1) => CursorIcon::ResizeNwSe,
        _ => CursorIcon::ResizeNeSw,
    }
}

/// The pending crop: everything outside dimmed, the frame with rule-of-
/// thirds guides, eight handles and the size under it.
fn draw_crop(painter: &egui::Painter, xf: &ViewTransform, view: ERect, r: RectF, t: &Tokens) {
    let f = xf.rect_to_screen(r);
    let shade = Color32::from_black_alpha(150);
    for band in [
        ERect::from_min_max(view.min, pos2(view.max.x, f.min.y)),
        ERect::from_min_max(pos2(view.min.x, f.max.y), view.max),
        ERect::from_min_max(pos2(view.min.x, f.min.y), pos2(f.min.x, f.max.y)),
        ERect::from_min_max(pos2(f.max.x, f.min.y), pos2(view.max.x, f.max.y)),
    ] {
        if band.is_positive() {
            painter.rect_filled(band, 0.0, shade);
        }
    }
    let guide = stroke(1.0, Color32::from_white_alpha(110));
    for k in [1.0 / 3.0, 2.0 / 3.0] {
        let x = f.min.x + f.width() * k;
        let y = f.min.y + f.height() * k;
        painter.line_segment([pos2(x, f.min.y), pos2(x, f.max.y)], guide);
        painter.line_segment([pos2(f.min.x, y), pos2(f.max.x, y)], guide);
    }
    painter.rect_stroke(f, 0.0, stroke(1.5, Color32::WHITE), StrokeKind::Middle);
    for hx in [-1i8, 0, 1] {
        for hy in [-1i8, 0, 1] {
            if (hx, hy) == (0, 0) {
                continue;
            }
            let c = pos2(
                f.center().x + f32::from(hx) * f.width() / 2.0,
                f.center().y + f32::from(hy) * f.height() / 2.0,
            );
            let h = ERect::from_center_size(c, Vec2::splat(t.handle_size));
            painter.rect(
                h,
                0.0,
                t.handle_fill,
                stroke(1.0, t.handle_stroke),
                StrokeKind::Inside,
            );
        }
    }
    badge(
        painter,
        f.center_bottom() + vec2(0.0, t.space_lg),
        &format!("{} × {} px", r.w.round(), r.h.round()),
        t,
    );
}

/// The Pen's line in its style, or the Eraser's swath, while dragging.
fn draw_trace(painter: &egui::Painter, xf: &ViewTransform, editor: &EditorState, points: &[Point]) {
    let line: Vec<Pos2> = points.iter().map(|q| xf.to_screen(*q)).collect();
    let s = if editor.toolbar.active == ToolId::Eraser {
        stroke(
            editor.tools.eraser.radius * 2.0 * xf.scale,
            Color32::from_white_alpha(90),
        )
    } else {
        let style = editor
            .sidebar
            .effective_style(ToolId::Pen)
            .unwrap_or_default();
        let k = style.stroke.unwrap_or_default();
        stroke(
            (k.width * xf.scale).max(1.0),
            Color32::from_rgba_unmultiplied(k.color.r, k.color.g, k.color.b, k.color.a),
        )
    };
    if line.len() == 1 {
        painter.circle_filled(line[0], s.width / 2.0, s.color);
    } else {
        painter.add(Shape::line(line, s));
    }
}

/// The floating text toolbar: over the text being typed (a new one's
/// format is kept in the edit until it is added; an existing node changes
/// at once), else over a single selected text.
#[allow(clippy::too_many_arguments)]
fn text_toolbar(
    ui: &Ui,
    xf: &ViewTransform,
    view: ERect,
    m: &capturecraft_format::Manifest,
    gizmo_tool: bool,
    editor: &mut EditorState,
    intents: &mut Vec<UiIntent>,
    t: &Tokens,
) {
    if let Some(e) = editor.canvas.text_edit.clone() {
        let (font, _, _) = canvas_tools::edit_style(editor, &e);
        let fit = if e.bubble {
            e.rect
        } else {
            canvas_tools::fit_text_rect(e.rect, &e.text, font.size)
        };
        let anchor = xf.rect_to_screen(fit).expand(4.0);
        match e.node {
            Some(id) => {
                let Some(Annotation::Callout(c)) = m.node(id).and_then(|n| n.annotation()) else {
                    return;
                };
                let f = text_format::TextFormat::of(c);
                if let Some(edit) =
                    text_format::toolbar(ui.ctx(), &f, Some(id), anchor, view, editor, t)
                {
                    intents.push(text_format::format_intent(id, &edit));
                }
            }
            None => {
                let mut f = e
                    .format
                    .clone()
                    .unwrap_or_else(|| text_format::TextFormat::new_text(&font, Color::BLACK));
                if let Some(edit) =
                    text_format::toolbar(ui.ctx(), &f, None, anchor, view, editor, t)
                    && let Some(e) = &mut editor.canvas.text_edit
                {
                    f.apply(&edit);
                    e.format = Some(f);
                }
            }
        }
        return;
    }
    if gizmo_tool
        && editor.canvas.gesture.is_none()
        && let Some((id, c)) = text_format::selected_text(m, &editor.canvas.selection)
        && let Some(node) = m.node(id).filter(|n| !n.locked)
        && let Some(f) = NodeFrame::of(node)
    {
        let s = f64::from(xf.scale.max(f32::EPSILON));
        let knob = f.to_canvas(rotate_handle_point(f.bounds, f64::from(STEM_PT) / s));
        let anchor = f
            .corners()
            .into_iter()
            .chain([knob])
            .map(|p| ERect::from_center_size(xf.to_screen(p), Vec2::splat(t.handle_size)))
            .reduce(|a, b| a.union(b))
            .unwrap_or(ERect::NOTHING);
        let format = text_format::TextFormat::of(c);
        if let Some(edit) =
            text_format::toolbar(ui.ctx(), &format, Some(id), anchor, view, editor, t)
        {
            intents.push(text_format::format_intent(id, &edit));
        }
    }
}

/// The text being typed, as a text field over its box (a callout's: inside
/// its body) at the canvas zoom. Enter (or a click elsewhere) finishes,
/// Shift+Enter starts a new line, Esc finishes too; the commit is one
/// command.
fn inline_text(
    ui: &mut Ui,
    xf: &ViewTransform,
    editor: &mut EditorState,
    intents: &mut Vec<UiIntent>,
    t: &Tokens,
) {
    let Some(mut e) = editor.canvas.text_edit.clone() else {
        return;
    };
    let (font, color, align) = canvas_tools::edit_style(editor, &e);
    let px = (font.size * xf.scale).clamp(6.0, 200.0);
    // A callout's text stays inside its body; a text box grows to fit.
    let fit = if e.bubble {
        e.rect
    } else {
        canvas_tools::fit_text_rect(e.rect, &e.text, font.size)
    };
    let r = xf.rect_to_screen(fit);
    let id = Id::new("canvas.inline_text");
    let col = Color32::from_rgba_unmultiplied(color.r, color.g, color.b, 255);
    let painter = ui.painter();
    painter.rect_filled(r.expand(2.0), 2.0, Color32::from_white_alpha(225));
    dashed_rect(painter, r.expand(2.0), t.gizmo_stroke);
    // Plain Enter finishes (Shift+Enter is the field's new line); taken
    // before the field sees it.
    let enter = ui.memory(|m| m.has_focus(id))
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
    let out = egui::TextEdit::multiline(&mut e.text)
        .id(id)
        .font(FontId::proportional(px))
        .text_color(col)
        .frame(egui::Frame::NONE)
        .desired_width(r.width())
        .horizontal_align(match align {
            TextAlign::Left | TextAlign::Justify => egui::Align::LEFT,
            TextAlign::Center => egui::Align::Center,
            TextAlign::Right => egui::Align::RIGHT,
        })
        .return_key(Some(egui::KeyboardShortcut::new(
            egui::Modifiers::SHIFT,
            Key::Enter,
        )))
        .hint_text("Type here");
    let resp = ui.put(r, out);
    let first = ui.ctx().data(|d| d.get_temp::<bool>(id)).is_none();
    if first {
        resp.request_focus();
        ui.ctx().data_mut(|d| d.insert_temp(id, true));
    }
    // Using the text toolbar (or its eyedropper) keeps the text open, and
    // the typing goes on in it afterwards.
    let ctx = ui.ctx().clone();
    let engaged = text_format::toolbar_engaged(&ctx) || editor.canvas.eyedropper.is_some();
    let done = enter || (!first && !resp.has_focus() && !engaged);
    if !done
        && !first
        && !resp.has_focus()
        && ctx.memory(|m| m.focused().is_none())
        && !egui::Popup::is_any_open(&ctx)
        && !ctx.input(|i| i.pointer.any_down())
        && editor.canvas.eyedropper.is_none()
    {
        resp.request_focus();
    }
    if done {
        ui.ctx().data_mut(|d| d.remove::<bool>(id));
        resp.surrender_focus();
        intents.extend(canvas_tools::text_commit_intent(editor, &e));
        editor.canvas.text_edit = None;
    } else {
        editor.canvas.text_edit = Some(e);
    }
}

/// Side of the tool badge that follows the pointer, points.
pub const POINTER_BADGE: f32 = 20.0;

/// Where the pointer's tool badge sits: below right of the hotspot, clear
/// of the crosshair's arms.
pub fn pointer_badge_rect(pointer: Pos2) -> ERect {
    ERect::from_min_size(pointer + vec2(12.0, 12.0), Vec2::splat(POINTER_BADGE))
}

/// The active tool's icon on a small chip next to the pointer, so the
/// crosshair says which tool it draws with.
fn pointer_badge(painter: &egui::Painter, pointer: Pos2, tool: ToolId, t: &Tokens) {
    let r = pointer_badge_rect(pointer);
    painter.rect(
        r,
        Tokens::corner(t.radius),
        t.badge_bg,
        stroke(1.0, t.badge_text.gamma_multiply(0.35)),
        StrokeKind::Inside,
    );
    let style = crate::icons::IconStyle {
        fg: t.badge_text,
        accent: t.record,
    };
    crate::icons::paint(
        painter,
        r.shrink(3.0),
        crate::widgets::tool_icon(tool),
        style,
    );
}

/// The command a finished gizmo drag runs, `None` when nothing changed.
pub fn finish_gizmo(
    g: &CanvasGesture,
    f: &NodeFrame,
    a: &Annotation,
    shift: bool,
    alt: bool,
) -> Option<UiIntent> {
    match *g {
        CanvasGesture::Resize { id, handle, local } => {
            let (sent, _) = gizmo::resize_preview(a, f.rotation, handle, local, shift, alt)?;
            let start = a.bounds()?;
            let same = (sent.x - start.x).abs() < 1e-6
                && (sent.y - start.y).abs() < 1e-6
                && (sent.w - start.w).abs() < 1e-6
                && (sent.h - start.h).abs() < 1e-6;
            (!same && sent.w > 0.0 && sent.h > 0.0).then(|| gizmo::resize_intent(id, sent))
        }
        CanvasGesture::Rotate { id, from, to } => {
            let r = gizmo::rotation_from_drag(f.pivot, from, to, f.rotation, shift);
            (r != f.rotation).then(|| gizmo::rotate_intent(id, r))
        }
        CanvasGesture::CornerRadius { id, radius } => {
            let before = corner_radius_of(a).map(|(_, r)| r);
            (before != Some(radius)).then(|| gizmo::radius_intent(id, radius))
        }
        CanvasGesture::Padding { id, padding } => {
            let before = padding_of(a).map(|(_, p)| p);
            (before != Some(padding)).then(|| gizmo::padding_intent(id, padding))
        }
        _ => None,
    }
}

/// The node frame's rotated bounds as a closed outline.
fn frame_outline(painter: &egui::Painter, xf: &ViewTransform, f: &NodeFrame, s: egui::Stroke) {
    let pts: Vec<Pos2> = f.corners().into_iter().map(|p| xf.to_screen(p)).collect();
    painter.add(Shape::closed_line(pts, s));
}

/// A text's selection border: dashed, dark over a light underlay so it
/// shows on any background.
fn dashed_frame(painter: &egui::Painter, xf: &ViewTransform, f: &NodeFrame, t: &Tokens) {
    let mut pts: Vec<Pos2> = f.corners().into_iter().map(|p| xf.to_screen(p)).collect();
    if let Some(&first) = pts.first() {
        pts.push(first);
    }
    painter.add(Shape::line(
        pts.clone(),
        stroke(1.0, t.handle_fill.gamma_multiply(0.8)),
    ));
    painter.extend(Shape::dashed_line(
        &pts,
        stroke(1.0, t.text_handle_stroke),
        4.0,
        3.0,
    ));
}

/// What a gizmo drag shows instead of the document's values.
#[derive(Clone, Copy, Debug, Default)]
struct Previews {
    /// Draw the parameter diamonds (hidden while resizing or moving).
    diamonds: bool,
    radius: Option<f32>,
    padding: Option<f32>,
}

/// The gizmo of a single selection: resize handles, the rotation stem and
/// the corner-radius or padding diamond. Text gets round handles and a
/// green knob (Snagit's text gizmo); everything else square handles.
fn draw_gizmo(
    painter: &egui::Painter,
    xf: &ViewTransform,
    f: &NodeFrame,
    a: &Annotation,
    previews: Previews,
    t: &Tokens,
) {
    let s = f64::from(xf.scale.max(f32::EPSILON));
    let scr = |p: Point| xf.to_screen(f.to_canvas(p));
    let b = f.bounds;
    let top = scr(handle_point(b, ResizeHandle::N));
    let knob = scr(rotate_handle_point(b, f64::from(STEM_PT) / s));
    let half = t.handle_size / 2.0;
    if gizmo::is_text_node(a) {
        painter.line_segment([top, knob], stroke(1.0, t.text_handle_stroke));
        painter.circle(
            knob,
            5.0,
            t.rotate_handle,
            stroke(1.0, t.text_handle_stroke),
        );
        for h in RESIZE_HANDLES {
            painter.circle(
                scr(handle_point(b, h)),
                half + 0.5,
                t.handle_fill,
                stroke(1.0, t.text_handle_stroke),
            );
        }
    } else {
        painter.line_segment([top, knob], stroke(3.0, t.gizmo_outline));
        painter.line_segment([top, knob], stroke(1.5, t.gizmo_stroke));
        painter.circle(knob, 5.5, t.handle_fill, stroke(1.5, t.handle_stroke));
        let (sin, cos) = f.rotation.sin_cos();
        for h in RESIZE_HANDLES {
            let c = scr(handle_point(b, h));
            let pts = [(-half, -half), (half, -half), (half, half), (-half, half)]
                .map(|(x, y)| c + vec2(x * cos - y * sin, x * sin + y * cos))
                .to_vec();
            painter.add(Shape::convex_polygon(
                pts,
                t.handle_fill,
                stroke(1.0, t.handle_stroke),
            ));
        }
    }
    if !previews.diamonds {
        return;
    }
    let inset = f64::from(RADIUS_INSET_PT) / s;
    if let Some((rect, r)) = corner_radius_of(a) {
        let r = previews.radius.unwrap_or(r);
        diamond(
            painter,
            scr(corner_radius_point(rect, r, inset)),
            6.0,
            t.tail_handle,
            t,
        );
    }
    if let Some((rect, p)) = padding_of(a) {
        let p = previews.padding.unwrap_or(p);
        let at = scr(padding_point(rect, p, inset));
        diamond(painter, at, 5.5, t.tail_handle, t);
        if previews.padding.is_some() {
            badge(
                painter,
                at + vec2(0.0, t.space_lg + 8.0),
                &format!("Padding {p:.0} px"),
                t,
            );
        }
    }
}

/// The rounded shadow frame around the canvas and its four margin handles.
#[allow(clippy::too_many_arguments)]
fn frame_margin_handles(
    ui: &mut Ui,
    painter: &egui::Painter,
    xf: &ViewTransform,
    canvas: Size,
    backdrop: &capturecraft_format::CanvasBackdrop,
    margin: f32,
    editor: &mut EditorState,
    intents: &mut Vec<UiIntent>,
    t: &Tokens,
) {
    let shown = match &editor.canvas.gesture {
        Some(CanvasGesture::FrameMargin { side, start, delta }) => {
            gizmo::margin_after_drag(*start, *side, *delta)
        }
        _ => margin,
    };
    let m = f64::from(shown);
    let frame = xf.rect_to_screen(RectF::new(
        -m,
        -m,
        f64::from(canvas.width) + 2.0 * m,
        f64::from(canvas.height) + 2.0 * m,
    ));
    let dragging = matches!(
        editor.canvas.gesture,
        Some(CanvasGesture::FrameMargin { .. })
    );
    dashed_rect(
        painter,
        frame,
        if dragging {
            t.expand_preview
        } else {
            t.canvas_edge
        },
    );
    // Off the edge midpoints, clear of the canvas expansion handles.
    let along = t.expand_handle_len + t.space_lg;
    for side in gizmo::MARGIN_HANDLES {
        let (at, size) = match side {
            ResizeHandle::N => (frame.center_top() + vec2(along, 0.0), vec2(20.0, 6.0)),
            ResizeHandle::S => (frame.center_bottom() + vec2(along, 0.0), vec2(20.0, 6.0)),
            ResizeHandle::W => (frame.left_center() + vec2(0.0, along), vec2(6.0, 20.0)),
            _ => (frame.right_center() + vec2(0.0, along), vec2(6.0, 20.0)),
        };
        let rect = ERect::from_center_size(at, size);
        let h = ui.interact(
            rect.expand(4.0),
            Id::new(("canvas.frame_margin", side)),
            Sense::drag(),
        );
        let hot = h.hovered() || h.dragged();
        painter.rect(
            rect,
            Tokens::corner(t.radius_pill),
            if hot { t.expand_preview } else { t.accent },
            stroke(1.0, t.expand_handle_stroke),
            StrokeKind::Outside,
        );
        if hot {
            ui.ctx().set_cursor_icon(resize_cursor(side));
            h.clone().on_hover_text("Drag to change the frame margin");
        }
        if h.drag_started() {
            editor.canvas.gesture = Some(CanvasGesture::FrameMargin {
                side,
                start: margin,
                delta: [0.0, 0.0],
            });
        }
        if let Some(CanvasGesture::FrameMargin {
            side: s,
            start,
            delta,
        }) = &mut editor.canvas.gesture
            && *s == side
        {
            if h.dragged() {
                let d = h.drag_delta() / xf.scale.max(f32::EPSILON);
                *delta = [delta[0] + d.x, delta[1] + d.y];
            }
            if h.drag_stopped() {
                let new = gizmo::margin_after_drag(*start, side, *delta);
                if new != *start {
                    intents.push(gizmo::backdrop_intent(&gizmo::with_margin(backdrop, new)));
                }
                editor.canvas.gesture = None;
            }
        }
    }
    if dragging {
        badge(
            painter,
            frame.center_bottom() + vec2(0.0, t.space_lg),
            &format!("Margin {shown:.0} px"),
            t,
        );
    }
}

/// The yellow diamond inside the canvas's top-left corner that sets the
/// rounded shadow frame's corner radius.
#[allow(clippy::too_many_arguments)]
fn frame_radius_handle(
    ui: &mut Ui,
    painter: &egui::Painter,
    xf: &ViewTransform,
    canvas: Size,
    backdrop: &capturecraft_format::CanvasBackdrop,
    radius: f32,
    editor: &mut EditorState,
    intents: &mut Vec<UiIntent>,
    t: &Tokens,
) {
    let size = [canvas.width, canvas.height];
    let rect = gizmo::canvas_rect(size);
    let inset = f64::from(RADIUS_INSET_PT / xf.scale.max(f32::EPSILON));
    let shown = match &editor.canvas.gesture {
        Some(CanvasGesture::FrameRadius { radius }) => *radius,
        _ => radius,
    };
    let rest = xf.to_screen(corner_radius_point(rect, radius, inset));
    let h = ui.interact(
        ERect::from_center_size(rest, Vec2::splat(14.0)),
        Id::new("canvas.frame_radius"),
        Sense::drag(),
    );
    diamond(
        painter,
        xf.to_screen(corner_radius_point(rect, shown, inset)),
        if h.hovered() || h.dragged() { 7.5 } else { 6.0 },
        t.tail_handle,
        t,
    );
    if let Some(p) = h.interact_pointer_pos() {
        let r = gizmo::frame_radius_from_drag(size, xf.to_canvas(p));
        if h.dragged() {
            editor.canvas.gesture = Some(CanvasGesture::FrameRadius { radius: r });
            badge(
                painter,
                xf.to_screen(corner_radius_point(rect, r, inset)) + vec2(0.0, t.space_lg + 8.0),
                &format!("Corners {r:.0} px"),
                t,
            );
        }
        if h.drag_stopped() {
            if r != radius {
                intents.push(gizmo::backdrop_intent(&gizmo::with_corner_radius(
                    backdrop, r,
                )));
            }
            editor.canvas.gesture = None;
        }
    }
    h.on_hover_text("Drag to round the frame's corners");
}

fn toggle(sel: &mut Vec<NodeId>, id: NodeId) {
    match sel.iter().position(|n| *n == id) {
        Some(i) => {
            sel.remove(i);
        }
        None => sel.push(id),
    }
}

/// Eyedropper: the composited colour under `c` into its target: a text
/// node by one command, else the style draft, which the sidebar then
/// applies to the selection like any inspector edit. A click off the
/// canvas (or on a transparent pixel) leaves the eyedropper on.
fn pick_color(
    ctx: &egui::Context,
    doc: &DocState,
    c: Point,
    eye: Eyedropper,
    editor: &mut EditorState,
    intents: &mut Vec<UiIntent>,
) {
    let Some([r, g, b, _]) = text_format::sample_doc(ctx, doc, c) else {
        // Off the canvas: keep picking.
        editor.canvas.eyedropper = Some(eye);
        return;
    };
    let color = Color::rgb(r, g, b);
    // The colour of a text being typed that has no node yet.
    if eye.node.is_none()
        && eye.target == ColorTarget::Text
        && let Some(e) = editor.canvas.text_edit.as_mut()
        && e.node.is_none()
    {
        e.format
            .get_or_insert_with(|| text_format::TextFormat::new_text(&Default::default(), color))
            .color = color;
        return;
    }
    if eye.node.is_some() {
        intents.extend(text_format::eyedropper_intent(&doc.manifest, eye, color));
        return;
    }
    let tool = right_sidebar::style_tool(editor, Some(doc));
    editor.sidebar.edit_draft(tool, |p| match eye.target {
        ColorTarget::Fill => p.fill = capturecraft_format::Fill::Solid { color },
        ColorTarget::Stroke => {
            p.stroke.get_or_insert_with(Default::default).color = color;
        }
        ColorTarget::Text => p.text_color = Some(color),
    });
    right_sidebar::mark_style_pending(ctx);
}

fn resize_cursor(h: ResizeHandle) -> CursorIcon {
    match h {
        ResizeHandle::N | ResizeHandle::S => CursorIcon::ResizeVertical,
        ResizeHandle::E | ResizeHandle::W => CursorIcon::ResizeHorizontal,
        ResizeHandle::NE | ResizeHandle::SW => CursorIcon::ResizeNeSw,
        ResizeHandle::NW | ResizeHandle::SE => CursorIcon::ResizeNwSe,
    }
}

/// A 2 × 2 checker texture, repeated by UV (one draw call at any zoom).
fn checker_texture(ctx: &egui::Context, t: &Tokens) -> TextureHandle {
    let id = Id::new(("canvas.checker", t.checker_light, t.checker_dark));
    if let Some(tex) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return tex;
    }
    let (a, b) = (t.checker_light, t.checker_dark);
    let image = ColorImage::new([2, 2], vec![a, b, b, a]);
    let tex = ctx.load_texture("canvas.checker", image, TextureOptions::NEAREST_REPEAT);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

fn diamond(painter: &egui::Painter, c: Pos2, r: f32, fill: Color32, t: &Tokens) {
    let pts = vec![
        c + vec2(0.0, -r),
        c + vec2(r, 0.0),
        c + vec2(0.0, r),
        c + vec2(-r, 0.0),
    ];
    painter.add(Shape::convex_polygon(
        pts,
        fill,
        stroke(1.5, t.gizmo_outline),
    ));
}

fn dashed_rect(painter: &egui::Painter, r: ERect, color: Color32) {
    let pts = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    painter.extend(Shape::dashed_line(&pts, stroke(1.5, color), 6.0, 4.0));
}

fn badge(painter: &egui::Painter, at: Pos2, text: &str, t: &Tokens) {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(12.0), t.badge_text);
    let pad = vec2(t.space_md, t.space_sm);
    let rect = ERect::from_center_size(at, galley.size() + 2.0 * pad);
    painter.rect_filled(rect, Tokens::corner(t.radius_pill), t.badge_bg);
    painter.galley(rect.min + pad, galley, t.badge_text);
}

/// The strip `[a, b]` a Cut Out would remove, edge to edge, with its seams
/// (a zig-zag hint for torn edges).
#[allow(clippy::too_many_arguments)]
fn draw_cut_band(
    painter: &egui::Painter,
    xf: &ViewTransform,
    canvas: Size,
    axis: Axis,
    a: f64,
    b: f64,
    torn: bool,
    t: &Tokens,
) {
    let (w, h) = (f64::from(canvas.width), f64::from(canvas.height));
    let (band, len) = match axis {
        Axis::Horizontal => (RectF::new(0.0, a, w, b - a), w),
        Axis::Vertical => (RectF::new(a, 0.0, b - a, h), h),
    };
    let r = xf.rect_to_screen(band);
    painter.rect_filled(r, 0.0, t.cutout_band);
    let s = stroke(1.5, t.cutout_seam);
    let line = |v: f64| -> Vec<Pos2> {
        let at = |u: f64| match axis {
            Axis::Horizontal => Point::new(u, v),
            Axis::Vertical => Point::new(v, u),
        };
        if !torn {
            return vec![xf.to_screen(at(0.0)), xf.to_screen(at(len))];
        }
        let step = (12.0 / f64::from(xf.scale.max(0.01))).max(1.0);
        let n = (len / step).ceil().min(4096.0) as usize;
        (0..=n)
            .map(|i| {
                let u = (i as f64 * step).min(len);
                let off = if i % 2 == 0 { -3.0 } else { 3.0 } / f64::from(xf.scale.max(0.01));
                let p = at(u);
                xf.to_screen(match axis {
                    Axis::Horizontal => Point::new(p.x, p.y + off),
                    Axis::Vertical => Point::new(p.x + off, p.y),
                })
            })
            .collect()
    };
    for v in [a, b] {
        painter.add(Shape::line(line(v), s));
    }
}

/// Outline of the annotation a drawing drag would create.
fn draw_preview(
    painter: &egui::Painter,
    xf: &ViewTransform,
    tool: ToolId,
    a: Point,
    b: Point,
    ellipse: bool,
    t: &Tokens,
) {
    let (pa, pb) = (xf.to_screen(a), xf.to_screen(b));
    let s = stroke(2.0, t.gizmo_stroke);
    let r = ERect::from_two_pos(pa, pb);
    match tool {
        ToolId::Arrow => painter.arrow(pa, pb - pa, s),
        ToolId::Shape | ToolId::Spotlight if ellipse => {
            painter.add(Shape::ellipse_stroke(r.center(), r.size() / 2.0, s));
        }
        ToolId::Callout
        | ToolId::Shape
        | ToolId::Blur
        | ToolId::Magnify
        | ToolId::Spotlight
        | ToolId::Highlighter
        | ToolId::GrabText => dashed_rect(painter, r, t.gizmo_stroke),
        _ => {}
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::state::BlurOptions;
    use capturecraft_format::{FilterMode, Offset};

    fn view() -> ERect {
        ERect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))
    }

    #[test]
    fn fit_centres_and_never_enlarges() {
        let xf = ViewTransform::new(Zoom::Fit, [0.0, 0.0], view(), Size::new(200, 100), 1.0);
        assert_eq!(xf.scale, 1.0);
        assert_eq!(xf.origin, pos2(400.0, 350.0));
        let xf = ViewTransform::new(Zoom::Fit, [0.0, 0.0], view(), Size::new(4000, 1000), 2.0);
        assert!((xf.scale - (1000.0 - 2.0 * FIT_MARGIN) / 4000.0).abs() < 1e-6);
        let xf = ViewTransform::new(
            Zoom::Percent { value: 100.0 },
            [0.0; 2],
            view(),
            Size::new(10, 10),
            2.0,
        );
        assert_eq!(xf.scale, 0.5, "100 % is one image pixel per physical pixel");
        let p = Point::new(3.0, 7.0);
        let back = xf.to_canvas(xf.to_screen(p));
        assert!((back.x - 3.0).abs() < 1e-4 && (back.y - 7.0).abs() < 1e-4);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let size = Size::new(800, 600);
        let xf = ViewTransform::new(Zoom::Fit, [0.0; 2], view(), size, 1.0);
        let cursor = pos2(300.0, 250.0);
        let before = xf.to_canvas(cursor);
        let (zoom, scroll) = zoom_about(&xf, 2.0, cursor, view(), size, 1.0);
        let after = ViewTransform::new(zoom, scroll, view(), size, 1.0);
        let p = after.to_canvas(cursor);
        assert!((p.x - before.x).abs() < 1e-3 && (p.y - before.y).abs() < 1e-3);
        let (zoom, _) = zoom_about(&after, 1e9, cursor, view(), size, 1.0);
        assert_eq!(zoom, Zoom::Percent { value: MAX_ZOOM });
    }

    #[test]
    fn handles_sit_on_the_corners_and_edge_midpoints() {
        let canvas = ERect::from_min_size(pos2(100.0, 100.0), vec2(400.0, 300.0));
        let t = Tokens::default();
        let rects = expand_handle_rects(canvas, &t);
        let at = |h| rects.iter().find(|(x, _)| *x == h).unwrap().1;
        assert_eq!(at(ResizeHandle::NW).center(), canvas.min);
        assert_eq!(at(ResizeHandle::SE).center(), canvas.max);
        assert_eq!(at(ResizeHandle::N).center(), canvas.center_top());
        assert_eq!(at(ResizeHandle::E).center(), canvas.right_center());
        for (_, r) in rects {
            assert_eq!(r.width(), t.canvas_handle_size);
        }
    }

    #[test]
    fn marquee_moves_and_deletes_run_in_the_engine() {
        use crate::state::SelectionFill;
        let mut s = capturecraft_engine::Session::new();
        let size = Size::new(80, 60);
        s.open(
            capturecraft_format::Manifest::new(size),
            capturecraft_engine::Pixels::new(size, vec![200; 80 * 60 * 4]),
        )
        .unwrap();
        // Healing needs the analyzer; Transparent does not heal.
        let mut e = EditorState::default();
        let m = s.active().unwrap().manifest.clone();
        assert!(delete_selection_intent(&e, &m).is_none());
        e.canvas.marquee = Some(RectF::new(10.0, 10.0, 20.0, 10.0));
        assert!(move_selection_intent(&e, &m, 0.0, 0.0).is_none());
        let Some(UiIntent::Command { id, params }) = move_selection_intent(&e, &m, 30.0, 5.0)
        else {
            unreachable!("a move intent")
        };
        assert_eq!(id, "smart.moveObject");
        assert_eq!(
            params["rect"],
            json!({ "x0": 10, "y0": 10, "x1": 30, "y1": 20 })
        );
        assert_eq!(params["background"], "transparent");
        e.tools.selection.background = SelectionFill::AutoFill;
        let Some(UiIntent::Command { params, .. }) = delete_selection_intent(&e, &m) else {
            unreachable!("a delete intent")
        };
        assert_eq!(params["background"], "heal");
        // Without an analyzer installed the smart commands are disabled,
        // and the document is unchanged.
        assert!(s.run(id, &params).is_err());
    }

    #[test]
    fn handle_drags_become_margins() {
        let d = ResizeDrag {
            handle: ResizeHandle::NW,
            start_size: Size::new(100, 80),
            start_origin: Offset { x: 5, y: 0 },
            delta: [-20.4, -10.0],
        };
        assert_eq!(expand_margins(&d), [20, 10, 0, 0]);
        let p = expand_params(&d, ExpandBackdrop::Auto).unwrap();
        assert_eq!(p["left"], 20);
        assert_eq!(p["background"]["kind"], "auto");
        let d = ResizeDrag {
            handle: ResizeHandle::SE,
            delta: [30.0, -15.0],
            ..d
        };
        assert_eq!(expand_margins(&d), [0, 0, 30, -15]);
        let d = ResizeDrag {
            delta: [0.2, -0.3],
            ..d
        };
        assert!(expand_params(&d, ExpandBackdrop::White).is_none());
    }

    #[test]
    fn drawing_gestures_carry_the_selected_style() {
        let mut e = EditorState::default();
        e.toolbar.active = ToolId::Arrow;
        let UiIntent::Command { id, params } =
            draw_intent(&e, Point::new(0.0, 0.0), Point::new(50.0, 20.0)).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(id, "annotation.addArrow");
        assert_eq!(params["end"], json!({ "x": 50.0, "y": 20.0 }));
        assert_eq!(params["end_head"], "triangle");
        assert!(params.get("stroke").is_some());
        assert!(draw_intent(&e, Point::new(0.0, 0.0), Point::new(1.0, 1.0)).is_none());
        for tool in [ToolId::Shape, ToolId::Highlighter, ToolId::Blur] {
            e.toolbar.active = tool;
            let click = Point::new(5.0, 5.0);
            assert!(draw_intent(&e, click, click).is_none(), "{tool:?}");
        }
        e.toolbar.active = ToolId::Highlighter;
        let UiIntent::Command { id, params } =
            draw_intent(&e, Point::new(40.0, 30.0), Point::new(10.0, 20.0)).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(id, "annotation.addShape");
        assert_eq!(
            params["bounds"],
            json!({ "x": 10.0, "y": 20.0, "w": 30.0, "h": 10.0 })
        );
        assert_eq!(
            (&params["blend"], &params["stroke"]),
            (&json!("multiply"), &Value::Null)
        );
        assert_eq!(params["fill"]["color"], "#ffe60066");
        e.toolbar.active = ToolId::Blur;
        e.tools.blur = BlurOptions::new(FilterMode::Pixelate, 100.0);
        let UiIntent::Command { id, params } =
            draw_intent(&e, Point::new(0.0, 0.0), Point::new(50.0, 20.0)).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(id, "annotation.addBlur");
        assert_eq!(
            params["effect"],
            json!({ "kind": "pixelate", "block_size_px": 48 })
        );

        e.toolbar.active = ToolId::Step;
        let UiIntent::Command { params, .. } =
            draw_intent(&e, Point::new(5.0, 5.0), Point::new(5.0, 5.0)).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(params["numbering"], "decimal");
        assert!(params["fill"].as_str().is_some_and(|c| c.starts_with("#")));

        e.toolbar.active = ToolId::Callout;
        let UiIntent::Command { params, .. } =
            draw_intent(&e, Point::new(10.0, 10.0), Point::new(110.0, 60.0)).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(params["bounds"]["w"], 100.0);
        assert_eq!(params["tails"].as_array().unwrap().len(), 1);
        assert_eq!(params["body"]["kind"], "rounded_rect");
    }

    #[test]
    fn padding_drags_commit_one_command() {
        use capturecraft_engine::node_commands::text_box;
        use capturecraft_format::{AnnotationNode, FontSpec};
        let a = Annotation::Callout(text_box(
            RectF::new(0.0, 0.0, 200.0, 80.0),
            "Hi",
            FontSpec::default(),
            Color::BLACK,
        ));
        let n = AnnotationNode::new(NodeId(4), a.clone());
        let f = NodeFrame::of(&n).unwrap();
        let g = CanvasGesture::Padding {
            id: NodeId(4),
            padding: 16.0,
        };
        let UiIntent::Command { id, params } = finish_gizmo(&g, &f, &a, false, false).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(id, "annotation.setTextFormat");
        assert_eq!(params, json!({ "ids": [4], "padding": 16.0 }));
        // The box's own padding (4 px): nothing to commit.
        let same = CanvasGesture::Padding {
            id: NodeId(4),
            padding: 4.0,
        };
        assert!(finish_gizmo(&same, &f, &a, false, false).is_none());
    }

    #[test]
    fn gizmo_drags_commit_one_command() {
        use capturecraft_format::{AnnotationNode, RegionShape, ShapeNode};
        let a = Annotation::Shape(ShapeNode {
            shape: RegionShape::Rect {
                rect: RectF::new(0.0, 0.0, 100.0, 50.0),
            },
            ..ShapeNode::default()
        });
        let n = AnnotationNode::new(NodeId(3), a.clone());
        let f = NodeFrame::of(&n).unwrap();
        let g = CanvasGesture::Resize {
            id: NodeId(3),
            handle: ResizeHandle::SE,
            local: Point::new(150.0, 75.0),
        };
        let UiIntent::Command { id, params } = finish_gizmo(&g, &f, &a, false, false).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(id, "node.resize");
        assert_eq!(params["bounds"]["w"], 150.0);
        let unchanged = CanvasGesture::Resize {
            id: NodeId(3),
            handle: ResizeHandle::SE,
            local: Point::new(100.0, 50.0),
        };
        assert!(finish_gizmo(&unchanged, &f, &a, false, false).is_none());
        let g = CanvasGesture::Rotate {
            id: NodeId(3),
            from: Point::new(50.0, -10.0),
            to: Point::new(100.0, 25.0),
        };
        let UiIntent::Command { id, params } = finish_gizmo(&g, &f, &a, true, false).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(id, "node.rotate");
        let deg = params["rotation_rad"].as_f64().unwrap().to_degrees();
        assert!((deg - 90.0).abs() < 1e-3, "{deg}");
        let g = CanvasGesture::CornerRadius {
            id: NodeId(3),
            radius: 8.0,
        };
        let UiIntent::Command { id, .. } = finish_gizmo(&g, &f, &a, false, false).unwrap() else {
            unreachable!()
        };
        assert_eq!(id, "annotation.setStyle");
    }

    /// The emitted parameters are accepted by the real commands.
    #[test]
    fn emitted_commands_run_in_the_engine() {
        use capturecraft_engine::{Pixels, Session};
        use capturecraft_format::Manifest;
        let mut s = Session::new();
        let size = Size::new(300, 200);
        s.open(
            Manifest::new(size),
            Pixels::new(size, vec![200; 300 * 200 * 4]),
        )
        .unwrap();
        let mut e = EditorState::default();
        e.tools.shape.kind = ShapeKind::Ellipse;
        for tool in [
            ToolId::Arrow,
            ToolId::Callout,
            ToolId::Step,
            ToolId::Shape,
            ToolId::Highlighter,
            ToolId::Blur,
        ] {
            e.toolbar.active = tool;
            // Every built-in Quick Style of the tool draws.
            let n = e
                .sidebar
                .quick_styles
                .for_tool(tool)
                .map_or(1, |t| t.styles.len());
            for k in 0..n {
                e.sidebar.quick_styles.select(tool, k);
                let i = draw_intent(&e, Point::new(20.0, 20.0), Point::new(120.0, 80.0)).unwrap();
                let out = crate::dispatch(&mut s, vec![i]);
                assert!(
                    matches!(out[0], crate::Dispatched::Ran { .. }),
                    "{tool:?} style {k}: {out:?}"
                );
            }
        }
        let cut = e.tools.cut_out.params(150.0, 120.0);
        let d = ResizeDrag {
            handle: ResizeHandle::SW,
            start_size: size,
            start_origin: Offset::default(),
            delta: [-40.0, 25.0],
        };
        let out = crate::dispatch(
            &mut s,
            vec![
                UiIntent::command("canvas.cutOut", cut),
                UiIntent::command(
                    "canvas.expandBounds",
                    expand_params(&d, ExpandBackdrop::White).unwrap(),
                ),
            ],
        );
        assert!(
            out.iter()
                .all(|o| matches!(o, crate::Dispatched::Ran { .. })),
            "{out:?}"
        );
        assert_eq!(
            s.active().unwrap().manifest.canvas.size,
            Size::new(340, 195)
        );
    }
}
