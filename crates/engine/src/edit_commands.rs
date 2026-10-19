//! Generic editing commands the editor's gizmos and Quick Styles drive:
//! growing the canvas from its outer handles, moving annotations, bending
//! arrows and applying a style preset.
//!
//! Like [`crate::commands`], every command parses into a typed struct
//! (`deny_unknown_fields`) and never panics on input; the session validates
//! the document afterwards and rolls back on any error.

use std::collections::HashMap;

use capturecraft_format::{
    Annotation, ArrowCurve, Background, CalloutBody, Color, CurvedArrow, DropShadow, Fill,
    FontSpec, HeadStyle, MAX_SIDE, Manifest, NodeId, RegionShape, Stroke,
};
use photocraft_geom::{Point, Rect, Size};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, has_doc, node_annotation, parse, push_node};
use crate::{Pixels, Result, Session};

pub(crate) static REGISTRY: &[CommandSpec] = &[
    CommandSpec {
        id: "annotation.addArrow",
        label: "Add Arrow",
        menu_path: &["Annotate", "Arrow"],
        shortcut: None,
        params: r##"{ start: {x, y}, end: {x, y}, stroke?: Stroke, shadow?: DropShadow|null,
  start_head?: HeadStyle = "none", end_head?: HeadStyle = "triangle", taper?: bool }
-> { id }"##,
        enabled: has_doc,
        run: add_arrow,
        journal: true,
    },
    CommandSpec {
        id: "canvas.expandBounds",
        label: "Expand Canvas",
        menu_path: &["Image", "Canvas"],
        shortcut: None,
        params: r##"{ left?: i32 = 0, top?: i32 = 0, right?: i32 = 0, bottom?: i32 = 0 (px added on each side;
  negative trims), background?: {kind: "transparent"} | {kind: "solid", color} | {kind: "auto"} (most
  common colour of the capture's outer edge) }
-> { canvas: {width, height}, base_origin: {x, y} }"##,
        enabled: has_doc,
        run: expand_bounds,
        journal: true,
    },
    CommandSpec {
        id: "annotation.move",
        label: "Move",
        menu_path: &[],
        shortcut: None,
        params: r##"{ ids: [u64], dx: f64, dy: f64 } -> { moved: [ids] }"##,
        enabled: has_doc,
        run: move_annotations,
        journal: true,
    },
    CommandSpec {
        id: "annotation.setArrowCurve",
        label: "Bend Arrow",
        menu_path: &[],
        shortcut: None,
        params: r##"{ id: u64, through?: {x, y} (point the curve passes at its middle),
  control?: {x, y} (quadratic control point) } — neither = straight -> { curve }"##,
        enabled: has_doc,
        run: set_arrow_curve,
        journal: true,
    },
    CommandSpec {
        id: "annotation.setStyle",
        label: "Apply Style",
        menu_path: &[],
        shortcut: None,
        params: r##"{ ids: [u64], stroke?: Stroke|null, fill?: Fill, shadow?: DropShadow|null, opacity?: 0..1,
  text_color?: color, font?: FontSpec, arrow_start?: HeadStyle, arrow_end?: HeadStyle, corner_radius?: f32 }
Fields an annotation type has no use for are ignored. -> { styled: [ids] }"##,
        enabled: has_doc,
        run: set_style,
        journal: true,
    },
];

/// Absent = keep, `null` = off (as in `commands.rs`).
fn double_option<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

// ---------------------------------------------------------------------------
// Queries for the editor's gizmos

/// Topmost visible, unlocked annotation whose bounds, grown by `slop`
/// canvas pixels, contain `p` (cut outs span the canvas and never hit).
pub fn node_at(m: &Manifest, p: Point, slop: f64) -> Option<NodeId> {
    let slop = if slop.is_finite() { slop.max(0.0) } else { 0.0 };
    m.annotations
        .iter()
        .rev()
        .filter(|n| n.visible && !n.locked)
        .find(|n| {
            n.annotation()
                .and_then(Annotation::bounds)
                .is_some_and(|b| {
                    p.x >= b.x - slop
                        && p.x <= b.x + b.w + slop
                        && p.y >= b.y - slop
                        && p.y <= b.y + b.h + slop
                })
        })
        .map(|n| n.id)
}

