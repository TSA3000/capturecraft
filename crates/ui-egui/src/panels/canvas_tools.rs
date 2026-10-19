//! The geometry and commands behind the canvas tools that are more than a
//! single drag: the Crop frame (PhotoCraft's `crop_ui`: handles, move,
//! redraw outside; Enter applies, Esc cancels), Pen and Eraser traces,
//! Shape polygons and inline text. Pure functions, tested without a
//! display; `canvas_viewport` wires them to the pointer.

use capturecraft_engine::node_commands::is_text_box;
use capturecraft_format::{Annotation, FontSpec, Manifest, NodeId, RectF, TextAlign};
use photocraft_geom::Point;
use serde_json::{Map, Value, json};

use crate::UiIntent;
use crate::state::{CropHit, EditorState, TextEditState, ToolId};

/// Smallest crop frame, canvas pixels.
pub const MIN_CROP: f64 = 1.0;

/// What the pointer at `p` grabs on crop frame `r`; `tol` is the handle
/// reach in canvas pixels (smaller on tiny frames, as in PhotoCraft).
pub fn crop_hit(r: RectF, p: Point, tol: f64) -> CropHit {
    let tol = tol.min(r.w / 3.0).min(r.h / 3.0).max(1.0);
    let near = |v: f64, lo: f64, hi: f64| -> Option<i8> {
        if (v - lo).abs() <= tol {
            Some(-1)
        } else if (v - hi).abs() <= tol {
            Some(1)
        } else if v > lo && v < hi {
            Some(0)
        } else {
            None
        }
    };
    match (near(p.x, r.x, r.x + r.w), near(p.y, r.y, r.y + r.h)) {
        (Some(0), Some(0)) => CropHit::Inside,
        (Some(hx), Some(hy)) => CropHit::Handle(hx, hy),
        _ => CropHit::Outside,
    }
}

/// The frame after dragging `hit` of `start` from `from` to `to`; `alt`
/// resizes about the centre. Whole pixels, never smaller than
/// [`MIN_CROP`].
pub fn crop_drag(start: RectF, hit: CropHit, from: Point, to: Point, alt: bool) -> RectF {
    let r = crop_drag_exact(start, hit, from, to, alt);
    let (x0, y0) = (r.x.round(), r.y.round());
    let (x1, y1) = ((r.x + r.w).round(), (r.y + r.h).round());
    RectF::new(x0, y0, (x1 - x0).max(MIN_CROP), (y1 - y0).max(MIN_CROP))
}

fn crop_drag_exact(start: RectF, hit: CropHit, from: Point, to: Point, alt: bool) -> RectF {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let (mut x0, mut y0, mut x1, mut y1) = (start.x, start.y, start.x + start.w, start.y + start.h);
    match hit {
        CropHit::Outside => return RectF::from_corners(from, to),
        CropHit::Inside => return RectF::new(start.x + dx, start.y + dy, start.w, start.h),
        CropHit::Handle(hx, hy) => {
            let axis = |lo: &mut f64, hi: &mut f64, h: i8, d: f64| match h {
                -1 => {
                    *lo += d;
                    if alt {
                        *hi -= d;
                    }
                }
                1 => {
                    *hi += d;
                    if alt {
                        *lo -= d;
                    }
                }
                _ => {}
            };
            axis(&mut x0, &mut x1, hx, dx);
            axis(&mut y0, &mut y1, hy, dy);
        }
    }
    let r = RectF::from_corners(Point::new(x0, y0), Point::new(x1, y1));
    RectF::new(r.x, r.y, r.w.max(MIN_CROP), r.h.max(MIN_CROP))
}

/// `canvas.crop` for `r`, `None` when it keeps the whole canvas.
pub fn crop_intent(m: &Manifest, r: RectF) -> Option<UiIntent> {
    let c = m.canvas.size;
    let whole = r.x.round() == 0.0
        && r.y.round() == 0.0
        && (r.w - f64::from(c.width)).abs() < 0.5
        && (r.h - f64::from(c.height)).abs() < 0.5;
    (!whole && r.w >= MIN_CROP && r.h >= MIN_CROP).then(|| {
        UiIntent::command(
            "canvas.crop",
            json!({ "rect": { "x": r.x, "y": r.y, "w": r.w, "h": r.h } }),
        )
    })
}

fn pts(points: &[Point]) -> Value {
    Value::Array(
        points
            .iter()
            .map(|p| json!({ "x": p.x, "y": p.y }))
            .collect(),
    )
}

