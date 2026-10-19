//! Transform commands the canvas gizmo drives (resize handles, the rotation
//! stem) and the canvas backdrop (the rounded drop-shadow frame).
//!
//! A node's geometry is stored unrotated; its rotation turns it about
//! [`Annotation::pivot`]. `node.resize` sets the unrotated bounds only, so
//! for a rotated node the pivot moves with the new bounds: the UI keeps the
//! anchor handle fixed on screen by sending bounds it has already corrected
//! for that.

use capturecraft_format::{Annotation, AnnotationNode, CanvasBackdrop, RectF, normalize_angle};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, has_doc, node_annotation, parse};
use crate::{Result, Session};

pub(crate) static REGISTRY: &[CommandSpec] = &[
    CommandSpec {
        id: "node.resize",
        label: "Resize",
        menu_path: &[],
        shortcut: None,
        params: r##"{ id: u64, bounds: {x, y, w, h} } — the new unrotated geometry bounds (w, h > 0);
  for a rotated node the caller corrects them so the anchor stays put -> { id, bounds }"##,
        enabled: has_doc,
        run: resize,
        journal: true,
    },
    CommandSpec {
        id: "node.rotate",
        label: "Rotate",
        menu_path: &[],
        shortcut: None,
        params: r##"{ id: u64, rotation_rad?: f32 (absolute) | by_rad?: f32 (relative) } — exactly one;
  clockwise on screen, about the node's pivot, stored within -π..=π -> { id, rotation_rad }"##,
        enabled: has_doc,
        run: rotate,
        journal: true,
    },
    CommandSpec {
        id: "node.transform",
        label: "Transform",
        menu_path: &[],
        shortcut: None,
        params: r##"{ id: u64, bounds?: {x, y, w, h}, rotation_rad?: f32 } — at least one; one undo step
-> { id, bounds, rotation_rad }"##,
        enabled: has_doc,
        run: transform,
        journal: true,
    },
    CommandSpec {
        id: "canvas.setBackdrop",
        label: "Canvas Backdrop",
        menu_path: &["Image", "Canvas"],
        shortcut: None,
        params: r##"{ backdrop: {kind: "none"} | {kind: "rounded_shadow_frame", margin_px: f32, corner_radius_px: f32,
  backdrop_fill: Fill (gradient points in frame space), shadow: DropShadow|null, border: Stroke|null} }