/// The point halfway along an arrow's shaft (its bend handle).
pub fn arrow_midpoint(a: &CurvedArrow) -> Point {
    let (s, e) = (a.start, a.end);
    let mix = |pts: &[(f64, Point)]| {
        Point::new(
            pts.iter().map(|(w, p)| w * p.x).sum(),
            pts.iter().map(|(w, p)| w * p.y).sum(),
        )
    };
    match a.curve {
        ArrowCurve::Straight => mix(&[(0.5, s), (0.5, e)]),
        ArrowCurve::Quadratic { control } => mix(&[(0.25, s), (0.5, control), (0.25, e)]),
        ArrowCurve::Cubic { c0, c1 } => mix(&[(0.125, s), (0.375, c0), (0.375, c1), (0.125, e)]),
    }
}

// ---------------------------------------------------------------------------
// Arrow tool

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddArrowParams {
    pub start: Point,
    pub end: Point,
    #[serde(default)]
    pub stroke: Option<Stroke>,
    #[serde(default, deserialize_with = "double_option")]
    pub shadow: Option<Option<DropShadow>>,
    #[serde(default)]
    pub start_head: Option<HeadStyle>,
    #[serde(default)]
    pub end_head: Option<HeadStyle>,
    #[serde(default)]
    pub taper: Option<bool>,
}

fn add_arrow(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.addArrow";
    let p: AddArrowParams = parse(CMD, p)?;
    let d = CurvedArrow::default();
    let mut arrow = CurvedArrow {
        start: p.start,
        end: p.end,
        stroke: p.stroke.unwrap_or(d.stroke),
        shadow: p.shadow.unwrap_or(d.shadow),
        taper: p.taper.unwrap_or(d.taper),
        ..d
    };
    if let Some(h) = p.start_head {
        arrow.start_head.style = h;
    }
    if let Some(h) = p.end_head {
        arrow.end_head.style = h;
    }
    let id = push_node(s.manifest_mut()?, Annotation::CurvedArrow(arrow));
    Ok(json!({ "id": id.0 }))
}