/// The Pen's `annotation.addPen` or the Eraser's `canvas.erase` for a
/// finished trace.
pub fn stroke_intent(editor: &EditorState, points: &[Point]) -> Option<UiIntent> {
    let tool = editor.toolbar.active;
    match tool {
        ToolId::Pen if points.len() >= 2 => {
            let style = editor.sidebar.effective_style(tool).unwrap_or_default();
            let mut p = Map::new();
            p.insert("points".into(), pts(points));
            if let Some(s) = &style.stroke {
                p.insert("stroke".into(), json!(s));
            }
            Some(UiIntent::command("annotation.addPen", Value::Object(p)))
        }
        ToolId::Eraser if !points.is_empty() => {
            let o = editor.tools.eraser;
            Some(UiIntent::command(
                "canvas.erase",
                json!({ "points": pts(points), "radius": o.radius, "hardness": o.hardness }),
            ))
        }
        _ => None,
    }
}

/// Adds the next trace point unless it is within half a pixel of the last.
pub fn push_trace(points: &mut Vec<Point>, p: Point) {
    if points
        .last()
        .is_none_or(|q| (q.x - p.x).hypot(q.y - p.y) >= 0.5)
    {
        points.push(p);
    }
}

/// `annotation.addShape` for the Shape tool's polygon, in its style.
pub fn polygon_intent(editor: &EditorState, corners: &[Point]) -> Option<UiIntent> {
    if corners.len() < 3 {
        return None;
    }
    let style = editor
        .sidebar
        .effective_style(ToolId::Shape)
        .unwrap_or_default();
    let mut p: Map<String, Value> = style
        .style_params()
        .into_iter()
        .filter(|(k, _)| matches!(k.as_str(), "fill" | "stroke" | "shadow"))
        .collect();
    if style.stroke.is_none() {
        p.insert("stroke".into(), Value::Null);
    }
    p.insert("points".into(), pts(corners));
    Some(UiIntent::command("annotation.addShape", Value::Object(p)))
}

/// The font and colour new text gets from the Text tool's Quick Style.
pub fn text_style(editor: &EditorState) -> (FontSpec, capturecraft_format::Color) {
    let style = editor
        .sidebar
        .effective_style(ToolId::Text)
        .unwrap_or_default();
    let font = style.font.unwrap_or(FontSpec {
        size: 24.0,
        ..FontSpec::default()
    });
    (
        font,
        style
            .text_color
            .unwrap_or(capturecraft_format::Color::BLACK),
    )
}

/// Starts typing a new text box at `rect` (a click gives a one-line box).
pub fn begin_new_text(editor: &EditorState, at: Point, rect: Option<RectF>) -> TextEditState {
    let (font, _) = text_style(editor);
    let line = f64::from(font.size) * 1.5 + 8.0;
    let rect = rect
        .filter(|r| r.w >= 8.0 && r.h >= 8.0)
        .unwrap_or(RectF::new(at.x, at.y - line / 2.0, 240.0, line));
    TextEditState {
        node: None,
        rect,
        text: String::new(),
        original: String::new(),
        style: None,
        bubble: false,
    }
}

/// Starts editing the text of node `id`, if it has any: a text box in
/// its box, a callout inside its body (inset by the text padding), both
/// in the node's own font and colour.
pub fn begin_edit_text(m: &Manifest, id: NodeId) -> Option<TextEditState> {
    let n = m.node(id)?;
    if n.locked {
        return None;
    }
    match n.annotation()? {
        Annotation::Callout(c) => {
            let text = c.text.text();
            let run = c.text.runs.first().cloned().unwrap_or_default();
            let bubble = !is_text_box(c);
            let rect = if bubble {
                let pad = f64::from(c.text.padding)
                    .min(c.bounds.w / 3.0)
                    .min(c.bounds.h / 3.0);
                RectF::new(
                    c.bounds.x + pad,
                    c.bounds.y + pad,
                    c.bounds.w - 2.0 * pad,
                    c.bounds.h - 2.0 * pad,
                )
            } else {
                c.bounds
            };
            Some(TextEditState {
                node: Some(id),
                rect,
                text: text.clone(),
                original: text,
                style: Some((run.font, run.color, c.text.align)),
                bubble,
            })
        }
        _ => None,
    }
}

/// The topmost text box or callout under `p` (double-click to edit its
/// text; the Text tool clicks into it instead of making a new box).
pub fn text_box_at(m: &Manifest, p: Point) -> Option<NodeId> {
    m.annotations
        .iter()
        .rev()
        .filter(|n| n.visible && !n.locked)
        .find(|n| match n.annotation() {
            Some(Annotation::Callout(c)) => {
                let b = c.bounds;
                p.x >= b.x && p.x <= b.x + b.w && p.y >= b.y && p.y <= b.y + b.h
            }
            _ => false,
        })
        .map(|n| n.id)
}

