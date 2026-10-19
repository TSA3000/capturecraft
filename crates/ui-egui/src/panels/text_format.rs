//! Text formatting shared by the floating text toolbar on the canvas and
//! the Properties drawer's Advanced section, and the live eyedropper's
//! sampling of the composited canvas.
//!
//! Neither the toolbar nor the drawer keeps its own copy of a text's
//! format: both read the selected node every frame ([`TextFormat::of`])
//! and send the same command (`annotation.setTextFormat`, one undo step
//! per change), so a change made in one shows in the other and on the
//! canvas on the next frame. Drags (the size field, the colour picker, the
//! sliders) are held until the pointer is released, so a drag is one
//! command too.
//!
//! The toolbar floats in screen space above the selected text box or
//! callout, below it when there is no room: text colour with the
//! eyedropper, font family, size, Bold / Italic / Underline and the
//! horizontal alignment (Left, Centre, Right, Justify).
//!
//! The eyedropper samples the document as exported (the CPU compositor's
//! flattened frame, annotations and frame included), cached until the
//! scene changes, and shows the colour under the pointer as
//! `R:255 G:255 B:255` next to it.

use std::sync::Arc;

use capturecraft_compositor::{CpuCompositor, RenderScene};
use capturecraft_engine::DocState;
use capturecraft_engine::node_commands::BOLD_FROM;
use capturecraft_engine::render::{Flattened, render_scene, straight_rgba8};
use capturecraft_format::{
    Annotation, Callout, Color, Fill, Manifest, NodeId, Stroke, TextAlign, VAlign,
};
use egui::text::{LayoutJob, TextFormat as GlyphFormat};
use egui::{
    Color32, ComboBox, Context, DragValue, FontId, Id, Order, Painter, Pos2, Rect, Sense,
    StrokeKind, Ui, Vec2, pos2, vec2,
};
use photocraft_geom::Point;
use serde_json::{Map, Value, json};

use super::gizmo;
use crate::UiIntent;
use crate::state::{ColorTarget, EditorState, Eyedropper};
use crate::theme::{Tokens, stroke};

/// Side of a toggle button, points.
pub const BUTTON: f32 = 24.0;
/// Space between the toolbar and the node (and the view's edge), points.
pub const TOOLBAR_GAP: f32 = 8.0;
/// The size menu's presets, pixels.
pub const SIZES: [f32; 17] = [
    8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 36.0, 48.0, 64.0, 72.0,
    96.0,
];
/// Families listed until the installed fonts are scanned (the bundled
/// ones, always there).
pub const FALLBACK_FAMILIES: [&str; 2] = ["Inter", "JetBrains Mono"];

pub const ALIGNS: [(TextAlign, &str); 4] = [
    (TextAlign::Left, "Align left"),
    (TextAlign::Center, "Centre"),
    (TextAlign::Right, "Align right"),
    (TextAlign::Justify, "Justify"),
];

pub const VALIGNS: [(VAlign, &str); 3] = [
    (VAlign::Top, "Top"),
    (VAlign::Middle, "Middle"),
    (VAlign::Bottom, "Bottom"),
];

// ---------------------------------------------------------------------------
// The format and its command

/// One change of a text's format.
#[derive(Clone, Debug, PartialEq)]
pub enum TextEdit {
    Family(String),
    Size(f32),
    Bold(bool),
    Italic(bool),
    Underline(bool),
    Color(Color),
    Align(TextAlign),
    VAlign(VAlign),
    Padding(f32),
}

impl TextEdit {
    fn field(&self) -> (&'static str, Value) {
        match self {
            TextEdit::Family(f) => ("family", json!(f)),
            TextEdit::Size(s) => ("size", json!(s)),
            TextEdit::Bold(b) => ("bold", json!(b)),
            TextEdit::Italic(i) => ("italic", json!(i)),
            TextEdit::Underline(u) => ("underline", json!(u)),
            TextEdit::Color(c) => ("color", json!(c)),
            TextEdit::Align(a) => ("align", json!(a)),
            TextEdit::VAlign(v) => ("valign", json!(v)),
            TextEdit::Padding(p) => ("padding", json!(p)),
        }
    }
}

