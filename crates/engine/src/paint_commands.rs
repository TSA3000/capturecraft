//! Freehand commands: the Pen tool (a vector stroke PhotoCraft's Freeform
//! Pen way: the trace thinned with `photocraft_algo::magnetic::simplify`
//! and drawn as the Bézier curve `fit_trace` fits through it) and the
//! Eraser, PhotoCraft's brush with `erase` on: round dabs along the drag
//! that clear the capture's alpha, written copy-on-write into the edited
//! base layer (so undo restores the pixels).

use capturecraft_format::{Annotation, Axis, MAX_POLYGON_POINTS, Manifest, PenStroke, Stroke};
use photocraft_algo::magnetic::simplify;
use photocraft_geom::{Point, Rect};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, cut_outs, has_doc, parse, push_node};
use crate::{EngineError, Result, Session};

pub(crate) static REGISTRY: &[CommandSpec] = &[
    CommandSpec {
        id: "annotation.addPen",
        label: "Add Pen Stroke",
        menu_path: &["Annotate", "Pen"],
        shortcut: None,
        params: r##"{ points: [{x, y}] (the pointer trace, canvas px; 2 or more), stroke?: Stroke,
  smoothing?: 0.1..100 = 2 (curve-fit tolerance, px) } -> { id, points (kept after thinning) }"##,
        enabled: has_doc,
        run: add_pen,
        journal: true,
    },
    CommandSpec {
        id: "canvas.erase",
        label: "Erase",
        menu_path: &[],
        shortcut: None,
        params: r##"{ points: [{x, y}] (canvas px), radius: f32 (brush radius, canvas px), hardness?: 0..1 = 0.8 }
-> { erased: {x0, y0, x1, y1} | null (base px) }"##,
        enabled: has_doc,
        run: erase,
        journal: true,
    },
];

/// Trace points closer than this to the line through their neighbours
/// are dropped before fitting, canvas px.
const THIN_TOLERANCE: f64 = 0.35;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PenParams {
    points: Vec<Point>,
    #[serde(default)]
    stroke: Option<Stroke>,
    #[serde(default)]
    smoothing: Option<f32>,
}

fn add_pen(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.addPen";
    let p: PenParams = parse(CMD, p)?;
    if p.points
        .iter()
        .any(|q| !(q.x.is_finite() && q.y.is_finite()))
    {
        return Err(bad(CMD, "points must be finite"));
    }
    let raw: Vec<[f64; 2]> = p.points.iter().map(|q| [q.x, q.y]).collect();
    // Thin harder until the trace fits the format's point limit.
    let mut tol = THIN_TOLERANCE;
    let mut thin = simplify(&raw, tol);
    while thin.len() > MAX_POLYGON_POINTS && tol < 64.0 {
        tol *= 2.0;
        thin = simplify(&raw, tol);
    }
    thin.dedup();
    if thin.len() < 2 {
        return Err(bad(CMD, "a pen stroke needs two distinct points"));
    }
    let d = PenStroke::default();
    let pen = PenStroke {
        points: thin.iter().map(|q| Point::new(q[0], q[1])).collect(),
        stroke: p.stroke.unwrap_or(d.stroke),
        smoothing: p.smoothing.unwrap_or(d.smoothing),
    };
    let n = pen.points.len();
    let id = push_node(s.manifest_mut()?, Annotation::Pen(pen));
    Ok(json!({ "id": id.0, "points": n }))
}

