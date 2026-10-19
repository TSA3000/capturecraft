//! The command registry and the signature CaptureCraft commands: Step tool,
//! Callout with tails and Cut Out (straight or torn). The Smart Move,
//! Simplify and text commands live in [`crate::smart_commands`].
//!
//! Every command validates its parameters into a typed struct
//! (`deny_unknown_fields`, so an agent's typo is an error, not a silent
//! no-op) and returns `Result<Value>`; nothing here panics on input.

use capturecraft_format::{
    Annotation, AnnotationNode, Axis, BadgeShape, Callout, CalloutBody, CalloutTail, Color,
    CutOutSeam, DEFAULT_SEQUENCE, DropShadow, Fill, FontSpec, Manifest, NodeId, Numbering, RectF,
    SeamEdge, StepBadge, Stroke, TailAnchor, TextBlock,
};
use photocraft_geom::{Point, Rect};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use std::sync::LazyLock;

use crate::{EngineError, Result, Session, edit_commands, smart_commands, transform_commands};

pub(crate) type Run = fn(&mut Session, &Value) -> Result<Value>;
pub(crate) type Enabled = fn(&Session) -> std::result::Result<(), String>;

/// Metadata + implementation for one command.
#[derive(Clone, Copy)]
pub struct CommandSpec {
    /// Stable id: `area.verbObject` (`annotation.addStep`).
    pub id: &'static str,
    pub label: &'static str,
    /// Menu placement, e.g. `["Image", "Cut Out"]`. Empty = not in menus.
    pub menu_path: &'static [&'static str],
    /// Default shortcut in egui notation (`Cmd+Shift+Z`), mapped per platform by the UI.
    pub shortcut: Option<&'static str>,
    /// Human/agent-readable parameter description (JSON-ish); also the MCP tool description.
    pub params: &'static str,
    pub enabled: Enabled,
    pub run: Run,
    /// Recorded in undo history (false for queries and for undo/redo).
    pub journal: bool,
}

static REGISTRY: &[CommandSpec] = &[
    CommandSpec {
        id: "edit.undo",
        label: "Undo",
        menu_path: &["Edit"],
        shortcut: Some("Cmd+Z"),
        params: "{}",
        enabled: can_undo,
        run: |s, _| s.undo(),
        journal: false,
    },
    CommandSpec {
        id: "edit.redo",
        label: "Redo",
        menu_path: &["Edit"],
        shortcut: Some("Cmd+Shift+Z"),
        params: "{}",
        enabled: can_redo,
        run: |s, _| s.redo(),
        journal: false,
    },
    CommandSpec {
        id: "annotation.addStep",
        label: "Add Step",
        menu_path: &["Annotate", "Step"],
        shortcut: None,
        params: r##"{ at: {x, y}, sequence?: string = "default", value?: u32 (default: next in sequence),
  numbering?: "decimal"|"upper_alpha"|"lower_alpha"|"upper_roman"|"lower_roman",
  shape?: "circle"|"rounded_square"|"teardrop"|"shield", radius?: f32, pointer_deg?: f32,
  fill?: "#rrggbb[aa]", text_color?: "#rrggbb[aa]", ring?: Stroke, shadow?: DropShadow|null }
-> { id, value, label, next }"##,
        enabled: has_doc,
        run: add_step,
        journal: true,
    },
    CommandSpec {
        id: "annotation.renumberSteps",
        label: "Renumber Steps",
        menu_path: &["Annotate", "Step"],
        shortcut: None,
        params: r##"{ sequence?: string = "default", start?: u32 = 1 } -> { count }"##,
        enabled: has_doc,
        run: renumber_steps,
        journal: true,
    },
    CommandSpec {
        id: "annotation.addCallout",
        label: "Add Callout",
        menu_path: &["Annotate", "Callout"],
        shortcut: None,
        params: r##"{ bounds: {x, y, w, h}, body?: {kind: "rect"|"rounded_rect"|"ellipse"|"cloud"|"banner", ...},
  text?: string, font?: FontSpec, text_color?: color, tails?: [Tail], fill?: Fill, stroke?: Stroke|null,
  shadow?: DropShadow|null, fillet_px?: f32 }