/// `annotation.setTextFormat` changing one attribute of node `id`.
pub fn format_intent(id: NodeId, e: &TextEdit) -> UiIntent {
    let (key, value) = e.field();
    let mut p = Map::new();
    p.insert("ids".into(), json!([id.0]));
    p.insert(key.into(), value);
    UiIntent::command("annotation.setTextFormat", Value::Object(p))
}

/// What the controls show: the first run's font and colour (a text typed
/// on the canvas has one run) and the block's layout.
#[derive(Clone, Debug, PartialEq)]
pub struct TextFormat {
    pub family: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Color,
    pub align: TextAlign,
    pub valign: VAlign,
    pub padding: f32,
}

impl TextFormat {
    pub fn of(c: &Callout) -> Self {
        let run = c.text.runs.first().cloned().unwrap_or_default();
        Self {
            family: run.font.family,
            size: run.font.size,
            bold: run.font.weight >= BOLD_FROM,
            italic: run.font.italic,
            underline: run.underline,
            color: run.color,
            align: c.text.align,
            valign: c.text.valign,
            padding: c.text.padding,
        }
    }
}

/// The selected text box or callout, when exactly one node is selected.
pub fn selected_text<'a>(m: &'a Manifest, selection: &[NodeId]) -> Option<(NodeId, &'a Callout)> {
    let [id] = selection else {
        return None;
    };
    match m.node(*id)?.annotation()? {
        Annotation::Callout(c) => Some((*id, c)),
        _ => None,
    }
}

/// The installed font families (the bundled ones until the background
/// font scan is done).
pub fn families(ctx: &Context) -> Arc<Vec<String>> {
    let id = Id::new("text_format.families");
    if let Some(f) = ctx.data(|d| d.get_temp::<Arc<Vec<String>>>(id)) {
        return f;
    }
    match capturecraft_compositor::text::font_families() {
        Some(f) if !f.is_empty() => {
            let f = Arc::new(f);
            ctx.data_mut(|d| d.insert_temp(id, f.clone()));
            f
        }
        // Not cached: asked again next frame.
        _ => Arc::new(FALLBACK_FAMILIES.map(String::from).to_vec()),
    }
}

// ---------------------------------------------------------------------------
// Controls (each returns the change it made, if any)

pub(crate) fn c32(c: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

pub(crate) fn from_c32(c: Color32) -> Color {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Color::rgba(r, g, b, a)
}

/// Keeps a value edited by a drag in temp data under `id` and gives it
/// back once the pointer is released, so a drag commits once.
pub fn held<T: Clone + Send + Sync + 'static>(ui: &Ui, id: Id, edited: Option<T>) -> Option<T> {
    if let Some(v) = edited {
        ui.data_mut(|d| d.insert_temp(id, v));
    }
    if ui.input(|i| i.pointer.any_down()) {
        return None;
    }
    let v = ui.data(|d| d.get_temp::<T>(id))?;
    ui.data_mut(|d| d.remove::<T>(id));
    Some(v)
}

/// The value a held control shows: the one being dragged, else `current`.
pub fn shown<T: Clone + Send + Sync + 'static>(ui: &Ui, id: Id, current: T) -> T {
    ui.data(|d| d.get_temp::<T>(id)).unwrap_or(current)
}

/// A square toggle with a stable id, its content painted by `paint`;
/// true when clicked.
pub fn toggle(
    ui: &mut Ui,
    id: Id,
    on: bool,
    hover: &str,
    t: &Tokens,
    paint: impl FnOnce(&Painter, Rect, Color32),
) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    crate::widgets::paint_state(ui, rect, &resp, on, t);
    let fg = if on || resp.hovered() {
        t.text
    } else {
        t.text_dim
    };
    paint(ui.painter(), rect.shrink(6.0), fg);
    let clicked = resp.clicked();
    resp.on_hover_text(hover);
    clicked
}