/// A canvas point in base pixels, through the cut outs (newest last).
pub fn canvas_point_to_base(m: &Manifest, p: Point) -> Point {
    let o = m.canvas.base_origin;
    let (mut x, mut y) = (p.x - f64::from(o.x), p.y - f64::from(o.y));
    let cuts: Vec<_> = cut_outs(m).collect();
    for c in cuts.into_iter().rev() {
        let shift = |v: f64| if v >= c.start { v + c.removed() } else { v };
        match c.axis {
            Axis::Horizontal => y = shift(y),
            Axis::Vertical => x = shift(x),
        }
    }
    Point::new(x, y)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EraseParams {
    points: Vec<Point>,
    radius: f32,
    #[serde(default)]
    hardness: Option<f32>,
}

/// Dabs every `radius / 4` along the polyline, so a fast drag leaves no gaps.
fn dabs(points: &[Point], radius: f64) -> Vec<Point> {
    let step = (radius / 4.0).max(0.5);
    let mut out = Vec::new();
    let Some(first) = points.first() else {
        return out;
    };
    out.push(*first);
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = (b.x - a.x).hypot(b.y - a.y);
        let n = (len / step).ceil().clamp(1.0, 10_000.0) as usize;
        for i in 1..=n {
            let t = i as f64 / n as f64;
            out.push(Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
        }
    }
    out
}

fn erase(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "canvas.erase";
    let p: EraseParams = parse(CMD, p)?;
    if !(p.radius.is_finite() && (0.5..=512.0).contains(&p.radius)) {
        return Err(bad(CMD, "radius must be within 0.5..=512"));
    }
    if p.points.is_empty() || p.points.len() > 100_000 {
        return Err(bad(CMD, "needs 1..=100000 points"));
    }
    if p.points
        .iter()
        .any(|q| !(q.x.is_finite() && q.y.is_finite()))
    {
        return Err(bad(CMD, "points must be finite"));
    }
    let hardness = f64::from(p.hardness.unwrap_or(0.8).clamp(0.0, 1.0));
    let doc = s.active_mut().ok_or(EngineError::NoDocument)?;
    let m = &doc.manifest;
    let r = f64::from(p.radius);
    let centers: Vec<Point> = dabs(&p.points, r)
        .into_iter()
        .map(|c| canvas_point_to_base(m, c))
        .collect();
    let bounds = Rect::from_size(m.base.size);
    let reach = r.ceil() as i32 + 1;
    let area = centers
        .iter()
        .map(|c| {
            let (x, y) = (c.x.floor() as i32, c.y.floor() as i32);
            Rect::new(x - reach, y - reach, x + reach + 1, y + reach + 1)
        })
        .reduce(|a, b| a.union(&b))
        .map(|a| a.intersect(&bounds))
        .filter(|a| !a.is_empty());
    let Some(area) = area else {
        return Ok(json!({ "erased": null }));
    };
    let base = doc
        .layers
        .edited
        .clone()
        .or_else(|| doc.pixels.clone())
        .ok_or_else(|| bad(CMD, "the document has no capture pixels"))?;
    let mut rgba = base.read_rgba8(area).map_err(EngineError::Other)?;
    let w = area.width() as usize;
    // Coverage per pixel: the strongest dab (soft edge outside `hardness`).
    let inner = r * hardness;
    for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = area.x0 as f64 + (i % w) as f64 + 0.5;
        let y = area.y0 as f64 + (i / w) as f64 + 0.5;
        let mut cover = 0.0f64;
        for c in &centers {
            let d = (x - c.x).hypot(y - c.y);
            let k = if d <= inner {
                1.0
            } else if d < r {
                1.0 - (d - inner) / (r - inner).max(1e-6)
            } else {
                0.0
            };
            cover = cover.max(k);
            if cover >= 1.0 {
                break;
            }
        }
        px[3] = (f64::from(px[3]) * (1.0 - cover)).round() as u8;
    }
    let mut edited = base;
    edited
        .write_rgba8(area, &rgba)
        .map_err(EngineError::Other)?;
    doc.layers.edited = Some(edited);
    Ok(json!({ "erased": { "x0": area.x0, "y0": area.y0, "x1": area.x1, "y1": area.y1 } }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::Pixels;
    use photocraft_geom::Size;

    fn session() -> Session {
        let mut s = Session::new();
        let px = Pixels::new(Size::new(100, 60), vec![255; 100 * 60 * 4]);
        s.open(Manifest::new(Size::new(100, 60)), px).unwrap();
        s
    }

    #[test]
    fn pen_strokes_are_thinned_and_drawn() {
        let mut s = session();
        // A straight drag sampled every pixel thins to its ends.
        let pts: Vec<Value> = (0..50).map(|x| json!({ "x": 10 + x, "y": 30 })).collect();
        let out = s
            .run("annotation.addPen", &json!({ "points": pts }))
            .unwrap();
        assert_eq!(out["points"], 2);
        let flat = crate::render::flatten(s.active().unwrap()).unwrap();
        let px = |x: usize, y: usize| flat.rgba[(y * 100 + x) * 4 + 1];
        assert!(px(30, 30) < 200, "red ink on the line");
        assert_eq!(px(30, 10), 255, "nothing off the line");
        assert!(
            s.run(
                "annotation.addPen",
                &json!({ "points": [{ "x": 1, "y": 1 }, { "x": 1, "y": 1 }] })
            )
            .is_err()
        );
    }

    #[test]
    fn the_eraser_clears_alpha_and_undo_brings_it_back() {
        let mut s = session();
        let out = s
            .run(
                "canvas.erase",
                &json!({ "points": [{ "x": 20, "y": 30 }, { "x": 60, "y": 30 }], "radius": 6, "hardness": 1 }),
            )
            .unwrap();
        assert_eq!(out["erased"]["y0"], 23);
        let px = s.active().unwrap().edited_pixels().unwrap();
        assert_eq!(px.pixel(40, 30)[3], 0, "on the drag");
        assert_eq!(px.pixel(40, 45)[3], 255, "beside it");
        assert_eq!(
            s.active().unwrap().pixels.as_ref().unwrap().pixel(40, 30)[3],
            255,
            "the capture stays"
        );
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(
            s.active().unwrap().edited_pixels().unwrap().pixel(40, 30)[3],
            255
        );
        // Off the capture: nothing to erase.
        let out = s
            .run(
                "canvas.erase",
                &json!({ "points": [{ "x": 500, "y": 500 }], "radius": 4 }),
            )
            .unwrap();
        assert!(out["erased"].is_null());
    }
}