/// The font, colour and alignment an inline edit types in.
pub fn edit_style(
    editor: &EditorState,
    e: &TextEditState,
) -> (FontSpec, capturecraft_format::Color, TextAlign) {
    e.style.clone().unwrap_or_else(|| {
        let (font, color) = text_style(editor);
        (font, color, TextAlign::Left)
    })
}

/// The box a new text needs: at least `rect`, grown to the text's lines.
pub fn fit_text_rect(rect: RectF, text: &str, font_px: f32) -> RectF {
    let size = f64::from(font_px.max(1.0));
    let lines = text.lines().count().max(1) as f64;
    let longest = text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f64;
    RectF::new(
        rect.x,
        rect.y,
        rect.w.max(longest * size * 0.6 + 16.0),
        rect.h.max(lines * size * 1.3 + 8.0),
    )
}

/// What committing an inline edit runs: `annotation.addText` for a new
/// box (nothing when left empty), `annotation.setText` for a changed one
/// or a callout, `annotation.delete` for a text box emptied.
pub fn text_commit_intent(editor: &EditorState, e: &TextEditState) -> Option<UiIntent> {
    let text = e.text.trim_end_matches('\n');
    match e.node {
        None if text.trim().is_empty() => None,
        None => {
            let (font, color) = text_style(editor);
            let r = fit_text_rect(e.rect, text, font.size);
            let style = editor
                .sidebar
                .effective_style(ToolId::Text)
                .unwrap_or_default();
            let mut p = json!({
                "bounds": { "x": r.x, "y": r.y, "w": r.w, "h": r.h },
                "text": text,
                "font": font,
                "color": color,
            });
            if let Some(s) = style.shadow {
                p["shadow"] = json!(s);
            }
            // A filled or outlined text box (the Text tool's Fill and
            // Outline).
            if !matches!(style.fill, capturecraft_format::Fill::None) {
                p["fill"] = json!(style.fill);
            }
            if let Some(s) = style.stroke {
                p["stroke"] = json!(s);
            }
            Some(UiIntent::command("annotation.addText", p))
        }
        Some(_) if text == e.original => None,
        Some(id) if e.bubble => Some(UiIntent::command(
            "annotation.setText",
            json!({ "id": id.0, "text": text.trim_end() }),
        )),
        Some(id) if text.trim().is_empty() => Some(UiIntent::command(
            "annotation.delete",
            json!({ "ids": [id.0] }),
        )),
        Some(id) => Some(UiIntent::command(
            "annotation.setText",
            json!({ "id": id.0, "text": text }),
        )),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use capturecraft_engine::{Pixels, Session};
    use photocraft_geom::Size;

    fn session() -> Session {
        let mut s = Session::new();
        let size = Size::new(300, 200);
        s.open(
            Manifest::new(size),
            Pixels::new(size, vec![255; 300 * 200 * 4]),
        )
        .unwrap();
        s
    }

    fn run(s: &mut Session, i: Option<UiIntent>) {
        let out = crate::dispatch(s, i.into_iter().collect());
        assert!(matches!(out[0], crate::Dispatched::Ran { .. }), "{out:?}");
    }

    #[test]
    fn crop_handles_move_resize_and_redraw() {
        let r = RectF::new(10.0, 10.0, 100.0, 60.0);
        assert_eq!(
            crop_hit(r, Point::new(10.0, 10.0), 6.0),
            CropHit::Handle(-1, -1)
        );
        assert_eq!(
            crop_hit(r, Point::new(60.0, 70.0), 6.0),
            CropHit::Handle(0, 1)
        );
        assert_eq!(crop_hit(r, Point::new(60.0, 40.0), 6.0), CropHit::Inside);
        assert_eq!(crop_hit(r, Point::new(200.0, 40.0), 6.0), CropHit::Outside);
        let from = Point::new(110.0, 70.0);
        let to = Point::new(130.0, 80.0);
        assert_eq!(
            crop_drag(r, CropHit::Handle(1, 1), from, to, false),
            RectF::new(10.0, 10.0, 120.0, 70.0)
        );
        assert_eq!(
            crop_drag(r, CropHit::Handle(1, 0), from, to, true),
            RectF::new(-10.0, 10.0, 140.0, 60.0)
        );
        assert_eq!(
            crop_drag(r, CropHit::Inside, from, to, false),
            RectF::new(30.0, 20.0, 100.0, 60.0)
        );
        assert_eq!(
            crop_drag(r, CropHit::Outside, to, from, false),
            RectF::new(110.0, 70.0, 20.0, 10.0)
        );
        // Dragging an edge past the other flips, never below 1 px.
        let flat = crop_drag(
            r,
            CropHit::Handle(0, 1),
            from,
            Point::new(110.0, 10.0),
            false,
        );
        assert_eq!(flat.h, MIN_CROP);
    }

    #[test]
    fn tool_intents_run_in_the_engine() {
        let mut s = session();
        let mut e = EditorState::default();
        let m = s.active().unwrap().manifest.clone();
        assert!(crop_intent(&m, RectF::new(0.0, 0.0, 300.0, 200.0)).is_none());
        run(
            &mut s,
            crop_intent(&m, RectF::new(20.0, 10.0, 200.0, 150.0)),
        );
        assert_eq!(
            s.active().unwrap().manifest.canvas.size,
            Size::new(200, 150)
        );

        let trace: Vec<Point> = (0..40)
            .map(|i| Point::new(10.0 + f64::from(i) * 3.0, 50.0 + f64::from(i % 5)))
            .collect();
        e.toolbar.active = ToolId::Pen;
        run(&mut s, stroke_intent(&e, &trace));
        e.toolbar.active = ToolId::Eraser;
        run(&mut s, stroke_intent(&e, &trace[..1]));
        let corners = [
            Point::new(10.0, 10.0),
            Point::new(80.0, 20.0),
            Point::new(40.0, 90.0),
        ];
        assert!(polygon_intent(&e, &corners[..2]).is_none());
        run(&mut s, polygon_intent(&e, &corners));

        // Text: a new box, then editing, then emptying it.
        let new = TextEditState {
            text: "Hello\nthere".into(),
            ..begin_new_text(&e, Point::new(20.0, 20.0), None)
        };
        run(&mut s, text_commit_intent(&e, &new));
        let m = &s.active().unwrap().manifest;
        let id = text_box_at(m, Point::new(30.0, 25.0)).unwrap();
        let mut edit = begin_edit_text(m, id).unwrap();
        assert_eq!(edit.text, "Hello\nthere");
        assert!(text_commit_intent(&e, &edit).is_none(), "unchanged");
        edit.text = "Bye".into();
        run(&mut s, text_commit_intent(&e, &edit));
        edit.original = "Bye".into();
        edit.text = "  ".into();
        run(&mut s, text_commit_intent(&e, &edit));
        assert!(s.active().unwrap().manifest.node(id).is_none());
        assert!(text_commit_intent(&e, &begin_new_text(&e, Point::new(1.0, 1.0), None)).is_none());
    }

    #[test]
    fn callouts_are_typed_into_inside_their_body_and_survive_emptying() {
        let mut s = Session::new();
        let size = Size::new(400, 300);
        s.open(
            Manifest::new(size),
            Pixels::new(size, vec![255; 400 * 300 * 4]),
        )
        .unwrap();
        let out = s
            .run(
                "annotation.addCallout",
                &json!({ "bounds": { "x": 20, "y": 20, "w": 200, "h": 100 },
                    "font": { "family": "Inter", "size": 30 }, "text_color": "#0000ff" }),
            )
            .unwrap();
        let id = NodeId(out["id"].as_u64().unwrap());
        let e = EditorState::default();
        let m = &s.active().unwrap().manifest;
        assert_eq!(text_box_at(m, Point::new(100.0, 60.0)), Some(id));
        let mut edit = begin_edit_text(m, id).unwrap();
        assert!(edit.bubble);
        assert_eq!(edit.rect, RectF::new(30.0, 30.0, 180.0, 80.0));
        let (font, color, align) = edit_style(&e, &edit);
        assert_eq!(
            (font.size, color),
            (30.0, capturecraft_format::Color::rgba(0, 0, 255, 255))
        );
        assert_eq!(align, TextAlign::Center);
        edit.text = "Hi\n".into();
        run(&mut s, text_commit_intent(&e, &edit));
        edit.original = "Hi".into();
        edit.text = String::new();
        run(&mut s, text_commit_intent(&e, &edit));
        let n = s.active().unwrap().manifest.node(id).unwrap();
        assert!(matches!(n.annotation(), Some(Annotation::Callout(c)) if c.text.text().is_empty()));
    }

    #[test]
    fn traces_skip_tiny_moves_and_text_boxes_grow() {
        let mut t = Vec::new();
        for p in [
            Point::new(0.0, 0.0),
            Point::new(0.2, 0.1),
            Point::new(1.0, 0.0),
        ] {
            push_trace(&mut t, p);
        }
        assert_eq!(t.len(), 2);
        let r = fit_text_rect(RectF::new(0.0, 0.0, 10.0, 10.0), "abc\nde", 20.0);
        assert!(r.w >= 3.0 * 12.0 && r.h >= 2.0 * 26.0, "{r:?}");
    }
}