/// A letter in `fmt`'s style (the B / I / U buttons).
fn letter(p: &Painter, r: Rect, text: &str, fmt: GlyphFormat, bold: bool) {
    let color = fmt.color;
    let galley = p.layout_job(LayoutJob::single_section(text.to_string(), fmt));
    let at = r.center() - galley.size() / 2.0;
    if bold {
        // The default fonts have no bold face: overstrike.
        p.galley(at + vec2(0.7, 0.0), galley.clone(), color);
    }
    p.galley(at, galley, color);
}

/// Bold, Italic and Underline.
pub fn style_toggles(ui: &mut Ui, salt: &str, f: &TextFormat, t: &Tokens) -> Option<TextEdit> {
    let font = FontId::proportional(15.0);
    let mut out = None;
    let base = |color| GlyphFormat {
        font_id: font.clone(),
        color,
        ..GlyphFormat::default()
    };
    if toggle(
        ui,
        Id::new(("text_format.bold", salt)),
        f.bold,
        "Bold",
        t,
        |p, r, c| letter(p, r, "B", base(c), true),
    ) {
        out = Some(TextEdit::Bold(!f.bold));
    }
    if toggle(
        ui,
        Id::new(("text_format.italic", salt)),
        f.italic,
        "Italic",
        t,
        |p, r, c| {
            letter(
                p,
                r,
                "I",
                GlyphFormat {
                    italics: true,
                    ..base(c)
                },
                false,
            )
        },
    ) {
        out = Some(TextEdit::Italic(!f.italic));
    }
    if toggle(
        ui,
        Id::new(("text_format.underline", salt)),
        f.underline,
        "Underline",
        t,
        |p, r, c| {
            letter(
                p,
                r,
                "U",
                GlyphFormat {
                    underline: egui::Stroke::new(1.0, c),
                    ..base(c)
                },
                false,
            )
        },
    ) {
        out = Some(TextEdit::Underline(!f.underline));
    }
    out
}

/// Four lines set left, centred, right or justified.
pub fn paint_align(p: &Painter, r: Rect, a: TextAlign, c: Color32) {
    const WIDTHS: [f32; 4] = [1.0, 0.6, 0.85, 0.5];
    for (i, w) in WIDTHS.into_iter().enumerate() {
        let w = match a {
            TextAlign::Justify if i < 3 => 1.0,
            _ => w,
        } * r.width();
        let y = r.top() + i as f32 * r.height() / 3.0;
        let x0 = match a {
            TextAlign::Left | TextAlign::Justify => r.left(),
            TextAlign::Center => r.center().x - w / 2.0,
            TextAlign::Right => r.right() - w,
        };
        p.line_segment([pos2(x0, y), pos2(x0 + w, y)], stroke(1.5, c));
    }
}

/// Two lines at the top, middle or bottom of a box.
pub fn paint_valign(p: &Painter, r: Rect, v: VAlign, c: Color32) {
    for y in [r.top(), r.bottom()] {
        p.line_segment(
            [pos2(r.left(), y), pos2(r.right(), y)],
            stroke(1.0, c.gamma_multiply(0.5)),
        );
    }
    let block = r.height() * 0.3;
    let y0 = match v {
        VAlign::Top => r.top() + 2.5,
        VAlign::Middle => r.center().y - block / 2.0,
        VAlign::Bottom => r.bottom() - 2.5 - block,
    };
    for (y, w) in [(y0, 0.8), (y0 + block, 0.5)] {
        let w = w * r.width();
        let x0 = r.center().x - w / 2.0;
        p.line_segment([pos2(x0, y), pos2(x0 + w, y)], stroke(1.5, c));
    }
}