Tail = { tip: {x, y}, base_width?: f32, curve_bias?: -1..1, anchor?: {kind: "nearest"|"center"|"edge", ...} }
-> { id }"##,
        enabled: has_doc,
        run: add_callout,
        journal: true,
    },
    CommandSpec {
        id: "annotation.addCalloutTail",
        label: "Add Callout Tail",
        menu_path: &[],
        shortcut: None,
        params: r##"{ id: u64, tail: Tail } -> { index }"##,
        enabled: has_doc,
        run: add_callout_tail,
        journal: true,
    },
    CommandSpec {
        id: "annotation.moveCalloutTail",
        label: "Move Callout Tail",
        menu_path: &[],
        shortcut: None,
        params: r##"{ id: u64, index: usize, tip: {x, y}, base_width?: f32 (the neck where it meets the body) }"##,
        enabled: has_doc,
        run: move_callout_tail,
        journal: true,
    },
    CommandSpec {
        id: "annotation.removeCalloutTail",
        label: "Remove Callout Tail",
        menu_path: &[],
        shortcut: None,
        params: r##"{ id: u64, index: usize }"##,
        enabled: has_doc,
        run: remove_callout_tail,
        journal: true,
    },
    CommandSpec {
        id: "canvas.cutOut",
        label: "Cut Out",
        menu_path: &["Image", "Cut Out"],
        shortcut: None,
        params: r##"{ axis: "horizontal"|"vertical", start: f64, end: f64 (canvas px; removes [start, end)),
  edge?: "straight"|"torn" = "straight", seed?: u64, amplitude_px?: f32 = 8, roughness?: 0..1 = 0.5,
  octaves?: 1..8 = 4, shadow?: DropShadow|null, gap_px?: f32 = 0 }
-> { id, removed, canvas: {width, height}, moved: [ids] }"##,
        enabled: has_doc,
        run: cut_out,
        journal: true,
    },
];

static ALL: LazyLock<Vec<CommandSpec>> = LazyLock::new(|| {
    REGISTRY
        .iter()
        .chain(edit_commands::REGISTRY)
        .chain(smart_commands::REGISTRY)
        .chain(transform_commands::REGISTRY)
        .chain(crate::node_commands::REGISTRY)
        .chain(crate::paint_commands::REGISTRY)
        .copied()
        .collect()
});

/// Every command, in registry order (menus and the MCP tool list use it).
pub fn registry() -> &'static [CommandSpec] {
    &ALL
}

// ---------------------------------------------------------------------------
// Enabled checks

pub(crate) fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active()
        .map(|_| ())
        .ok_or_else(|| "no document open".into())
}

fn can_undo(s: &Session) -> std::result::Result<(), String> {
    s.active()
        .filter(|d| d.can_undo())
        .map(|_| ())
        .ok_or_else(|| "nothing to undo".into())
}

fn can_redo(s: &Session) -> std::result::Result<(), String> {
    s.active()
        .filter(|d| d.can_redo())
        .map(|_| ())
        .ok_or_else(|| "nothing to redo".into())
}

// ---------------------------------------------------------------------------
// Helpers

/// `null` means "no parameters".
pub(crate) fn parse<T: DeserializeOwned>(cmd: &str, p: &Value) -> Result<T> {
    let p = if p.is_null() { &json!({}) } else { p };
    T::deserialize(p).map_err(|e| bad(cmd, e))
}

pub(crate) fn bad(cmd: &str, msg: impl std::fmt::Display) -> EngineError {
    EngineError::BadParams {
        cmd: cmd.to_string(),
        msg: msg.to_string(),
    }
}

pub(crate) fn push_node(m: &mut Manifest, annotation: Annotation) -> NodeId {
    let id = m.alloc_node_id();
    m.annotations.push(AnnotationNode::new(id, annotation));
    id
}

pub(crate) fn node_annotation(m: &mut Manifest, id: u64) -> Result<&mut AnnotationNode> {
    let node = m.node_mut(NodeId(id)).ok_or(EngineError::NoNode(id))?;
    if node.locked {
        return Err(EngineError::Other(format!("annotation {id} is locked")));
    }
    Ok(node)
}