-> { backdrop, frame_size: {width, height} }"##,
        enabled: has_doc,
        run: set_backdrop,
        journal: true,
    },
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResizeParams {
    pub id: u64,
    pub bounds: RectF,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotateParams {
    pub id: u64,
    #[serde(default)]
    pub rotation_rad: Option<f32>,
    #[serde(default)]
    pub by_rad: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformParams {
    pub id: u64,
    #[serde(default)]
    pub bounds: Option<RectF>,
    #[serde(default)]
    pub rotation_rad: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackdropParams {
    pub backdrop: CanvasBackdrop,
}

/// The node `id` as a transformable annotation: known, unlocked, not a cut
/// out.
fn transformable<'a>(cmd: &str, s: &'a mut Session, id: u64) -> Result<&'a mut AnnotationNode> {
    let node = node_annotation(s.manifest_mut()?, id)?;
    match node.annotation() {
        None => Err(bad(cmd, format!("annotation {id} has an unknown type"))),
        Some(Annotation::CutOutSeam(_)) => Err(bad(
            cmd,
            format!("cut out {id} cannot be resized or rotated"),
        )),
        Some(_) => Ok(node),
    }
}

fn check_bounds(cmd: &str, r: RectF) -> Result<()> {
    let ok = [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite()) && r.w > 0.0 && r.h > 0.0;
    if ok {
        Ok(())
    } else {
        Err(bad(
            cmd,
            "bounds must be finite with a positive width and height",
        ))
    }
}

fn check_angle(cmd: &str, a: f32) -> Result<f32> {
    if a.is_finite() {
        Ok(normalize_angle(a))
    } else {
        Err(bad(cmd, "the angle must be finite"))
    }
}

fn apply_bounds(cmd: &str, node: &mut AnnotationNode, to: RectF) -> Result<RectF> {
    let id = node.id.0;
    let Some(a) = node.annotation_mut() else {
        return Err(bad(cmd, format!("annotation {id} has an unknown type")));
    };
    let from = a
        .bounds()
        .ok_or_else(|| bad(cmd, format!("annotation {id} has no bounds")))?;
    a.resize(from, to);
    Ok(a.bounds().unwrap_or(to))
}

fn bounds_json(r: RectF) -> Value {
    json!({ "x": r.x, "y": r.y, "w": r.w, "h": r.h })
}

fn resize(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "node.resize";
    let p: ResizeParams = parse(CMD, p)?;
    check_bounds(CMD, p.bounds)?;
    let node = transformable(CMD, s, p.id)?;
    let b = apply_bounds(CMD, node, p.bounds)?;
    node.touch();
    Ok(json!({ "id": p.id, "bounds": bounds_json(b) }))
}

fn rotate(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "node.rotate";
    let p: RotateParams = parse(CMD, p)?;
    let node = transformable(CMD, s, p.id)?;
    let to = match (p.rotation_rad, p.by_rad) {
        (Some(a), None) => a,
        (None, Some(d)) => node.rotation_rad + d,
        _ => return Err(bad(CMD, "give exactly one of `rotation_rad` and `by_rad`")),
    };
    node.rotation_rad = check_angle(CMD, to)?;
    node.touch();
    Ok(json!({ "id": p.id, "rotation_rad": node.rotation_rad }))
}

fn transform(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "node.transform";
    let p: TransformParams = parse(CMD, p)?;
    if p.bounds.is_none() && p.rotation_rad.is_none() {
        return Err(bad(CMD, "give `bounds`, `rotation_rad` or both"));
    }
    if let Some(b) = p.bounds {
        check_bounds(CMD, b)?;
    }
    let angle = p.rotation_rad.map(|a| check_angle(CMD, a)).transpose()?;
    let node = transformable(CMD, s, p.id)?;
    if let Some(b) = p.bounds {
        apply_bounds(CMD, node, b)?;
    }
    if let Some(a) = angle {
        node.rotation_rad = a;
    }
    node.touch();
    let bounds = node
        .annotation()
        .and_then(Annotation::bounds)
        .unwrap_or_default();
    Ok(json!({
        "id": p.id,
        "bounds": bounds_json(bounds),
        "rotation_rad": node.rotation_rad,
    }))
}

fn set_backdrop(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "canvas.setBackdrop";
    let p: BackdropParams = parse(CMD, p)?;
    let m = s.manifest_mut()?;
    m.canvas.backdrop = p.backdrop;
    // The session validates the whole manifest afterwards and rolls back.
    let size = m.canvas.backdrop.frame_size(m.canvas.size);
    let backdrop = serde_json::to_value(&m.canvas.backdrop).map_err(|e| bad(CMD, e))?;
    Ok(json!({
        "backdrop": backdrop,
        "frame_size": { "width": size.width, "height": size.height },
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::Pixels;
    use capturecraft_format::{Manifest, NodeId};
    use photocraft_geom::Size;

    fn session() -> Session {
        let mut s = Session::new();
        let px = Pixels::new(Size::new(200, 100), [255u8; 4].repeat(200 * 100));
        s.open(Manifest::new(Size::new(200, 100)), px).unwrap();
        s
    }

    fn node(s: &Session, id: u64) -> AnnotationNode {
        s.active()
            .unwrap()
            .manifest
            .node(NodeId(id))
            .unwrap()
            .clone()
    }

    fn add_callout(s: &mut Session) -> u64 {
        s.run(
            "annotation.addCallout",
            &json!({ "bounds": { "x": 20, "y": 30, "w": 100, "h": 40 }, "text": "Hi" }),
        )
        .unwrap()["id"]
            .as_u64()
            .unwrap()
    }

    #[test]
    fn resize_sets_the_bounds_and_undoes() {
        let mut s = session();
        let id = add_callout(&mut s);
        let rev = node(&s, id).revision;
        let r = s
            .run(
                "node.resize",
                &json!({ "id": id, "bounds": { "x": 10, "y": 10, "w": 50, "h": 80 } }),
            )
            .unwrap();
        assert_eq!(
            r["bounds"],
            json!({ "x": 10.0, "y": 10.0, "w": 50.0, "h": 80.0 })
        );
        let n = node(&s, id);
        assert!(n.revision > rev);
        assert_eq!(
            n.annotation().and_then(Annotation::bounds),
            Some(RectF::new(10.0, 10.0, 50.0, 80.0))
        );
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(
            node(&s, id).annotation().and_then(Annotation::bounds),
            Some(RectF::new(20.0, 30.0, 100.0, 40.0))
        );
        s.run("edit.redo", &json!({})).unwrap();
        assert_eq!(
            node(&s, id).annotation().and_then(Annotation::bounds),
            Some(RectF::new(10.0, 10.0, 50.0, 80.0))
        );
    }

    #[test]
    fn rotate_is_absolute_or_relative_and_wraps() {
        use std::f32::consts::PI;
        let mut s = session();
        let id = add_callout(&mut s);
        s.run("node.rotate", &json!({ "id": id, "rotation_rad": 0.5 }))
            .unwrap();
        let r = s
            .run("node.rotate", &json!({ "id": id, "by_rad": 2.0 * PI }))
            .unwrap();
        assert!((r["rotation_rad"].as_f64().unwrap() - 0.5).abs() < 1e-5);
        s.run("node.rotate", &json!({ "id": id, "by_rad": PI }))
            .unwrap();
        let a = node(&s, id).rotation_rad;
        assert!((a - (0.5 - PI)).abs() < 1e-5, "{a}");
        s.run("edit.undo", &json!({})).unwrap();
        assert!((node(&s, id).rotation_rad - 0.5).abs() < 1e-5);
        s.run("edit.undo", &json!({})).unwrap();
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(node(&s, id).rotation_rad, 0.0);
    }

    #[test]
    fn transform_is_one_undo_step() {
        let mut s = session();
        let id = add_callout(&mut s);
        let r = s
            .run(
                "node.transform",
                &json!({ "id": id, "bounds": { "x": 0, "y": 0, "w": 40, "h": 20 }, "rotation_rad": 1.0 }),
            )
            .unwrap();
        assert_eq!(r["bounds"]["w"], json!(40.0));
        assert!((r["rotation_rad"].as_f64().unwrap() - 1.0).abs() < 1e-6);
        s.run("edit.undo", &json!({})).unwrap();
        let n = node(&s, id);
        assert_eq!(n.rotation_rad, 0.0);
        assert_eq!(
            n.annotation().and_then(Annotation::bounds),
            Some(RectF::new(20.0, 30.0, 100.0, 40.0))
        );
    }

    #[test]
    fn invalid_transforms_are_rejected() {
        let mut s = session();
        let id = add_callout(&mut s);
        let cut = s
            // Right of the callout, so the cut does not move it.
            .run(
                "canvas.cutOut",
                &json!({ "axis": "vertical", "start": 180, "end": 190 }),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        for (cmd, p) in [
            (
                "node.resize",
                json!({ "id": id, "bounds": { "x": 0, "y": 0, "w": 0, "h": 5 } }),
            ),
            (
                "node.resize",
                json!({ "id": 999, "bounds": { "x": 0, "y": 0, "w": 5, "h": 5 } }),
            ),
            (
                "node.resize",
                json!({ "id": cut, "bounds": { "x": 0, "y": 0, "w": 5, "h": 5 } }),
            ),
            (
                "node.resize",
                json!({ "id": id, "bounds": { "x": 0, "y": 0, "w": 5, "h": 5 }, "z": 1 }),
            ),
            ("node.rotate", json!({ "id": id })),
            (
                "node.rotate",
                json!({ "id": id, "rotation_rad": 1, "by_rad": 1 }),
            ),
            ("node.rotate", json!({ "id": cut, "rotation_rad": 1 })),
            ("node.transform", json!({ "id": id })),
        ] {
            assert!(s.run(cmd, &p).is_err(), "{cmd} {p}");
        }
        let n = node(&s, id);
        assert_eq!(n.rotation_rad, 0.0);
        assert_eq!(
            n.annotation().and_then(Annotation::bounds),
            Some(RectF::new(20.0, 30.0, 100.0, 40.0))
        );
        // Locked nodes refuse.
        s.manifest_mut()
            .unwrap()
            .node_mut(NodeId(id))
            .unwrap()
            .locked = true;
        assert!(
            s.run("node.rotate", &json!({ "id": id, "rotation_rad": 1 }))
                .is_err()
        );
    }

    #[test]
    fn backdrop_sets_the_frame_and_undoes() {
        let mut s = session();
        let frame = serde_json::to_value(CanvasBackdrop::rounded_shadow_frame()).unwrap();
        let r = s
            .run("canvas.setBackdrop", &json!({ "backdrop": frame }))
            .unwrap();
        assert_eq!(r["frame_size"], json!({ "width": 296, "height": 196 }));
        assert_eq!(
            s.active().unwrap().manifest.canvas.backdrop,
            CanvasBackdrop::rounded_shadow_frame()
        );
        // Out-of-range margins fail validation and roll back.
        let mut bad_frame = frame.clone();
        bad_frame["margin_px"] = json!(-4);
        assert!(
            s.run("canvas.setBackdrop", &json!({ "backdrop": bad_frame }))
                .is_err()
        );
        assert!(
            s.run(
                "canvas.setBackdrop",
                &json!({ "backdrop": { "kind": "polaroid" } })
            )
            .is_err()
        );
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(
            s.active().unwrap().manifest.canvas.backdrop,
            CanvasBackdrop::None
        );
        s.run("edit.redo", &json!({})).unwrap();
        assert_eq!(
            s.active().unwrap().manifest.canvas.backdrop,
            CanvasBackdrop::rounded_shadow_frame()
        );
    }
}