/// Left, Centre, Right and Justify.
pub fn align_toggles(ui: &mut Ui, salt: &str, current: TextAlign, t: &Tokens) -> Option<TextAlign> {
    let mut out = None;
    for (a, label) in ALIGNS {
        if toggle(
            ui,
            Id::new(("text_format.align", salt, label)),
            current == a,
            label,
            t,
            |p, r, c| paint_align(p, r, a, c),
        ) && current != a
        {
            out = Some(a);
        }
    }
    out
}

/// Top, Middle and Bottom.
pub fn valign_toggles(ui: &mut Ui, salt: &str, current: VAlign, t: &Tokens) -> Option<VAlign> {
    let mut out = None;
    for (v, label) in VALIGNS {
        if toggle(
            ui,
            Id::new(("text_format.valign", salt, label)),
            current == v,
            label,
            t,
            |p, r, c| paint_valign(p, r, v, c),
        ) && current != v
        {
            out = Some(v);
        }
    }
    out
}

/// The font family menu (the current family is listed even when it is
/// not installed).
pub fn family_combo(ui: &mut Ui, salt: &str, current: &str, width: f32) -> Option<String> {
    let families = families(ui.ctx());
    let mut out = None;
    ComboBox::from_id_salt(("text_format.family", salt))
        .selected_text(current)
        .width(width)
        .height(320.0)
        .show_ui(ui, |ui| {
            if !families.iter().any(|f| f == current) {
                let _ = ui.selectable_label(true, current);
            }
            for f in families.iter() {
                if ui.selectable_label(f == current, f).clicked() && f != current {
                    out = Some(f.clone());
                }
            }
        })
        .response
        .on_hover_text("Font");
    out
}

/// The size field (drag or type) and its preset menu.
pub fn size_control(ui: &mut Ui, salt: &str, current: f32) -> Option<f32> {
    let id = Id::new(("text_format.size", salt));
    let mut v = shown(ui, id, current);
    let resp = ui
        .add(
            DragValue::new(&mut v)
                .range(1.0..=1024.0)
                .speed(0.25)
                .max_decimals(1)
                .update_while_editing(false),
        )
        .on_hover_text("Font size, px");
    let dragged = held(ui, id, resp.changed().then_some(v));
    let mut preset = None;
    ComboBox::from_id_salt(("text_format.sizes", salt))
        .selected_text("")
        .width(12.0)
        .show_ui(ui, |ui| {
            for s in SIZES {
                if ui.selectable_label(s == v, format!("{s:.0}")).clicked() {
                    preset = Some(s);
                }
            }
        });
    preset
        .or(dragged)
        .filter(|s| s.is_finite() && (s - current).abs() > f32::EPSILON)
}

/// A colour swatch (held while its picker is dragged); the new colour.
pub fn color_swatch(ui: &mut Ui, id: Id, current: Color, hover: &str) -> Option<Color> {
    let mut c = shown(ui, id, c32(current));
    let changed = egui::color_picker::color_edit_button_srgba(
        ui,
        &mut c,
        egui::color_picker::Alpha::OnlyBlend,
    )
    .on_hover_text(hover)
    .changed();
    held(ui, id, changed.then_some(c))
        .map(from_c32)
        .filter(|n| *n != current)
}

/// The eyedropper toggle: a pipette.
pub fn eyedropper_button(ui: &mut Ui, id: Id, active: bool, t: &Tokens) -> bool {
    toggle(
        ui,
        id,
        active,
        "Pick a colour from the canvas (Esc cancels)",
        t,
        |p, r, c| {
            let tip = r.left_bottom();
            let head = r.right_top() + vec2(-2.5, 2.5);
            p.line_segment([tip, head], stroke(2.0, c));
            p.circle_filled(head, 2.8, c);
            p.line_segment(
                [head + vec2(-4.0, -1.0), head + vec2(1.0, 4.0)],
                stroke(1.5, c),
            );
        },
    )
}

// ---------------------------------------------------------------------------
// The floating toolbar