// ---------------------------------------------------------------------------
// Canvas expansion

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackdropParam {
    Transparent,
    Solid {
        color: Color,
    },
    /// The most common colour along the capture's outer edge.
    Auto,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpandBoundsParams {
    #[serde(default)]
    pub left: i32,
    #[serde(default)]
    pub top: i32,
    #[serde(default)]
    pub right: i32,
    #[serde(default)]
    pub bottom: i32,
    #[serde(default)]
    pub background: Option<BackdropParam>,
}

/// Most common colour of the outer 1 px ring of `p`.
pub fn border_color(p: &Pixels) -> Color {
    let s = p.size();
    let (w, h) = (s.width as i32, s.height as i32);
    let strips = [
        Rect::new(0, 0, w, 1),
        Rect::new(0, h - 1, w, h),
        Rect::new(0, 0, 1, h),
        Rect::new(w - 1, 0, w, h),
    ];
    let mut counts: HashMap<[u8; 4], u32> = HashMap::new();
    for r in strips.iter().filter(|r| !r.is_empty()) {
        if let Ok(rgba) = p.read_rgba8(*r) {
            for px in rgba.as_chunks::<4>().0 {
                *counts.entry(*px).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        // Ties go to the smaller value so the result is deterministic.
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map_or(Color::WHITE, |([r, g, b, a], _)| Color::rgba(r, g, b, a))
}

fn expand_bounds(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "canvas.expandBounds";
    let p: ExpandBoundsParams = parse(CMD, p)?;
    let background = match p.background {
        None => None,
        Some(BackdropParam::Transparent) => Some(Background::Transparent),
        Some(BackdropParam::Solid { color }) => Some(Background::Solid { color }),
        Some(BackdropParam::Auto) => {
            let doc = s.active().ok_or(crate::EngineError::NoDocument)?;
            let px = doc
                .display_pixels()
                .ok_or_else(|| bad(CMD, "auto needs the capture's pixels"))?;
            Some(Background::Solid {
                color: border_color(px),
            })
        }
    };
    let m = s.manifest_mut()?;
    let side = |len: u32, a: i32, b: i32| -> Result<u32> {
        let v = i64::from(len) + i64::from(a) + i64::from(b);
        if !(1..=i64::from(MAX_SIDE)).contains(&v) {
            return Err(bad(
                CMD,
                format!("the canvas side would be {v} px (1..={MAX_SIDE})"),
            ));
        }
        Ok(v as u32)
    };
    let size = Size::new(
        side(m.canvas.size.width, p.left, p.right)?,
        side(m.canvas.size.height, p.top, p.bottom)?,
    );
    m.canvas.size = size;
    let o = &mut m.canvas.base_origin;
    o.x = o.x.saturating_add(p.left);
    o.y = o.y.saturating_add(p.top);
    if let Some(b) = background {
        m.canvas.background = b;
    }
    // Annotations keep their place on the capture (cut outs are in base
    // space and move with the origin by themselves).
    if p.left != 0 || p.top != 0 {
        translate_all(m, f64::from(p.left), f64::from(p.top));
    }
    let o = m.canvas.base_origin;
    Ok(json!({
        "canvas": { "width": size.width, "height": size.height },
        "base_origin": { "x": o.x, "y": o.y },
    }))
}

fn translate_all(m: &mut Manifest, dx: f64, dy: f64) {
    for n in &mut m.annotations {
        if let Some(a) = n.annotation_mut() {
            if matches!(a, Annotation::CutOutSeam(_)) {
                continue;
            }
            a.translate(dx, dy);
            n.touch();
        }
    }
    if let Some(c) = &mut m.cursor {
        c.position.x += dx;
        c.position.y += dy;
    }
}

// ---------------------------------------------------------------------------
// Move

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveParams {
    pub ids: Vec<u64>,
    pub dx: f64,
    pub dy: f64,
}

fn move_annotations(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.move";
    let mut p: MoveParams = parse(CMD, p)?;
    let lim = f64::from(MAX_SIDE) * 2.0;
    if !(p.dx.is_finite() && p.dy.is_finite() && p.dx.abs() <= lim && p.dy.abs() <= lim) {
        return Err(bad(
            CMD,
            "dx and dy must be finite and within the canvas range",
        ));
    }
    p.ids.sort_unstable();
    p.ids.dedup();
    let m = s.manifest_mut()?;
    for &id in &p.ids {
        let node = node_annotation(m, id)?;
        match node.annotation_mut() {
            Some(Annotation::CutOutSeam(_)) => {
                return Err(bad(CMD, format!("cut out {id} stays with the capture")));
            }
            Some(a) => a.translate(p.dx, p.dy),
            None => return Err(bad(CMD, format!("annotation {id} has an unknown type"))),
        }
        node.touch();
    }
    Ok(json!({ "moved": p.ids }))
}

// ---------------------------------------------------------------------------
// Arrow curve

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrowCurveParams {
    pub id: u64,
    #[serde(default)]
    pub through: Option<Point>,
    #[serde(default)]
    pub control: Option<Point>,
}

/// Control point of the quadratic through `s`, `m` (at t = ½) and `e`.
pub fn control_through(s: Point, m: Point, e: Point) -> Point {
    Point::new(2.0 * m.x - (s.x + e.x) / 2.0, 2.0 * m.y - (s.y + e.y) / 2.0)
}

fn set_arrow_curve(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.setArrowCurve";
    let p: ArrowCurveParams = parse(CMD, p)?;
    let node = node_annotation(s.manifest_mut()?, p.id)?;
    let Some(Annotation::CurvedArrow(a)) = node.annotation_mut() else {
        return Err(bad(CMD, format!("annotation {} is not an arrow", p.id)));
    };
    a.curve = match (p.through, p.control) {
        (Some(_), Some(_)) => return Err(bad(CMD, "give `through` or `control`, not both")),
        (Some(m), None) => ArrowCurve::Quadratic {
            control: control_through(a.start, m, a.end),
        },
        (None, Some(control)) => ArrowCurve::Quadratic { control },
        (None, None) => ArrowCurve::Straight,
    };
    let curve = serde_json::to_value(a.curve).map_err(|e| bad(CMD, e))?;
    node.touch();
    Ok(json!({ "curve": curve }))
}

// ---------------------------------------------------------------------------
// Style

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct StyleParams {
    pub ids: Vec<u64>,
    #[serde(default, deserialize_with = "double_option")]
    pub stroke: Option<Option<Stroke>>,
    #[serde(default)]
    pub fill: Option<Fill>,
    #[serde(default, deserialize_with = "double_option")]
    pub shadow: Option<Option<DropShadow>>,
    #[serde(default)]
    pub opacity: Option<f32>,
    #[serde(default)]
    pub text_color: Option<Color>,
    #[serde(default)]
    pub font: Option<FontSpec>,
    #[serde(default)]
    pub arrow_start: Option<HeadStyle>,
    #[serde(default)]
    pub arrow_end: Option<HeadStyle>,
    #[serde(default)]
    pub corner_radius: Option<f32>,
}

/// The colour of a solid fill (badges take only solid colours).
fn solid(f: &Fill) -> Option<Color> {
    match f {
        Fill::Solid { color } => Some(*color),
        _ => None,
    }
}

fn apply_style(a: &mut Annotation, p: &StyleParams) {
    match a {
        Annotation::Callout(c) => {
            if let Some(s) = p.stroke {
                c.stroke = s;
            }
            if let Some(f) = &p.fill {
                c.fill = f.clone();
            }
            if let Some(s) = p.shadow {
                c.shadow = s;
            }
            for run in &mut c.text.runs {
                if let Some(col) = p.text_color {
                    run.color = col;
                }
                if let Some(font) = &p.font {
                    run.font = font.clone();
                }
            }
            if let (Some(radius), CalloutBody::Rect | CalloutBody::RoundedRect { .. }) =
                (p.corner_radius, c.body)
            {
                c.body = CalloutBody::RoundedRect { radius };
            }
        }
        Annotation::StepBadge(b) => {
            if let Some(col) = p.fill.as_ref().and_then(solid) {
                b.fill = col;
            }
            if let Some(col) = p.text_color {
                b.text_color = col;
            }
            if let Some(font) = &p.font {
                b.font = font.clone();
            }
            if let Some(s) = p.stroke {
                b.ring = s;
            }
            if let Some(s) = p.shadow {
                b.shadow = s;
            }
        }
        Annotation::CurvedArrow(arrow) => {
            // An arrow is its stroke: `null` cannot remove it.
            if let Some(Some(s)) = p.stroke {
                arrow.stroke = s;
            }
            if let Some(s) = p.shadow {
                arrow.shadow = s;
            }
            if let Some(h) = p.arrow_start {
                arrow.start_head.style = h;
            }
            if let Some(h) = p.arrow_end {
                arrow.end_head.style = h;
            }
        }
        Annotation::Shape(sh) => {
            if let Some(s) = p.stroke {
                sh.stroke = s;
            }
            if let Some(f) = &p.fill {
                sh.fill = f.clone();
            }
            if let Some(s) = p.shadow {
                sh.shadow = s;
            }
            if let Some(r) = p.corner_radius {
                sh.shape = match std::mem::replace(
                    &mut sh.shape,
                    RegionShape::Polygon { points: Vec::new() },
                ) {
                    RegionShape::Rect { rect } | RegionShape::RoundedRect { rect, .. } => {
                        RegionShape::RoundedRect { rect, radius: r }
                    }
                    other => other,
                };
            }
        }
        Annotation::MagnifierLens(m) => {
            if let Some(s) = p.stroke {
                m.border = s;
            }
        }
        Annotation::BlurRegion(_)
        | Annotation::Spotlight(_)
        | Annotation::CutOutSeam(_)
        | Annotation::SmartObject(_) => {}
    }
}

fn set_style(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.setStyle";
    let mut p: StyleParams = parse(CMD, p)?;
    p.ids.sort_unstable();
    p.ids.dedup();
    let m = s.manifest_mut()?;
    for &id in &p.ids {
        let node = node_annotation(m, id)?;
        if let Some(o) = p.opacity {
            node.opacity = o;
        }
        match node.annotation_mut() {
            Some(a) => apply_style(a, &p),
            None => return Err(bad(CMD, format!("annotation {id} has an unknown type"))),
        }
        node.touch();
    }
    Ok(json!({ "styled": p.ids }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use capturecraft_format::Offset;
    use photocraft_geom::Size;

    fn session(w: u32, h: u32, rgba: [u8; 4]) -> Session {
        let mut s = Session::new();
        let data = rgba.repeat((w * h) as usize);
        let px = Pixels::new(Size::new(w, h), data);
        s.open(Manifest::new(Size::new(w, h)), px).unwrap();
        s
    }

    fn manifest(s: &Session) -> &Manifest {
        &s.active().unwrap().manifest
    }

    fn add_arrow(s: &mut Session) -> u64 {
        s.run(
            "annotation.addArrow",
            &json!({ "start": { "x": 0, "y": 0 }, "end": { "x": 100, "y": 0 } }),
        )
        .unwrap()["id"]
            .as_u64()
            .unwrap()
    }

    #[test]
    fn queries_find_the_topmost_node_and_the_bend_handle() {
        let mut s = session(200, 200, [255; 4]);
        let a = add_arrow(&mut s);
        let step = s
            .run("annotation.addStep", &json!({ "at": { "x": 50, "y": 0 } }))
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let m = manifest(&s);
        assert_eq!(node_at(m, Point::new(50.0, 0.0), 2.0), Some(NodeId(step)));
        assert_eq!(node_at(m, Point::new(95.0, 1.0), 2.0), Some(NodeId(a)));
        assert_eq!(node_at(m, Point::new(150.0, 150.0), 2.0), None);
        s.run(
            "annotation.setArrowCurve",
            &json!({ "id": a, "through": { "x": 50, "y": 30 } }),
        )
        .unwrap();
        let Some(Annotation::CurvedArrow(arr)) = manifest(&s).node(NodeId(a)).unwrap().annotation()
        else {
            panic!()
        };
        let mid = arrow_midpoint(arr);
        assert!((mid.x - 50.0).abs() < 1e-9 && (mid.y - 30.0).abs() < 1e-9);
    }

    #[test]
    fn expand_grows_outward_and_keeps_annotations_on_the_capture() {
        let mut s = session(100, 80, [10, 20, 30, 255]);
        let r = s
            .run("annotation.addStep", &json!({ "at": { "x": 10, "y": 10 } }))
            .unwrap();
        let step = r["id"].as_u64().unwrap();
        let r = s
            .run(
                "canvas.expandBounds",
                &json!({ "left": 20, "top": 5, "right": 30, "background": { "kind": "auto" } }),
            )
            .unwrap();
        assert_eq!(r["canvas"], json!({ "width": 150, "height": 85 }));
        let m = manifest(&s);
        assert_eq!(m.canvas.base_origin, Offset { x: 20, y: 5 });
        assert_eq!(
            m.canvas.background,
            Background::Solid {
                color: Color::rgba(10, 20, 30, 255)
            }
        );
        let Some(Annotation::StepBadge(b)) = m.node(NodeId(step)).unwrap().annotation() else {
            panic!("not a step");
        };
        assert_eq!((b.center.x, b.center.y), (30.0, 15.0));
        // Undo restores both.
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(manifest(&s).canvas.size, Size::new(100, 80));
    }

    #[test]
    fn expand_rejects_degenerate_canvases() {
        let mut s = session(10, 10, [0; 4]);
        for p in [
            json!({ "left": -10 }),
            json!({ "right": i32::MAX }),
            json!({ "top": 1, "nope": 1 }),
            json!({ "background": { "kind": "frame" } }),
        ] {
            assert!(s.run("canvas.expandBounds", &p).is_err(), "{p}");
        }
        assert_eq!(manifest(&s).canvas.size, Size::new(10, 10));
    }

    #[test]
    fn border_colour_takes_the_majority() {
        let mut p = Pixels::new(Size::new(4, 3), [9u8, 9, 9, 255].repeat(12)).unwrap();
        p.write_rgba8(Rect::new(0, 0, 1, 1), &[1, 2, 3, 255])
            .unwrap();
        assert_eq!(border_color(&p), Color::rgba(9, 9, 9, 255));
    }

    #[test]
    fn move_translates_and_rejects_cut_outs() {
        let mut s = session(200, 200, [255; 4]);
        let a = add_arrow(&mut s);
        s.run(
            "annotation.move",
            &json!({ "ids": [a, a], "dx": 5, "dy": -2 }),
        )
        .unwrap();
        let Some(Annotation::CurvedArrow(arr)) = manifest(&s).node(NodeId(a)).unwrap().annotation()
        else {
            panic!()
        };
        assert_eq!((arr.start.x, arr.start.y), (5.0, -2.0));
        let cut = s
            .run(
                "canvas.cutOut",
                &json!({ "axis": "horizontal", "start": 10, "end": 20 }),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        assert!(
            s.run(
                "annotation.move",
                &json!({ "ids": [a, cut], "dx": 1, "dy": 1 })
            )
            .is_err()
        );
        assert!(
            s.run(
                "annotation.move",
                &json!({ "ids": [a], "dx": f64::MAX, "dy": 0 })
            )
            .is_err()
        );
        assert!(
            s.run(
                "annotation.move",
                &json!({ "ids": [999], "dx": 1, "dy": 1 })
            )
            .is_err()
        );
    }

    #[test]
    fn arrow_curve_passes_through_the_handle() {
        let mut s = session(200, 200, [255; 4]);
        let a = add_arrow(&mut s);
        s.run(
            "annotation.setArrowCurve",
            &json!({ "id": a, "through": { "x": 50, "y": 40 } }),
        )
        .unwrap();
        let Some(Annotation::CurvedArrow(arr)) = manifest(&s).node(NodeId(a)).unwrap().annotation()
        else {
            panic!()
        };
        let ArrowCurve::Quadratic { control } = arr.curve else {
            panic!()
        };
        // B(½) = (S + 2C + E) / 4.
        let mid = |s: f64, c: f64, e: f64| (s + 2.0 * c + e) / 4.0;
        assert_eq!(mid(arr.start.x, control.x, arr.end.x), 50.0);
        assert_eq!(mid(arr.start.y, control.y, arr.end.y), 40.0);
        s.run("annotation.setArrowCurve", &json!({ "id": a }))
            .unwrap();
        let Some(Annotation::CurvedArrow(arr)) = manifest(&s).node(NodeId(a)).unwrap().annotation()
        else {
            panic!()
        };
        assert_eq!(arr.curve, ArrowCurve::Straight);
        let step = s
            .run("annotation.addStep", &json!({ "at": { "x": 1, "y": 1 } }))
            .unwrap()["id"]
            .clone();
        assert!(
            s.run("annotation.setArrowCurve", &json!({ "id": step }))
                .is_err()
        );
    }

    #[test]
    fn style_applies_what_each_type_uses() {
        let mut s = session(200, 200, [255; 4]);
        let a = add_arrow(&mut s);
        let step = s
            .run("annotation.addStep", &json!({ "at": { "x": 50, "y": 50 } }))
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let red = Color::rgb(200, 0, 0);
        s.run(
            "annotation.setStyle",
            &json!({
                "ids": [a, step],
                "fill": { "kind": "solid", "color": red },
                "stroke": null,
                "shadow": null,
                "arrow_end": "open_v",
                "opacity": 0.5,
            }),
        )
        .unwrap();
        let m = manifest(&s);
        let Some(Annotation::CurvedArrow(arr)) = m.node(NodeId(a)).unwrap().annotation() else {
            panic!()
        };
        assert_eq!(arr.end_head.style, HeadStyle::OpenV);
        assert!(arr.shadow.is_none());
        assert_eq!(
            arr.stroke,
            CurvedArrow::default().stroke,
            "null keeps the shaft"
        );
        let Some(Annotation::StepBadge(b)) = m.node(NodeId(step)).unwrap().annotation() else {
            panic!()
        };
        assert_eq!(b.fill, red);
        assert!(b.ring.is_none());
        assert_eq!(m.node(NodeId(step)).unwrap().opacity, 0.5);
        // Out-of-range opacity is the format's error and rolls back.
        assert!(
            s.run("annotation.setStyle", &json!({ "ids": [a], "opacity": 3 }))
                .is_err()
        );
        assert_eq!(manifest(&s).node(NodeId(a)).unwrap().opacity, 0.5);
    }
}