fn callout_mut<'a>(cmd: &str, node: &'a mut AnnotationNode) -> Result<&'a mut Callout> {
    let id = node.id.0;
    match node.annotation_mut() {
        Some(Annotation::Callout(c)) => Ok(c),
        _ => Err(bad(cmd, format!("annotation {id} is not a callout"))),
    }
}

/// Absent = default, `null` = off: for `Option<Stroke>`/`Option<DropShadow>`
/// style fields where "not given" and "none" differ.
fn double_option<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

// ---------------------------------------------------------------------------
// Step tool

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddStepParams {
    pub at: Point,
    #[serde(default)]
    pub sequence: Option<String>,
    #[serde(default)]
    pub value: Option<u32>,
    #[serde(default)]
    pub numbering: Option<Numbering>,
    #[serde(default)]
    pub shape: Option<BadgeShape>,
    #[serde(default)]
    pub radius: Option<f32>,
    #[serde(default)]
    pub pointer_deg: Option<f32>,
    #[serde(default)]
    pub fill: Option<Color>,
    #[serde(default)]
    pub text_color: Option<Color>,
    #[serde(default, deserialize_with = "double_option")]
    pub ring: Option<Option<Stroke>>,
    #[serde(default, deserialize_with = "double_option")]
    pub shadow: Option<Option<DropShadow>>,
}

/// Steps of one sequence, in document order.
fn steps_in<'a>(m: &'a Manifest, sequence: &'a str) -> impl Iterator<Item = &'a StepBadge> + 'a {
    m.annotations
        .iter()
        .filter_map(move |n| match n.annotation() {
            Some(Annotation::StepBadge(b)) if b.sequence == sequence => Some(b),
            _ => None,
        })
}