/// Where the toolbar of `size` goes for a node whose screen bounds (with
/// its rotation knob) are `anchor`: centred above it, below it when the
/// view has no room above, inside the view's top edge when neither fits.
pub fn toolbar_pos(anchor: Rect, size: Vec2, view: Rect, gap: f32) -> Pos2 {
    let left = view.left() + gap;
    let right = (view.right() - gap - size.x).max(left);
    let x = (anchor.center().x - size.x / 2.0).clamp(left, right);
    let above = anchor.top() - gap - size.y;
    let below = anchor.bottom() + gap;
    let y = if above >= view.top() + gap {
        above
    } else if below + size.y <= view.bottom() - gap {
        below
    } else {
        view.top() + gap
    };
    pos2(x, y)
}

/// The toolbar of text node `id` (screen bounds `anchor`) in the canvas
/// `view`.
#[allow(clippy::too_many_arguments)]
pub fn toolbar(
    ctx: &Context,
    id: NodeId,
    c: &Callout,
    anchor: Rect,
    view: Rect,
    editor: &mut EditorState,
    intents: &mut Vec<UiIntent>,
    t: &Tokens,
) {
    let area = Id::new("canvas.text_toolbar");
    // Last frame's size: the first frame guesses.
    let size = ctx
        .data(|d| d.get_temp::<Vec2>(area))
        .unwrap_or(vec2(440.0, BUTTON + 10.0));
    let f = TextFormat::of(c);
    let mut edit = None;
    let out = egui::Area::new(area)
        .order(Order::Foreground)
        .fixed_pos(toolbar_pos(anchor, size, view, TOOLBAR_GAP))
        .constrain(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(t.chrome)
                .stroke(stroke(1.0, t.chrome_border))
                .corner_radius(Tokens::corner(t.radius))
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.horizontal(|ui| {
                        let salt = "toolbar";
                        if let Some(col) = color_swatch(
                            ui,
                            Id::new(("text_format.color", salt)),
                            f.color,
                            "Text colour",
                        ) {
                            edit = Some(TextEdit::Color(col));
                        }
                        let picking = editor.canvas.eyedropper
                            == Some(Eyedropper {
                                target: ColorTarget::Text,
                                node: Some(id),
                            });
                        if eyedropper_button(
                            ui,
                            Id::new(("text_format.eyedropper", salt)),
                            picking,
                            t,
                        ) {
                            editor.canvas.eyedropper = (!picking).then_some(Eyedropper {
                                target: ColorTarget::Text,
                                node: Some(id),
                            });
                        }
                        ui.add_space(4.0);
                        if let Some(fam) = family_combo(ui, salt, &f.family, 120.0) {
                            edit = Some(TextEdit::Family(fam));
                        }
                        if let Some(s) = size_control(ui, salt, f.size) {
                            edit = Some(TextEdit::Size(s));
                        }
                        ui.add_space(4.0);
                        if let Some(e) = style_toggles(ui, salt, &f, t) {
                            edit = Some(e);
                        }
                        ui.add_space(4.0);
                        if let Some(a) = align_toggles(ui, salt, f.align, t) {
                            edit = Some(TextEdit::Align(a));
                        }
                    });
                });
        });
    ctx.data_mut(|d| d.insert_temp(area, out.response.rect.size()));
    if let Some(e) = edit {
        intents.push(format_intent(id, &e));
    }
}

// ---------------------------------------------------------------------------
// Eyedropper

/// The document as flattened for export, with the scene it was rendered
/// from.
pub type Composite = Arc<(RenderScene, Flattened)>;

/// The flattened document the eyedropper samples, rendered once and
/// cached until the scene changes.
pub fn composite(ctx: &Context, doc: &DocState) -> Option<Composite> {
    let id = Id::new("canvas.eyedropper.composite");
    let scene = render_scene(doc);
    if let Some(c) = ctx.data(|d| d.get_temp::<Composite>(id))
        && c.0 == scene
    {
        return Some(c);
    }
    let frame = CpuCompositor::render(&scene).ok()?;
    let flat = Flattened {
        width: frame.width,
        height: frame.height,
        rgba: straight_rgba8(&frame.pixels),
    };
    let c = Arc::new((scene, flat));
    ctx.data_mut(|d| d.insert_temp(id, c.clone()));
    Some(c)
}

/// Frees the eyedropper's cached render.
pub fn drop_composite(ctx: &Context) {
    ctx.data_mut(|d| d.remove::<Composite>(Id::new("canvas.eyedropper.composite")));
}

/// The straight RGBA of `flat` at canvas point `c`; `offset` is the
/// canvas position of the image's top-left (negative inside a frame).
/// `None` off the image.
pub fn sample(flat: &Flattened, offset: f64, c: Point) -> Option<[u8; 4]> {
    let (x, y) = ((c.x - offset).floor(), (c.y - offset).floor());
    if !(x >= 0.0 && y >= 0.0 && x < f64::from(flat.width) && y < f64::from(flat.height)) {
        return None;
    }
    let i = (y as usize * flat.width as usize + x as usize) * 4;
    let px = flat.rgba.get(i..i + 4)?;
    Some([px[0], px[1], px[2], px[3]])
}

/// The colour under the pointer, as the eyedropper's tooltip says it.
pub fn rgb_label(c: [u8; 4]) -> String {
    format!("R:{} G:{} B:{}", c[0], c[1], c[2])
}

/// The colour the eyedropper takes at `c`: the composited canvas, else
/// the capture pixels; `None` off the canvas or on a transparent pixel.
pub fn sample_doc(ctx: &Context, doc: &DocState, c: Point) -> Option<[u8; 4]> {
    if let Some(comp) = composite(ctx, doc) {
        let canvas = doc.manifest.canvas.size;
        let off = gizmo::image_offset(
            &doc.manifest.canvas.backdrop,
            [canvas.width, canvas.height],
            [comp.1.width, comp.1.height],
        );
        return sample(&comp.1, off, c).filter(|p| p[3] > 0);
    }
    let px = doc.display_pixels()?;
    let o = doc.manifest.canvas.base_origin;
    let (x, y) = (
        (c.x.floor() as i64) - i64::from(o.x),
        (c.y.floor() as i64) - i64::from(o.y),
    );
    let s = px.size();
    if x < 0 || y < 0 || x >= i64::from(s.width) || y >= i64::from(s.height) {
        return None;
    }
    Some(px.pixel(x as i32, y as i32)).filter(|p| p[3] > 0)
}

/// The command that gives text node `eye.node` the sampled `color`;
/// `None` for a draft target or a node that is not text.
pub fn eyedropper_intent(m: &Manifest, eye: Eyedropper, color: Color) -> Option<UiIntent> {
    let id = eye.node?;
    let Annotation::Callout(c) = m.node(id)?.annotation()? else {
        return None;
    };
    Some(match eye.target {
        ColorTarget::Text => format_intent(id, &TextEdit::Color(color)),
        ColorTarget::Fill => UiIntent::command(
            "annotation.setStyle",
            json!({ "ids": [id.0], "fill": Fill::Solid { color } }),
        ),
        ColorTarget::Stroke => UiIntent::command(
            "annotation.setStyle",
            json!({ "ids": [id.0], "stroke": Stroke { color, ..c.stroke.unwrap_or_default() } }),
        ),
    })
}