/// The value the Step tool places next: one past the highest in the sequence.
pub fn next_step_value(m: &Manifest, sequence: &str) -> u32 {
    steps_in(m, sequence)
        .map(|b| b.value)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn add_step(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.addStep";
    let p: AddStepParams = parse(CMD, p)?;
    let m = s.manifest_mut()?;
    let sequence = p.sequence.unwrap_or_else(|| DEFAULT_SEQUENCE.into());
    // The style follows the sequence's last badge, so a sequence stays uniform.
    let template = steps_in(m, &sequence).last().cloned().unwrap_or_default();
    let value = match p.value {
        Some(0) => return Err(bad(CMD, "value starts at 1")),
        Some(v) => v,
        None => next_step_value(m, &sequence),
    };
    let badge = StepBadge {
        center: p.at,
        radius: p.radius.unwrap_or(template.radius),
        shape: p.shape.unwrap_or(template.shape),
        pointer_deg: p.pointer_deg.or(template.pointer_deg),
        sequence: sequence.clone(),
        value,
        numbering: p.numbering.unwrap_or(template.numbering),
        fill: p.fill.unwrap_or(template.fill),
        text_color: p.text_color.unwrap_or(template.text_color),
        font: template.font,
        ring: p.ring.unwrap_or(template.ring),
        shadow: p.shadow.unwrap_or(template.shadow),
    };
    let label = badge.label();
    let id = push_node(m, Annotation::StepBadge(badge));
    Ok(json!({ "id": id.0, "value": value, "label": label, "next": next_step_value(m, &sequence) }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenumberStepsParams {
    #[serde(default)]
    pub sequence: Option<String>,
    #[serde(default)]
    pub start: Option<u32>,
}

fn renumber_steps(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.renumberSteps";
    let p: RenumberStepsParams = parse(CMD, p)?;
    let sequence = p.sequence.unwrap_or_else(|| DEFAULT_SEQUENCE.into());
    let mut next = p.start.unwrap_or(1);
    if next == 0 {
        return Err(bad(CMD, "start must be at least 1"));
    }
    let m = s.manifest_mut()?;
    let mut count = 0u32;
    for n in &mut m.annotations {
        let changed = match n.annotation_mut() {
            Some(Annotation::StepBadge(b)) if b.sequence == sequence => {
                let changed = b.value != next;
                b.value = next;
                next = next.saturating_add(1);
                count += 1;
                changed
            }
            _ => false,
        };
        if changed {
            n.touch();
        }
    }
    Ok(json!({ "count": count }))
}

// ---------------------------------------------------------------------------
// Callout with tails

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TailParams {
    pub tip: Point,
    #[serde(default)]
    pub base_width: Option<f32>,
    #[serde(default)]
    pub curve_bias: Option<f32>,
    #[serde(default)]
    pub anchor: Option<TailAnchor>,
}

impl TailParams {
    fn into_tail(self) -> CalloutTail {
        let d = CalloutTail::default();
        CalloutTail {
            tip: self.tip,
            base_width: self.base_width.unwrap_or(d.base_width),
            curve_bias: self.curve_bias.unwrap_or(d.curve_bias),
            anchor: self.anchor.unwrap_or(d.anchor),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddCalloutParams {
    pub bounds: RectF,
    #[serde(default)]
    pub body: Option<CalloutBody>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub font: Option<FontSpec>,
    #[serde(default)]
    pub text_color: Option<Color>,
    #[serde(default)]
    pub tails: Vec<TailParams>,
    #[serde(default)]
    pub fill: Option<Fill>,
    #[serde(default, deserialize_with = "double_option")]
    pub stroke: Option<Option<Stroke>>,
    #[serde(default, deserialize_with = "double_option")]
    pub shadow: Option<Option<DropShadow>>,
    #[serde(default)]
    pub fillet_px: Option<f32>,
}

fn add_callout(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.addCallout";
    let p: AddCalloutParams = parse(CMD, p)?;
    if !(p.bounds.w >= 1.0 && p.bounds.h >= 1.0) {
        return Err(bad(CMD, "bounds must be at least 1×1"));
    }
    let d = Callout::default();
    // Without text, a font or colour still lands in an empty run: the
    // style the callout's text gets once it is typed.
    let text = match p.text {
        None if p.font.is_none() && p.text_color.is_none() => d.text,
        t => TextBlock::plain(
            &t.unwrap_or_default(),
            p.font.unwrap_or_default(),
            p.text_color.unwrap_or(Color::BLACK),
        ),
    };
    let callout = Callout {
        bounds: p.bounds,
        body: p.body.unwrap_or(d.body),
        tails: p.tails.into_iter().map(TailParams::into_tail).collect(),
        fill: p.fill.unwrap_or(d.fill),
        stroke: p.stroke.unwrap_or(d.stroke),
        shadow: p.shadow.unwrap_or(d.shadow),
        text,
        fillet_px: p.fillet_px.unwrap_or(d.fillet_px),
    };
    // Range checks (tail count, curve bias, NaN) are the format's: the
    // session validates the whole document after the command and rolls
    // back with the offending path.
    let id = push_node(s.manifest_mut()?, Annotation::Callout(callout));
    Ok(json!({ "id": id.0 }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddCalloutTailParams {
    pub id: u64,
    pub tail: TailParams,
}

fn add_callout_tail(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.addCalloutTail";
    let p: AddCalloutTailParams = parse(CMD, p)?;
    let node = node_annotation(s.manifest_mut()?, p.id)?;
    let c = callout_mut(CMD, node)?;
    c.tails.push(p.tail.into_tail());
    let index = c.tails.len() - 1;
    node.touch();
    Ok(json!({ "index": index }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveCalloutTailParams {
    pub id: u64,
    pub index: usize,
    pub tip: Point,
    #[serde(default)]
    pub base_width: Option<f32>,
}

fn move_callout_tail(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.moveCalloutTail";
    let p: MoveCalloutTailParams = parse(CMD, p)?;
    let node = node_annotation(s.manifest_mut()?, p.id)?;
    let c = callout_mut(CMD, node)?;
    let tail = c
        .tails
        .get_mut(p.index)
        .ok_or_else(|| bad(CMD, format!("callout {} has no tail {}", p.id, p.index)))?;
    tail.tip = p.tip;
    if let Some(w) = p.base_width {
        tail.base_width = w;
    }
    node.touch();
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveCalloutTailParams {
    pub id: u64,
    pub index: usize,
}

fn remove_callout_tail(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.removeCalloutTail";
    let p: RemoveCalloutTailParams = parse(CMD, p)?;
    let node = node_annotation(s.manifest_mut()?, p.id)?;
    let c = callout_mut(CMD, node)?;
    if p.index >= c.tails.len() {
        return Err(bad(
            CMD,
            format!("callout {} has no tail {}", p.id, p.index),
        ));
    }
    c.tails.remove(p.index);
    node.touch();
    Ok(json!({}))
}

// ---------------------------------------------------------------------------
// Cut Out

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeParam {
    #[default]
    Straight,
    Torn,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutOutParams {
    pub axis: Axis,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub edge: EdgeParam,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub amplitude_px: Option<f32>,
    #[serde(default)]
    pub roughness: Option<f32>,
    #[serde(default)]
    pub octaves: Option<u8>,
    #[serde(default, deserialize_with = "double_option")]
    pub shadow: Option<Option<DropShadow>>,
    #[serde(default)]
    pub gap_px: Option<f32>,
}

/// Length of the base raster along `axis` after the cut outs so far.
fn cut_extent(m: &Manifest, axis: Axis) -> f64 {
    let full = match axis {
        Axis::Horizontal => m.base.size.height,
        Axis::Vertical => m.base.size.width,
    };
    let removed: f64 = cut_outs(m)
        .filter(|c| c.axis == axis)
        .map(CutOutSeam::removed)
        .sum();
    (f64::from(full) - removed).max(0.0)
}

pub(crate) fn cut_outs(m: &Manifest) -> impl Iterator<Item = &CutOutSeam> {
    m.annotations.iter().filter_map(|n| match n.annotation() {
        Some(Annotation::CutOutSeam(c)) => Some(c),
        _ => None,
    })
}

/// SplitMix64: a deterministic default seed per node.
fn mix_seed(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn cut_out(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "canvas.cutOut";
    let p: CutOutParams = parse(CMD, p)?;
    if !(p.start.is_finite() && p.end.is_finite()) || p.end <= p.start {
        return Err(bad(CMD, "need finite start < end"));
    }
    let m = s.manifest_mut()?;
    // Canvas → base space (the space left by earlier cuts).
    let origin = match p.axis {
        Axis::Horizontal => f64::from(m.canvas.base_origin.y),
        Axis::Vertical => f64::from(m.canvas.base_origin.x),
    };
    let (start, end) = (p.start - origin, p.end - origin);
    let extent = cut_extent(m, p.axis);
    if start < 0.0 || end > extent {
        return Err(bad(
            CMD,
            format!("the cut must lie on the capture (0..{extent} along this axis)"),
        ));
    }
    let removed = end - start;
    if removed >= extent {
        return Err(bad(CMD, "a cut out cannot remove the whole capture"));
    }
    let id = m.alloc_node_id();
    let edge = match p.edge {
        EdgeParam::Straight => SeamEdge::Straight,
        EdgeParam::Torn => SeamEdge::Torn {
            seed: p.seed.unwrap_or_else(|| mix_seed(id.0)),
            amplitude_px: p.amplitude_px.unwrap_or(8.0),
            roughness: p.roughness.unwrap_or(0.5),
            octaves: p.octaves.unwrap_or(4),
        },
    };
    let default_shadow = matches!(edge, SeamEdge::Torn { .. }).then(DropShadow::default);
    let seam = CutOutSeam {
        axis: p.axis,
        start,
        end,
        edge,
        shadow: p.shadow.unwrap_or(default_shadow),
        gap_px: p.gap_px.unwrap_or(0.0),
    };

    // Annotations past the strip move with the far half; those inside it
    // land on the seam.
    let mut moved = Vec::new();
    for n in &mut m.annotations {
        let Some(a) = n.annotation_mut() else {
            continue;
        };
        let Some(b) = a.bounds() else { continue };
        let c = match p.axis {
            Axis::Horizontal => b.center().y,
            Axis::Vertical => b.center().x,
        };
        let shift = if c >= p.end {
            -removed
        } else if c > p.start {
            p.start - c
        } else {
            continue;
        };
        match p.axis {
            Axis::Horizontal => a.translate(0.0, shift),
            Axis::Vertical => a.translate(shift, 0.0),
        }
        n.touch();
        moved.push(n.id.0);
    }
    let cut_px = removed.round() as u32;
    let size = &mut m.canvas.size;
    match p.axis {
        Axis::Horizontal => size.height = size.height.saturating_sub(cut_px).max(1),
        Axis::Vertical => size.width = size.width.saturating_sub(cut_px).max(1),
    }
    m.annotations
        .push(AnnotationNode::new(id, Annotation::CutOutSeam(seam)));
    let canvas = m.canvas.size;
    Ok(json!({
        "id": id.0,
        "removed": removed,
        "canvas": { "width": canvas.width, "height": canvas.height },
        "moved": moved,
    }))
}

/// Maps a base-space rectangle to canvas space through the cut outs;
/// `None` when it lies entirely inside removed strips.
pub fn base_rect_to_canvas(m: &Manifest, r: Rect) -> Option<RectF> {
    let (mut x0, mut x1) = (f64::from(r.x0), f64::from(r.x1));
    let (mut y0, mut y1) = (f64::from(r.y0), f64::from(r.y1));
    for c in cut_outs(m) {
        let map = |v: f64| {
            if v <= c.start {
                v
            } else if v >= c.end {
                v - c.removed()
            } else {
                c.start
            }
        };
        match c.axis {
            Axis::Horizontal => (y0, y1) = (map(y0), map(y1)),
            Axis::Vertical => (x0, x1) = (map(x0), map(x1)),
        }
    }
    let o = m.canvas.base_origin;
    (x1 > x0 && y1 > y0)
        .then(|| RectF::new(x0 + f64::from(o.x), y0 + f64::from(o.y), x1 - x0, y1 - y0))
}

/// Maps a canvas-space rectangle back to the base raster, undoing the cut
/// outs (newest first), rounded outward and clipped to the base; `None`
/// when no base pixel is inside. A selection edge on a seam stays on the
/// visible side of it.
pub fn canvas_rect_to_base(m: &Manifest, r: RectF) -> Option<Rect> {
    let o = m.canvas.base_origin;
    let (mut x0, mut x1) = (r.x - f64::from(o.x), r.x + r.w - f64::from(o.x));
    let (mut y0, mut y1) = (r.y - f64::from(o.y), r.y + r.h - f64::from(o.y));
    let cuts: Vec<_> = cut_outs(m).collect();
    for c in cuts.into_iter().rev() {
        let lo = |v: f64| if v >= c.start { v + c.removed() } else { v };
        let hi = |v: f64| if v > c.start { v + c.removed() } else { v };
        match c.axis {
            Axis::Horizontal => (y0, y1) = (lo(y0), hi(y1)),
            Axis::Vertical => (x0, x1) = (lo(x0), hi(x1)),
        }
    }
    let px = |v: f64| {
        if v.is_finite() {
            v.clamp(-1e9, 1e9) as i32
        } else {
            0
        }
    };
    let b = Rect::new(px(x0.floor()), px(y0.floor()), px(x1.ceil()), px(y1.ceil()))
        .intersect(&Rect::from_size(m.base.size));
    (!b.is_empty()).then_some(b)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::Pixels;
    use photocraft_geom::Size;

    fn session(w: u32, h: u32) -> Session {
        let mut s = Session::new();
        let px = Pixels::new(Size::new(w, h), vec![255; (w * h * 4) as usize]);
        s.open(Manifest::new(Size::new(w, h)), px).unwrap();
        s
    }

    fn manifest(s: &Session) -> &Manifest {
        &s.active().unwrap().manifest
    }

    #[test]
    fn canvas_rects_map_back_to_the_base() {
        let mut s = session(100, 80);
        let r = |x, y, w, h| RectF::new(x, y, w, h);
        let m = manifest(&s);
        assert_eq!(
            canvas_rect_to_base(m, r(10.2, 5.0, 20.0, 9.5)),
            Some(Rect::new(10, 5, 31, 15))
        );
        assert_eq!(canvas_rect_to_base(m, r(200.0, 0.0, 5.0, 5.0)), None);
        // A 20 px horizontal strip removed at y = 30.
        s.run(
            "canvas.cutOut",
            &serde_json::json!({ "axis": "horizontal", "start": 30, "end": 50 }),
        )
        .unwrap();
        let m = manifest(&s);
        assert_eq!(
            canvas_rect_to_base(m, r(0.0, 25.0, 10.0, 10.0)),
            Some(Rect::new(0, 25, 10, 55))
        );
        assert_eq!(
            canvas_rect_to_base(m, r(0.0, 30.0, 10.0, 5.0)),
            Some(Rect::new(0, 50, 10, 55))
        );
        let back = base_rect_to_canvas(m, Rect::new(0, 50, 10, 55)).unwrap();
        assert_eq!((back.y, back.h), (30.0, 5.0));
    }

    #[test]
    fn registry_ids_are_unique() {
        let mut ids: Vec<_> = registry().iter().map(|c| c.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn steps_auto_increment_per_sequence() {
        let mut s = session(400, 300);
        for i in 1..=3 {
            let r = s.run(
                "annotation.addStep",
                &json!({ "at": { "x": 10.0 * f64::from(i), "y": 10.0 } }),
            );
            assert_eq!(r.unwrap()["value"], i);
        }
        let r = s
            .run(
                "annotation.addStep",
                &json!({ "at": { "x": 5, "y": 5 }, "sequence": "b", "numbering": "upper_alpha" }),
            )
            .unwrap();
        assert_eq!(r["label"], "A");
        // The sequence keeps its style.
        let r = s
            .run(
                "annotation.addStep",
                &json!({ "at": { "x": 6, "y": 6 }, "sequence": "b" }),
            )
            .unwrap();
        assert_eq!(r["label"], "B");
        assert_eq!(next_step_value(manifest(&s), DEFAULT_SEQUENCE), 4);
    }

    #[test]
    fn renumber_closes_gaps() {
        let mut s = session(100, 100);
        for v in [3, 7, 9] {
            s.run(
                "annotation.addStep",
                &json!({ "at": { "x": 1, "y": 1 }, "value": v }),
            )
            .unwrap();
        }
        let r = s.run("annotation.renumberSteps", &json!({})).unwrap();
        assert_eq!(r["count"], 3);
        assert_eq!(next_step_value(manifest(&s), DEFAULT_SEQUENCE), 4);
    }

    #[test]
    fn bad_params_change_nothing() {
        let mut s = session(100, 100);
        let before = manifest(&s).clone();
        let bad = [
            (
                "annotation.addStep",
                json!({ "at": { "x": "left", "y": 1 } }),
            ),
            (
                "annotation.addStep",
                json!({ "at": { "x": 1, "y": 1 }, "colour": "#fff" }),
            ),
            (
                "annotation.addStep",
                json!({ "at": { "x": 1, "y": 1 }, "radius": -4 }),
            ),
            (
                "annotation.addCallout",
                json!({ "bounds": { "x": 0, "y": 0, "w": 0, "h": 5 } }),
            ),
            (
                "annotation.addCallout",
                json!({ "bounds": { "x": 0, "y": 0, "w": 9, "h": 5 },
                "tails": [{ "tip": { "x": 1, "y": 1 }, "curve_bias": 3 }] }),
            ),
            (
                "annotation.moveCalloutTail",
                json!({ "id": 99, "index": 0, "tip": { "x": 1, "y": 1 } }),
            ),
            (
                "canvas.cutOut",
                json!({ "axis": "horizontal", "start": 50, "end": 40 }),
            ),
            (
                "canvas.cutOut",
                json!({ "axis": "horizontal", "start": 0, "end": 100 }),
            ),
            (
                "canvas.cutOut",
                json!({ "axis": "vertical", "start": 90, "end": 120 }),
            ),
            (
                "canvas.cutOut",
                json!({ "axis": "vertical", "start": 10, "end": 20, "edge": "torn", "octaves": 0 }),
            ),
            ("nope.nothing", json!({})),
        ];
        for (cmd, p) in bad {
            assert!(s.run(cmd, &p).is_err(), "{cmd} {p}");
            assert_eq!(manifest(&s), &before, "{cmd} {p}");
        }
        assert!(!s.active().unwrap().can_undo());
    }

    #[test]
    fn callout_tails_add_move_remove_undo() {
        let mut s = session(400, 300);
        let r = s
            .run(
                "annotation.addCallout",
                &json!({
                    "bounds": { "x": 20, "y": 20, "w": 160, "h": 80 },
                    "text": "Click here",
                    "tails": [{ "tip": { "x": 100, "y": 200 } }]
                }),
            )
            .unwrap();
        let id = r["id"].as_u64().unwrap();
        let r = s
            .run(
                "annotation.addCalloutTail",
                &json!({ "id": id, "tail": { "tip": { "x": 300, "y": 60 }, "curve_bias": -0.5 } }),
            )
            .unwrap();
        assert_eq!(r["index"], 1);
        s.run(
            "annotation.moveCalloutTail",
            &json!({ "id": id, "index": 1, "tip": { "x": 320, "y": 70 } }),
        )
        .unwrap();
        s.run(
            "annotation.removeCalloutTail",
            &json!({ "id": id, "index": 0 }),
        )
        .unwrap();
        let tails = |s: &Session| match manifest(s)
            .node(NodeId(id))
            .and_then(AnnotationNode::annotation)
        {
            Some(Annotation::Callout(c)) => c.tails.iter().map(|t| t.tip.x).collect::<Vec<_>>(),
            _ => panic!("callout expected"),
        };
        assert_eq!(tails(&s), [320.0]);
        s.run("edit.undo", &Value::Null).unwrap();
        assert_eq!(tails(&s), [100.0, 320.0]);
        s.run("edit.redo", &Value::Null).unwrap();
        assert_eq!(tails(&s), [320.0]);
        // The neck handle sends the width along with the unchanged tip.
        s.run(
            "annotation.moveCalloutTail",
            &json!({ "id": id, "index": 0, "tip": { "x": 320, "y": 70 }, "base_width": 40 }),
        )
        .unwrap();
        let Some(Annotation::Callout(c)) = manifest(&s)
            .node(NodeId(id))
            .and_then(AnnotationNode::annotation)
        else {
            panic!("callout expected")
        };
        assert_eq!(c.tails[0].base_width, 40.0);
        assert!(
            s.run(
                "annotation.moveCalloutTail",
                &json!({ "id": id, "index": 0, "tip": { "x": 320, "y": 70 }, "base_width": -1 }),
            )
            .is_err(),
            "a negative neck is rejected"
        );
    }

    #[test]
    fn cut_out_shrinks_canvas_and_moves_annotations() {
        let mut s = session(200, 400);
        s.run("annotation.addStep", &json!({ "at": { "x": 50, "y": 50 } }))
            .unwrap();
        s.run(
            "annotation.addStep",
            &json!({ "at": { "x": 50, "y": 300 } }),
        )
        .unwrap();
        s.run(
            "annotation.addStep",
            &json!({ "at": { "x": 50, "y": 150 } }),
        )
        .unwrap();
        let r = s
            .run(
                "canvas.cutOut",
                &json!({ "axis": "horizontal", "start": 100, "end": 200, "edge": "torn" }),
            )
            .unwrap();
        assert_eq!(r["canvas"]["height"], 300);
        let ys: Vec<f64> = manifest(&s)
            .annotations
            .iter()
            .filter_map(|n| match n.annotation() {
                Some(Annotation::StepBadge(b)) => Some(b.center.y),
                _ => None,
            })
            .collect();
        assert_eq!(ys, [50.0, 200.0, 100.0]);
        let seam = manifest(&s)
            .annotations
            .iter()
            .find_map(|n| match n.annotation() {
                Some(Annotation::CutOutSeam(c)) => Some(c.clone()),
                _ => None,
            });
        let seam = seam.unwrap();
        assert!(matches!(seam.edge, SeamEdge::Torn { .. }) && seam.shadow.is_some());
        // A second cut is measured in the remaining 300 px.
        assert!(
            s.run(
                "canvas.cutOut",
                &json!({ "axis": "horizontal", "start": 250, "end": 310 })
            )
            .is_err()
        );
        assert!(
            s.run(
                "canvas.cutOut",
                &json!({ "axis": "horizontal", "start": 250, "end": 300 })
            )
            .is_ok()
        );
        // Base rows 330..350 sit below the first cut and above the second.
        assert!(base_rect_to_canvas(manifest(&s), Rect::new(0, 380, 10, 400)).is_none());
        let r = base_rect_to_canvas(manifest(&s), Rect::new(0, 330, 10, 350)).unwrap();
        assert_eq!((r.y, r.h), (230.0, 20.0));
        assert!(base_rect_to_canvas(manifest(&s), Rect::new(0, 120, 10, 180)).is_none());
    }
}