/// The eyedropper's tooltip: a swatch and the RGB values, below right of
/// the pointer (flipped to stay inside `view`).
pub fn eyedropper_tooltip(painter: &Painter, pointer: Pos2, view: Rect, c: [u8; 4], t: &Tokens) {
    let text = rgb_label(c);
    let galley = painter.layout_no_wrap(text, FontId::monospace(12.0), t.badge_text);
    let swatch = 14.0;
    let pad = vec2(t.space_md, t.space_sm);
    let size = vec2(
        swatch + t.space_sm + galley.size().x,
        galley.size().y.max(swatch),
    ) + 2.0 * pad;
    let mut at = pointer + vec2(16.0, 16.0);
    if at.x + size.x > view.right() {
        at.x = pointer.x - 16.0 - size.x;
    }
    if at.y + size.y > view.bottom() {
        at.y = pointer.y - 16.0 - size.y;
    }
    let r = Rect::from_min_size(at, size);
    painter.rect(
        r,
        Tokens::corner(t.radius),
        t.badge_bg,
        stroke(1.0, t.badge_text.gamma_multiply(0.35)),
        StrokeKind::Inside,
    );
    let sw = Rect::from_min_size(
        pos2(r.left() + pad.x, r.center().y - swatch / 2.0),
        Vec2::splat(swatch),
    );
    painter.rect(
        sw,
        Tokens::corner(t.radius_sm),
        Color32::from_rgb(c[0], c[1], c[2]),
        stroke(1.0, t.badge_text),
        StrokeKind::Inside,
    );
    painter.galley(
        pos2(
            sw.right() + t.space_sm,
            r.center().y - galley.size().y / 2.0,
        ),
        galley,
        t.badge_text,
    );
    // A ring round the hotspot in the sampled colour.
    painter.circle(
        pointer,
        6.0,
        Color32::TRANSPARENT,
        stroke(3.0, Color32::from_rgb(c[0], c[1], c[2])),
    );
    painter.circle_stroke(pointer, 7.5, stroke(1.0, t.gizmo_outline));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use capturecraft_engine::node_commands::text_box;
    use capturecraft_format::FontSpec;
    use photocraft_geom::Size;

    fn params(i: UiIntent) -> (&'static str, Value) {
        let UiIntent::Command { id, params } = i else {
            panic!()
        };
        (id, params)
    }

    #[test]
    fn each_edit_sends_one_field() {
        let id = NodeId(5);
        for (e, key, value) in [
            (TextEdit::Bold(true), "bold", json!(true)),
            (TextEdit::Italic(false), "italic", json!(false)),
            (TextEdit::Underline(true), "underline", json!(true)),
            (TextEdit::Size(18.0), "size", json!(18.0)),
            (TextEdit::Family("Inter".into()), "family", json!("Inter")),
            (
                TextEdit::Align(TextAlign::Justify),
                "align",
                json!("justify"),
            ),
            (TextEdit::VAlign(VAlign::Bottom), "valign", json!("bottom")),
            (TextEdit::Padding(6.0), "padding", json!(6.0)),
            (
                TextEdit::Color(Color::rgb(1, 2, 3)),
                "color",
                json!(Color::rgb(1, 2, 3)),
            ),
        ] {
            let (cmd, p) = params(format_intent(id, &e));
            assert_eq!(cmd, "annotation.setTextFormat");
            assert_eq!(p, json!({ "ids": [5], key: value }), "{e:?}");
        }
    }

    #[test]
    fn the_format_reads_the_first_run() {
        let mut c = text_box(
            capturecraft_format::RectF::new(0.0, 0.0, 100.0, 40.0),
            "Hi",
            FontSpec {
                family: "Inter".into(),
                size: 20.0,
                weight: 700,
                italic: true,
            },
            Color::rgb(255, 0, 0),
        );
        c.text.runs[0].underline = true;
        let f = TextFormat::of(&c);
        assert!(f.bold && f.italic && f.underline);
        assert_eq!((f.size, f.color), (20.0, Color::rgb(255, 0, 0)));
        assert_eq!((f.align, f.valign), (TextAlign::Left, VAlign::Top));
        c.text.runs.clear();
        assert_eq!(TextFormat::of(&c).size, FontSpec::default().size);
    }

    #[test]
    fn the_toolbar_sits_above_else_below_inside_the_view() {
        let view = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let size = vec2(400.0, 30.0);
        let node = Rect::from_min_size(pos2(300.0, 200.0), vec2(200.0, 50.0));
        assert_eq!(toolbar_pos(node, size, view, 8.0), pos2(200.0, 162.0));
        // No room above: below.
        let top = Rect::from_min_size(pos2(300.0, 20.0), vec2(200.0, 50.0));
        assert_eq!(toolbar_pos(top, size, view, 8.0), pos2(200.0, 78.0));
        // Neither: pinned inside the top edge; never off the left or right.
        let tall = Rect::from_min_size(pos2(-50.0, 10.0), vec2(100.0, 580.0));
        assert_eq!(toolbar_pos(tall, size, view, 8.0), pos2(8.0, 8.0));
        let right = Rect::from_min_size(pos2(780.0, 300.0), vec2(100.0, 50.0));
        assert_eq!(toolbar_pos(right, size, view, 8.0).x, 392.0);
    }

    #[test]
    fn samples_come_from_the_flattened_frame() {
        let flat = Flattened {
            width: 2,
            height: 2,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 0],
        };
        assert_eq!(
            sample(&flat, 0.0, Point::new(1.5, 0.2)),
            Some([4, 5, 6, 255])
        );
        // A framed render starts `margin` before the canvas.
        assert_eq!(
            sample(&flat, -1.0, Point::new(0.0, 0.0)),
            Some([10, 11, 12, 0])
        );
        assert_eq!(sample(&flat, 0.0, Point::new(-0.1, 0.0)), None);
        assert_eq!(sample(&flat, 0.0, Point::new(2.0, 0.0)), None);
        assert_eq!(rgb_label([255, 0, 128, 255]), "R:255 G:0 B:128");
    }

    #[test]
    fn the_eyedropper_sees_annotations() {
        let mut s = capturecraft_engine::Session::new();
        let size = Size::new(40, 30);
        s.open(
            Manifest::new(size),
            capturecraft_engine::Pixels::new(size, vec![255; 40 * 30 * 4]),
        )
        .unwrap();
        s.run(
            "annotation.addShape",
            &json!({ "bounds": { "x": 0, "y": 0, "w": 20, "h": 20 },
                     "fill": { "kind": "solid", "color": "#0000ff" } }),
        )
        .unwrap();
        let ctx = Context::default();
        let doc = s.active().unwrap();
        let inside = sample_doc(&ctx, doc, Point::new(10.0, 10.0)).unwrap();
        assert_eq!(&inside[..3], &[0, 0, 255]);
        let outside = sample_doc(&ctx, doc, Point::new(35.0, 25.0)).unwrap();
        assert_eq!(&outside[..3], &[255, 255, 255]);
        assert_eq!(sample_doc(&ctx, doc, Point::new(-3.0, 5.0)), None);
    }

    #[test]
    fn picked_colours_go_to_the_targeted_text() {
        let mut m = Manifest::new(Size::new(100, 100));
        let c = text_box(
            capturecraft_format::RectF::new(0.0, 0.0, 50.0, 20.0),
            "Hi",
            FontSpec::default(),
            Color::BLACK,
        );
        m.annotations.push(capturecraft_format::AnnotationNode::new(
            NodeId(1),
            Annotation::Callout(c),
        ));
        let red = Color::rgb(255, 0, 0);
        let eye = |target| Eyedropper {
            target,
            node: Some(NodeId(1)),
        };
        let (cmd, p) = params(eyedropper_intent(&m, eye(ColorTarget::Text), red).unwrap());
        assert_eq!(
            (cmd, &p["color"]),
            ("annotation.setTextFormat", &json!(red))
        );
        let (cmd, p) = params(eyedropper_intent(&m, eye(ColorTarget::Fill), red).unwrap());
        assert_eq!(
            (cmd, &p["fill"]["color"]),
            ("annotation.setStyle", &json!(red))
        );
        let (_, p) = params(eyedropper_intent(&m, eye(ColorTarget::Stroke), red).unwrap());
        assert_eq!(p["stroke"]["color"], json!(red));
        // The draft is the sidebar's business.
        let draft = Eyedropper {
            target: ColorTarget::Text,
            node: None,
        };
        assert!(eyedropper_intent(&m, draft, red).is_none());
    }
}
