//! Commands on annotation nodes as objects: delete, arrange (z-order),
//! layer properties (visibility, lock, name), groups, copy/paste and
//! duplicate; the Text, Magnify and Spotlight tools; Crop; and jumping
//! through the undo history (the History panel).
//!
//! Like [`crate::commands`], every command parses into a typed struct
//! (`deny_unknown_fields`) and never panics on input; the session validates
//! the document afterwards and rolls back on any error.

use std::collections::BTreeMap;

use capturecraft_format::{
    Annotation, AnnotationNode, Callout, CalloutBody, Color, FontSpec, LensShape, MagnifierLens,
    Manifest, NodeId, RectF, Spotlight, TextAlign, TextBlock, VAlign,
};
use photocraft_geom::Point;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, has_doc, node_annotation, parse, push_node};
use crate::edit_commands::RegionKind;
use crate::{EngineError, Result, Session};

pub(crate) static REGISTRY: &[CommandSpec] = &[
    CommandSpec {
        id: "annotation.delete",
        label: "Delete",
        menu_path: &["Edit"],
        shortcut: Some("Delete"),
        params: r##"{ ids: [u64] } (locked nodes are kept) -> { deleted: [ids] }"##,
        enabled: has_doc,
        run: delete,
        journal: true,
    },
    CommandSpec {
        id: "annotation.arrange",
        label: "Arrange",
        menu_path: &["Arrange"],
        shortcut: None,
        params: r##"{ ids: [u64], to: "front"|"back"|"forward"|"backward" } -> { order: [ids] (bottom first) }"##,
        enabled: has_doc,
        run: arrange,
        journal: true,
    },
    CommandSpec {
        id: "annotation.reorder",
        label: "Reorder Layer",
        menu_path: &[],
        shortcut: None,
        params: r##"{ id: u64, index: usize (0 = bottom; clamped) } -> { order: [ids] }"##,
        enabled: has_doc,
        run: reorder,
        journal: true,
    },
    CommandSpec {
        id: "annotation.setProps",
        label: "Layer Properties",
        menu_path: &[],
        shortcut: None,
        params: r##"{ ids: [u64], visible?: bool, locked?: bool, name?: string (≤ 64 bytes) } -> { changed: [ids] }"##,
        enabled: has_doc,
        run: set_props,
        journal: true,
    },
    CommandSpec {
        id: "annotation.group",
        label: "Group",
        menu_path: &["Arrange"],
        shortcut: Some("Cmd+G"),
        params: r##"{ ids: [u64] (2 or more) } -> { group }"##,
        enabled: has_doc,
        run: group,
        journal: true,
    },
    CommandSpec {
        id: "annotation.ungroup",
        label: "Ungroup",
        menu_path: &["Arrange"],
        shortcut: Some("Cmd+Shift+G"),
        params: r##"{ ids: [u64] } -> { ungrouped: [ids] }"##,
        enabled: has_doc,
        run: ungroup,
        journal: true,
    },
    CommandSpec {
        id: "annotation.paste",
        label: "Paste",
        menu_path: &["Edit"],
        shortcut: Some("Cmd+V"),
        params: r##"{ nodes: [AnnotationNode] (as in manifest.json; smart objects and cut outs are skipped),
  dx?: f64 = 0, dy?: f64 = 0 } -> { ids }"##,
        enabled: has_doc,
        run: paste,
        journal: true,
    },
    CommandSpec {
        id: "annotation.duplicate",
        label: "Duplicate",
        menu_path: &["Edit"],
        shortcut: Some("Cmd+D"),
        params: r##"{ ids: [u64], dx?: f64 = 16, dy?: f64 = 16 } (smart objects keep their pixels) -> { ids }"##,
        enabled: has_doc,
        run: duplicate,
        journal: true,
    },
    CommandSpec {
        id: "annotation.addText",
        label: "Add Text",
        menu_path: &["Annotate", "Text"],
        shortcut: None,
        params: r##"{ bounds: {x, y, w, h}, text?: string = "Text", font?: FontSpec, color?: "#rrggbb[aa]",
  align?: "left"|"center"|"right"|"justify" = "left", shadow?: DropShadow|null, fill?: Fill = none,
  stroke?: Stroke|null = null, underline?: bool = false, valign?: "top"|"middle"|"bottom" = "top",
  padding?: f32 = 4 } -> { id }"##,
        enabled: has_doc,
        run: add_text,
        journal: true,
    },
    CommandSpec {
        id: "annotation.setText",
        label: "Edit Text",
        menu_path: &[],
        shortcut: None,
        params: r##"{ id: u64 (a text box or callout), text?: string, align?: "left"|"center"|"right"|"justify" }
  (keeps the first run's font and colour) -> {}"##,
        enabled: has_doc,
        run: set_text,
        journal: true,
    },
    CommandSpec {
        id: "annotation.setTextFormat",
        label: "Format Text",
        menu_path: &[],
        shortcut: None,
        params: r##"{ ids: [u64] (text boxes or callouts), family?: string, size?: f32, bold?: bool, italic?: bool,
  underline?: bool, color?: "#rrggbb[aa]", align?: "left"|"center"|"right"|"justify",
  valign?: "top"|"middle"|"bottom", padding?: f32 } (font fields apply to every run) -> { formatted: [ids] }"##,
        enabled: has_doc,
        run: set_text_format,
        journal: true,
    },
    CommandSpec {
        id: "annotation.addMagnifier",
        label: "Add Magnifier",
        menu_path: &["Annotate", "Magnify"],
        shortcut: None,
        params: r##"{ center: {x, y}, radius?: f32 = 80, zoom?: f32 = 2, source_center?: {x, y} (default: center),
  border?: Stroke|null } -> { id }"##,
        enabled: has_doc,
        run: add_magnifier,
        journal: true,
    },
    CommandSpec {
        id: "annotation.addSpotlight",
        label: "Add Spotlight",
        menu_path: &["Annotate", "Spotlight"],
        shortcut: None,
        params: r##"{ bounds: {x, y, w, h}, kind?: "rect"|"ellipse" = "rect", dim_opacity?: 0..1 = 0.55,
  blur_sigma?: f32 } -> { id }"##,
        enabled: has_doc,
        run: add_spotlight,
        journal: true,
    },
    CommandSpec {
        id: "canvas.crop",
        label: "Crop",
        menu_path: &["Image"],
        shortcut: None,
        params: r##"{ rect: {x, y, w, h} (canvas px, rounded outward to whole pixels) }
-> { canvas: {width, height}, base_origin: {x, y} }"##,
        enabled: has_doc,
        run: crop,
        journal: true,
    },
    CommandSpec {
        id: "edit.jump",
        label: "Go to History State",
        menu_path: &[],
        shortcut: None,
        params: r##"{ steps: i32 } (negative: undo that many, positive: redo) -> { undone|redone: [labels] }"##,
        enabled: has_doc,
        run: jump,
        journal: false,
    },
];

/// Sorted, without repeats.
fn unique(mut ids: Vec<u64>) -> Vec<u64> {
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn order(m: &Manifest) -> Value {
    json!(m.annotations.iter().map(|n| n.id.0).collect::<Vec<_>>())
}

fn check_exist(cmd: &str, m: &Manifest, ids: &[u64]) -> Result<()> {
    if ids.is_empty() {
        return Err(bad(cmd, "ids is empty"));
    }
    match ids.iter().find(|id| m.node(NodeId(**id)).is_none()) {
        Some(id) => Err(EngineError::NoNode(*id)),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Delete, arrange, reorder

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdsParams {
    ids: Vec<u64>,
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.delete";
    let p: IdsParams = parse(CMD, p)?;
    let ids = unique(p.ids);
    let m = s.manifest_mut()?;
    check_exist(CMD, m, &ids)?;
    let gone: Vec<u64> = m
        .annotations
        .iter()
        .filter(|n| ids.contains(&n.id.0) && !n.locked)
        .map(|n| n.id.0)
        .collect();
    m.annotations.retain(|n| !gone.contains(&n.id.0));
    Ok(json!({ "deleted": gone }))
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ArrangeTo {
    Front,
    Back,
    Forward,
    Backward,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArrangeParams {
    ids: Vec<u64>,
    to: ArrangeTo,
}

fn arrange(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.arrange";
    let p: ArrangeParams = parse(CMD, p)?;
    let ids = unique(p.ids);
    let m = s.manifest_mut()?;
    check_exist(CMD, m, &ids)?;
    let picked = |n: &AnnotationNode| ids.contains(&n.id.0);
    let v = &mut m.annotations;
    match p.to {
        ArrangeTo::Front | ArrangeTo::Back => {
            // Stable: the picked nodes keep their order among themselves.
            let (mut sel, rest): (Vec<_>, Vec<_>) = v.drain(..).partition(|n| picked(n));
            *v = if matches!(p.to, ArrangeTo::Front) {
                let mut r = rest;
                r.append(&mut sel);
                r
            } else {
                sel.extend(rest);
                sel
            };
        }
        ArrangeTo::Forward => {
            for i in (0..v.len().saturating_sub(1)).rev() {
                if picked(&v[i]) && !picked(&v[i + 1]) {
                    v.swap(i, i + 1);
                }
            }
        }
        ArrangeTo::Backward => {
            for i in 1..v.len() {
                if picked(&v[i]) && !picked(&v[i - 1]) {
                    v.swap(i, i - 1);
                }
            }
        }
    }
    Ok(json!({ "order": order(m) }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReorderParams {
    id: u64,
    index: usize,
}

fn reorder(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.reorder";
    let p: ReorderParams = parse(CMD, p)?;
    let m = s.manifest_mut()?;
    let from = m
        .annotations
        .iter()
        .position(|n| n.id.0 == p.id)
        .ok_or(EngineError::NoNode(p.id))?;
    let node = m.annotations.remove(from);
    let to = p.index.min(m.annotations.len());
    m.annotations.insert(to, node);
    Ok(json!({ "order": order(m) }))
}

// ---------------------------------------------------------------------------
// Layer properties and groups

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PropsParams {
    ids: Vec<u64>,
    #[serde(default)]
    visible: Option<bool>,
    #[serde(default)]
    locked: Option<bool>,
    #[serde(default)]
    name: Option<String>,
}

fn set_props(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.setProps";
    let p: PropsParams = parse(CMD, p)?;
    if p.name.as_ref().is_some_and(|n| n.len() > 64) {
        return Err(bad(CMD, "name is longer than 64 bytes"));
    }
    let ids = unique(p.ids);
    let m = s.manifest_mut()?;
    check_exist(CMD, m, &ids)?;
    // Locked nodes take property changes (that is how they unlock).
    for n in m.annotations.iter_mut().filter(|n| ids.contains(&n.id.0)) {
        if let Some(v) = p.visible {
            n.visible = v;
        }
        if let Some(l) = p.locked {
            n.locked = l;
        }
        if let Some(name) = &p.name {
            n.name = name.trim().to_string();
        }
        n.touch();
    }
    Ok(json!({ "changed": ids }))
}

fn next_group(m: &Manifest) -> u64 {
    m.annotations
        .iter()
        .filter_map(|n| n.group)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn group(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.group";
    let p: IdsParams = parse(CMD, p)?;
    let ids = unique(p.ids);
    if ids.len() < 2 {
        return Err(bad(CMD, "a group needs at least two annotations"));
    }
    let m = s.manifest_mut()?;
    check_exist(CMD, m, &ids)?;
    let g = next_group(m);
    for n in m.annotations.iter_mut().filter(|n| ids.contains(&n.id.0)) {
        n.group = Some(g);
    }
    Ok(json!({ "group": g }))
}

fn ungroup(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.ungroup";
    let p: IdsParams = parse(CMD, p)?;
    let ids = unique(p.ids);
    let m = s.manifest_mut()?;
    check_exist(CMD, m, &ids)?;
    // Ungrouping one member dissolves its whole group.
    let groups: Vec<u64> = m
        .annotations
        .iter()
        .filter(|n| ids.contains(&n.id.0))
        .filter_map(|n| n.group)
        .collect();
    let mut out = Vec::new();
    for n in &mut m.annotations {
        if n.group.is_some_and(|g| groups.contains(&g)) {
            n.group = None;
            out.push(n.id.0);
        }
    }
    Ok(json!({ "ungrouped": out }))
}

/// Every member of the groups of `ids`, plus `ids` (the editor's
/// selection rule: clicking one member selects the group).
pub fn expand_groups(m: &Manifest, ids: &[NodeId]) -> Vec<NodeId> {
    let groups: Vec<u64> = ids.iter().filter_map(|id| m.node(*id)?.group).collect();
    m.annotations
        .iter()
        .filter(|n| ids.contains(&n.id) || n.group.is_some_and(|g| groups.contains(&g)))
        .map(|n| n.id)
        .collect()
}

// ---------------------------------------------------------------------------
// Paste and duplicate

/// Adds copies of `nodes` with fresh ids, moved by `(dx, dy)`; groups are
/// renumbered so copies never join the originals' groups.
fn insert_copies(
    m: &mut Manifest,
    nodes: Vec<AnnotationNode>,
    dx: f64,
    dy: f64,
) -> Vec<(NodeId, NodeId)> {
    let mut groups: BTreeMap<u64, u64> = BTreeMap::new();
    let mut out = Vec::new();
    for mut n in nodes {
        let old = n.id;
        n.id = m.alloc_node_id();
        n.locked = false;
        n.revision = 0;
        n.group = n.group.map(|g| {
            let fresh = next_group(m) + groups.len() as u64;
            *groups.entry(g).or_insert(fresh)
        });
        if let Some(a) = n.annotation_mut() {
            a.translate(dx, dy);
        }
        out.push((old, n.id));
        m.annotations.push(n);
    }
    out
}

fn check_offset(cmd: &str, dx: f64, dy: f64) -> Result<()> {
    let lim = f64::from(capturecraft_format::MAX_SIDE);
    if dx.is_finite() && dy.is_finite() && dx.abs() <= lim && dy.abs() <= lim {
        Ok(())
    } else {
        Err(bad(
            cmd,
            "dx and dy must be finite and within the canvas range",
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasteParams {
    nodes: Vec<Value>,
    #[serde(default)]
    dx: f64,
    #[serde(default)]
    dy: f64,
}

fn paste(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.paste";
    let p: PasteParams = parse(CMD, p)?;
    check_offset(CMD, p.dx, p.dy)?;
    if p.nodes.len() > 1000 {
        return Err(bad(CMD, "more than 1000 nodes"));
    }
    let mut nodes = Vec::new();
    for v in p.nodes {
        let n: AnnotationNode = serde_json::from_value(v).map_err(|e| bad(CMD, e))?;
        // Smart objects need their pixels, cut outs belong to their capture.
        if matches!(
            n.annotation(),
            Some(Annotation::SmartObject(_) | Annotation::CutOutSeam(_)) | None
        ) {
            continue;
        }
        nodes.push(n);
    }
    if nodes.is_empty() {
        return Err(bad(CMD, "nothing that can be pasted"));
    }
    let m = s.manifest_mut()?;
    let ids: Vec<u64> = insert_copies(m, nodes, p.dx, p.dy)
        .into_iter()
        .map(|(_, new)| new.0)
        .collect();
    Ok(json!({ "ids": ids }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DuplicateParams {
    ids: Vec<u64>,
    #[serde(default = "sixteen")]
    dx: f64,
    #[serde(default = "sixteen")]
    dy: f64,
}

fn sixteen() -> f64 {
    16.0
}

fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.duplicate";
    let p: DuplicateParams = parse(CMD, p)?;
    check_offset(CMD, p.dx, p.dy)?;
    let ids = unique(p.ids);
    let doc = s.active_mut().ok_or(EngineError::NoDocument)?;
    check_exist(CMD, &doc.manifest, &ids)?;
    let nodes: Vec<AnnotationNode> = doc
        .manifest
        .annotations
        .iter()
        .filter(|n| {
            ids.contains(&n.id.0) && !matches!(n.annotation(), Some(Annotation::CutOutSeam(_)))
        })
        .cloned()
        .collect();
    let pairs = insert_copies(&mut doc.manifest, nodes, p.dx, p.dy);
    for (old, new) in &pairs {
        if let Some(px) = doc.layers.objects.get(old).cloned() {
            doc.layers.objects.insert(*new, px);
            if let Some(Annotation::SmartObject(o)) =
                doc.manifest.node_mut(*new).and_then(|n| n.annotation_mut())
            {
                o.image_path = capturecraft_format::object_path(*new);
            }
        }
    }
    Ok(json!({ "ids": pairs.iter().map(|(_, n)| n.0).collect::<Vec<_>>() }))
}

// ---------------------------------------------------------------------------
// Text

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Align {
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddTextParams {
    bounds: RectF,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    font: Option<FontSpec>,
    #[serde(default)]
    color: Option<Color>,
    #[serde(default)]
    align: Option<Align>,
    #[serde(default, deserialize_with = "double_option")]
    shadow: Option<Option<capturecraft_format::DropShadow>>,
    #[serde(default)]
    fill: Option<capturecraft_format::Fill>,
    #[serde(default, deserialize_with = "double_option")]
    stroke: Option<Option<capturecraft_format::Stroke>>,
    #[serde(default)]
    underline: bool,
    #[serde(default)]
    valign: Option<VAlign>,
    #[serde(default)]
    padding: Option<f32>,
}

fn double_option<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// A text box: a callout without body, outline or tail, its text at the
/// top left (what Edit Text also creates).
pub fn text_box(bounds: RectF, text: &str, font: FontSpec, color: Color) -> Callout {
    let mut block = TextBlock::plain(text, font, color);
    block.align = TextAlign::Left;
    block.valign = VAlign::Top;
    block.padding = 4.0;
    Callout {
        bounds,
        body: CalloutBody::Rect,
        tails: Vec::new(),
        fill: capturecraft_format::Fill::None,
        stroke: None,
        shadow: None,
        text: block,
        fillet_px: 0.0,
    }
}

/// A callout drawn as a text box: without tails, and either bare or with
/// the square body a text box is created with (a filled or outlined text
/// box stays a text box).
pub fn is_text_box(c: &Callout) -> bool {
    c.tails.is_empty()
        && (matches!(c.body, CalloutBody::Rect)
            || (c.stroke.is_none() && matches!(c.fill, capturecraft_format::Fill::None)))
}

fn add_text(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.addText";
    let p: AddTextParams = parse(CMD, p)?;
    if !(p.bounds.w > 0.0 && p.bounds.h > 0.0) {
        return Err(bad(CMD, "bounds need a positive width and height"));
    }
    let font = p.font.unwrap_or_else(|| FontSpec {
        size: 24.0,
        ..FontSpec::default()
    });
    let text = p.text.unwrap_or_else(|| "Text".into());
    let mut c = text_box(p.bounds, &text, font, p.color.unwrap_or(Color::BLACK));
    c.text.align = match p.align.unwrap_or(Align::Left) {
        Align::Left => TextAlign::Left,
        Align::Center => TextAlign::Center,
        Align::Right => TextAlign::Right,
        Align::Justify => TextAlign::Justify,
    };
    c.shadow = p.shadow.flatten();
    if let Some(f) = p.fill {
        c.fill = f;
    }
    c.stroke = p.stroke.flatten();
    for run in &mut c.text.runs {
        run.underline = p.underline;
    }
    if let Some(v) = p.valign {
        c.text.valign = v;
    }
    if let Some(pad) = p.padding {
        if !(pad.is_finite() && pad >= 0.0) {
            return Err(bad(CMD, "padding must be a non-negative number"));
        }
        c.text.padding = pad;
    }
    let id = push_node(s.manifest_mut()?, Annotation::Callout(c));
    Ok(json!({ "id": id.0 }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetTextParams {
    id: u64,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    align: Option<TextAlign>,
}

fn set_text(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.setText";
    let p: SetTextParams = parse(CMD, p)?;
    if p.text.is_none() && p.align.is_none() {
        return Err(bad(CMD, "nothing to change: give text or align"));
    }
    let node = node_annotation(s.manifest_mut()?, p.id)?;
    let Some(Annotation::Callout(c)) = node.annotation_mut() else {
        return Err(bad(CMD, format!("annotation {} has no text", p.id)));
    };
    if let Some(text) = p.text {
        // An emptied callout keeps its (empty) run: the font and colour
        // its next text gets.
        let first = c.text.runs.first().cloned().unwrap_or_default();
        c.text.runs = vec![capturecraft_format::TextRun { text, ..first }];
    }
    if let Some(align) = p.align {
        c.text.align = align;
    }
    node.touch();
    Ok(json!({}))
}

/// CSS weights of the toolbar's Bold toggle: on and off.
pub const BOLD_WEIGHT: u16 = 700;
pub const REGULAR_WEIGHT: u16 = 400;
/// From this weight up a font counts as bold.
pub const BOLD_FROM: u16 = 600;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextFormatParams {
    ids: Vec<u64>,
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    size: Option<f32>,
    #[serde(default)]
    bold: Option<bool>,
    #[serde(default)]
    italic: Option<bool>,
    #[serde(default)]
    underline: Option<bool>,
    #[serde(default)]
    color: Option<Color>,
    #[serde(default)]
    align: Option<TextAlign>,
    #[serde(default)]
    valign: Option<VAlign>,
    #[serde(default)]
    padding: Option<f32>,
}

impl TextFormatParams {
    fn changes_runs(&self) -> bool {
        self.family.is_some()
            || self.size.is_some()
            || self.bold.is_some()
            || self.italic.is_some()
            || self.underline.is_some()
            || self.color.is_some()
    }

    fn is_empty(&self) -> bool {
        !self.changes_runs()
            && self.align.is_none()
            && self.valign.is_none()
            && self.padding.is_none()
    }
}

/// Changes single text attributes of text boxes and callouts, keeping the
/// rest of each run's font (unlike `annotation.setStyle`, which replaces
/// the whole font).
fn set_text_format(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.setTextFormat";
    let mut p: TextFormatParams = parse(CMD, p)?;
    if p.is_empty() {
        return Err(bad(CMD, "nothing to change"));
    }
    if p.family.as_ref().is_some_and(|f| f.trim().is_empty()) {
        return Err(bad(CMD, "family must not be empty"));
    }
    if let Some(pad) = p.padding
        && !(pad.is_finite() && pad >= 0.0)
    {
        return Err(bad(CMD, "padding must be a non-negative number"));
    }
    p.ids.sort_unstable();
    p.ids.dedup();
    let m = s.manifest_mut()?;
    for &id in &p.ids {
        let node = node_annotation(m, id)?;
        let Some(Annotation::Callout(c)) = node.annotation_mut() else {
            return Err(bad(CMD, format!("annotation {id} has no text")));
        };
        // An empty callout keeps the text style in an empty run.
        if c.text.runs.is_empty() && p.changes_runs() {
            c.text.runs.push(capturecraft_format::TextRun::default());
        }
        for run in &mut c.text.runs {
            if let Some(f) = &p.family {
                run.font.family = f.trim().to_string();
            }
            if let Some(size) = p.size {
                run.font.size = size;
            }
            match p.bold {
                Some(true) if run.font.weight < BOLD_FROM => run.font.weight = BOLD_WEIGHT,
                Some(false) if run.font.weight >= BOLD_FROM => run.font.weight = REGULAR_WEIGHT,
                _ => {}
            }
            if let Some(i) = p.italic {
                run.font.italic = i;
            }
            if let Some(u) = p.underline {
                run.underline = u;
            }
            if let Some(col) = p.color {
                run.color = col;
            }
        }
        if let Some(a) = p.align {
            c.text.align = a;
        }
        if let Some(v) = p.valign {
            c.text.valign = v;
        }
        if let Some(pad) = p.padding {
            c.text.padding = pad;
        }
        node.touch();
    }
    Ok(json!({ "formatted": p.ids }))
}

// ---------------------------------------------------------------------------
// Magnify and Spotlight

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MagnifierParams {
    center: Point,
    #[serde(default)]
    radius: Option<f32>,
    #[serde(default)]
    zoom: Option<f32>,
    #[serde(default)]
    source_center: Option<Point>,
    #[serde(default, deserialize_with = "double_option")]
    border: Option<Option<capturecraft_format::Stroke>>,
}

fn add_magnifier(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.addMagnifier";
    let p: MagnifierParams = parse(CMD, p)?;
    let d = MagnifierLens::default();
    let radius = p.radius.unwrap_or(80.0);
    if !(radius.is_finite() && radius >= 4.0) {
        return Err(bad(CMD, "radius must be at least 4 px"));
    }
    let lens = MagnifierLens {
        source_center: p.source_center.unwrap_or(p.center),
        lens: LensShape::Circle {
            center: p.center,
            radius,
        },
        zoom: p.zoom.unwrap_or(d.zoom),
        border: p.border.unwrap_or(d.border),
        ..d
    };
    let id = push_node(s.manifest_mut()?, Annotation::MagnifierLens(lens));
    Ok(json!({ "id": id.0 }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpotlightParams {
    bounds: RectF,
    #[serde(default)]
    kind: RegionKind,
    #[serde(default)]
    dim_opacity: Option<f32>,
    #[serde(default)]
    blur_sigma: Option<f32>,
}

fn add_spotlight(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "annotation.addSpotlight";
    let p: SpotlightParams = parse(CMD, p)?;
    let d = Spotlight::default();
    let target = crate::edit_commands::region_of(CMD, p.bounds, p.kind, None)?;
    let spot = Spotlight {
        targets: vec![target],
        dim_opacity: p.dim_opacity.unwrap_or(d.dim_opacity),
        blur_sigma: p.blur_sigma,
        ..d
    };
    let id = push_node(s.manifest_mut()?, Annotation::Spotlight(spot));
    Ok(json!({ "id": id.0 }))
}

// ---------------------------------------------------------------------------
// Crop

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CropParams {
    rect: RectF,
}

fn crop(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "canvas.crop";
    let p: CropParams = parse(CMD, p)?;
    let r = p.rect;
    if ![r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite()) || r.w < 1.0 || r.h < 1.0 {
        return Err(bad(CMD, "rect must be finite and at least 1 × 1 px"));
    }
    let size = s.manifest_mut()?.canvas.size;
    let (x0, y0) = (r.x.floor(), r.y.floor());
    let (x1, y1) = ((r.x + r.w).ceil(), (r.y + r.h).ceil());
    let clamp = |v: f64| v.clamp(-f64::from(i32::MAX / 2), f64::from(i32::MAX / 2)) as i32;
    // Negative margins trim; a rect past the edge adds padding.
    let params = json!({
        "left": -clamp(x0),
        "top": -clamp(y0),
        "right": clamp(x1) - size.width as i32,
        "bottom": clamp(y1) - size.height as i32,
    });
    crate::edit_commands::expand_bounds(s, &params).map_err(|e| match e {
        EngineError::BadParams { msg, .. } => bad(CMD, msg),
        e => e,
    })
}

// ---------------------------------------------------------------------------
// History

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JumpParams {
    steps: i32,
}

fn jump(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "edit.jump";
    let p: JumpParams = parse(CMD, p)?;
    let doc = s.active().ok_or(EngineError::NoDocument)?;
    let (undo, redo) = doc.history_labels();
    let n = p.steps.unsigned_abs() as usize;
    let available = if p.steps < 0 { undo.len() } else { redo.len() };
    if n > available {
        return Err(bad(CMD, format!("only {available} steps that way")));
    }
    let mut labels = Vec::new();
    for _ in 0..n {
        let v = if p.steps < 0 { s.undo()? } else { s.redo()? };
        if let Some(l) = v.as_object().and_then(|o| o.values().next()).cloned() {
            labels.push(l);
        }
    }
    let key = if p.steps < 0 { "undone" } else { "redone" };
    Ok(json!({ key: labels }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::Pixels;
    use photocraft_geom::Size;

    fn session() -> Session {
        let mut s = Session::new();
        let px = Pixels::new(Size::new(200, 100), vec![255; 200 * 100 * 4]);
        s.open(Manifest::new(Size::new(200, 100)), px).unwrap();
        s
    }

    fn arrow(s: &mut Session, x: f64) -> u64 {
        s.run(
            "annotation.addArrow",
            &json!({ "start": { "x": x, "y": 10 }, "end": { "x": x + 50.0, "y": 10 } }),
        )
        .unwrap()["id"]
            .as_u64()
            .unwrap()
    }

    fn ids(s: &Session) -> Vec<u64> {
        s.active()
            .unwrap()
            .manifest
            .annotations
            .iter()
            .map(|n| n.id.0)
            .collect()
    }

    #[test]
    fn arrange_reorder_and_delete() {
        let mut s = session();
        let (a, b, c) = (arrow(&mut s, 0.0), arrow(&mut s, 10.0), arrow(&mut s, 20.0));
        s.run("annotation.arrange", &json!({ "ids": [a], "to": "front" }))
            .unwrap();
        assert_eq!(ids(&s), [b, c, a]);
        s.run(
            "annotation.arrange",
            &json!({ "ids": [a], "to": "backward" }),
        )
        .unwrap();
        assert_eq!(ids(&s), [b, a, c]);
        s.run(
            "annotation.arrange",
            &json!({ "ids": [c, a], "to": "back" }),
        )
        .unwrap();
        assert_eq!(ids(&s), [a, c, b]);
        s.run("annotation.reorder", &json!({ "id": a, "index": 99 }))
            .unwrap();
        assert_eq!(ids(&s), [c, b, a]);
        s.run(
            "annotation.setProps",
            &json!({ "ids": [b], "locked": true, "visible": false, "name": " Box " }),
        )
        .unwrap();
        let out = s
            .run("annotation.delete", &json!({ "ids": [a, b] }))
            .unwrap();
        assert_eq!(out["deleted"], json!([a]), "locked nodes stay");
        assert_eq!(ids(&s), [c, b]);
        let n = s.active().unwrap().manifest.node(NodeId(b)).unwrap();
        assert_eq!((n.visible, n.locked, n.name.as_str()), (false, true, "Box"));
        assert!(
            s.run("annotation.delete", &json!({ "ids": [999] }))
                .is_err()
        );
    }

    #[test]
    fn groups_copies_and_duplicates() {
        let mut s = session();
        let (a, b) = (arrow(&mut s, 0.0), arrow(&mut s, 10.0));
        assert!(s.run("annotation.group", &json!({ "ids": [a] })).is_err());
        let g = s
            .run("annotation.group", &json!({ "ids": [a, b] }))
            .unwrap()["group"]
            .as_u64()
            .unwrap();
        let m = &s.active().unwrap().manifest;
        assert_eq!(expand_groups(m, &[NodeId(a)]), [NodeId(a), NodeId(b)]);
        let copied: Vec<Value> = m
            .annotations
            .iter()
            .map(|n| serde_json::to_value(n).unwrap())
            .collect();
        let pasted = s
            .run(
                "annotation.paste",
                &json!({ "nodes": copied, "dx": 5, "dy": 7 }),
            )
            .unwrap();
        let new: Vec<u64> = pasted["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        assert_eq!(new.len(), 2);
        let m = &s.active().unwrap().manifest;
        let copy = m.node(NodeId(new[0])).unwrap();
        assert_ne!(copy.group, Some(g), "copies form their own group");
        assert_eq!(copy.group, m.node(NodeId(new[1])).unwrap().group);
        let Some(Annotation::CurvedArrow(arr)) = copy.annotation() else {
            panic!()
        };
        assert_eq!((arr.start.x, arr.start.y), (5.0, 17.0));
        let dup = s
            .run("annotation.duplicate", &json!({ "ids": [a] }))
            .unwrap();
        assert_eq!(dup["ids"].as_array().unwrap().len(), 1);
        s.run("annotation.ungroup", &json!({ "ids": [b] })).unwrap();
        let m = &s.active().unwrap().manifest;
        assert!(m.node(NodeId(a)).unwrap().group.is_none());
        assert!(
            s.run(
                "annotation.paste",
                &json!({ "nodes": [{ "id": 1, "kind": 5 }] })
            )
            .is_err()
        );
    }

    #[test]
    fn text_magnifier_spotlight_and_crop() {
        let mut s = session();
        let t = s
            .run(
                "annotation.addText",
                &json!({ "bounds": { "x": 10, "y": 10, "w": 120, "h": 40 }, "text": "Hi" }),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        s.run("annotation.setText", &json!({ "id": t, "text": "Hello" }))
            .unwrap();
        let m = &s.active().unwrap().manifest;
        let Some(Annotation::Callout(c)) = m.node(NodeId(t)).unwrap().annotation() else {
            panic!()
        };
        assert!(is_text_box(c));
        assert_eq!(
            (c.text.text().as_str(), c.text.runs[0].font.size),
            ("Hello", 24.0)
        );
        s.run(
            "annotation.addMagnifier",
            &json!({ "center": { "x": 50, "y": 50 }, "zoom": 3 }),
        )
        .unwrap();
        s.run(
            "annotation.addSpotlight",
            &json!({ "bounds": { "x": 5, "y": 5, "w": 20, "h": 20 }, "kind": "ellipse" }),
        )
        .unwrap();
        let out = s
            .run(
                "canvas.crop",
                &json!({ "rect": { "x": 10.5, "y": 20, "w": 100, "h": 50 } }),
            )
            .unwrap();
        assert_eq!(out["canvas"], json!({ "width": 101, "height": 50 }));
        // The text box stays on the capture.
        let m = &s.active().unwrap().manifest;
        let Some(Annotation::Callout(c)) = m.node(NodeId(t)).unwrap().annotation() else {
            panic!()
        };
        assert_eq!((c.bounds.x, c.bounds.y), (0.0, -10.0));
        assert!(
            s.run(
                "canvas.crop",
                &json!({ "rect": { "x": 0, "y": 0, "w": 0.5, "h": 5 } })
            )
            .is_err()
        );
    }

    #[test]
    fn text_format_changes_single_attributes() {
        let mut s = session();
        let t = s
            .run(
                "annotation.addText",
                &json!({ "bounds": { "x": 10, "y": 10, "w": 120, "h": 40 }, "text": "Hi",
                         "font": { "family": "Inter", "size": 20, "weight": 400, "italic": false } }),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let text = |s: &Session| {
            let m = &s.active().unwrap().manifest;
            let Some(Annotation::Callout(c)) = m.node(NodeId(t)).unwrap().annotation() else {
                panic!()
            };
            c.text.clone()
        };
        s.run(
            "annotation.setTextFormat",
            &json!({ "ids": [t], "bold": true, "underline": true, "align": "justify",
                     "valign": "bottom", "padding": 12 }),
        )
        .unwrap();
        let b = text(&s);
        let run = &b.runs[0];
        assert_eq!((run.font.weight, run.font.size), (BOLD_WEIGHT, 20.0));
        assert!(run.underline && !run.font.italic);
        assert_eq!(
            (b.align, b.valign, b.padding),
            (TextAlign::Justify, VAlign::Bottom, 12.0)
        );
        s.run(
            "annotation.setTextFormat",
            &json!({ "ids": [t], "family": "JetBrains Mono", "size": 30, "bold": false,
                     "italic": true, "color": "#ff0000" }),
        )
        .unwrap();
        let run = text(&s).runs[0].clone();
        assert_eq!(run.font.family, "JetBrains Mono");
        assert_eq!((run.font.size, run.font.weight), (30.0, REGULAR_WEIGHT));
        assert!(run.font.italic && run.underline);
        assert_eq!(run.color, Color::rgb(255, 0, 0));
        // One undo step per command.
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(text(&s).runs[0].font.size, 20.0);
        let arrow = s
            .run(
                "annotation.addArrow",
                &json!({ "start": { "x": 0, "y": 0 }, "end": { "x": 9, "y": 9 } }),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        for bad in [
            json!({ "ids": [t] }),
            json!({ "ids": [t], "size": 0 }),
            json!({ "ids": [t], "padding": -1 }),
            json!({ "ids": [t], "family": " " }),
            json!({ "ids": [t], "align": "middle" }),
            json!({ "ids": [arrow], "bold": true }),
            json!({ "ids": [999], "bold": true }),
        ] {
            assert!(s.run("annotation.setTextFormat", &bad).is_err(), "{bad}");
        }
        assert_eq!(text(&s).runs[0].font.size, 20.0);
    }

    #[test]
    fn filled_text_boxes_stay_text_boxes() {
        let mut s = session();
        let t = s
            .run(
                "annotation.addText",
                &json!({ "bounds": { "x": 10, "y": 10, "w": 120, "h": 40 } }),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        s.run(
            "annotation.setStyle",
            &json!({ "ids": [t], "fill": { "kind": "solid", "color": "#ffff00" },
                     "stroke": { "color": "#000000", "width": 2 } }),
        )
        .unwrap();
        let filled = s
            .run(
                "annotation.addText",
                &json!({ "bounds": { "x": 10, "y": 60, "w": 120, "h": 40 },
                         "fill": { "kind": "solid", "color": "#ffff00" },
                         "stroke": { "color": "#000000", "width": 2 } }),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let m = &s.active().unwrap().manifest;
        let Some(Annotation::Callout(c)) = m.node(NodeId(t)).unwrap().annotation() else {
            panic!()
        };
        assert!(is_text_box(c));
        let Some(Annotation::Callout(f)) = m.node(NodeId(filled)).unwrap().annotation() else {
            panic!()
        };
        assert!(is_text_box(f) && f.stroke.is_some_and(|s| s.width == 2.0));
        let formatted = s
            .run(
                "annotation.addText",
                &json!({ "bounds": { "x": 10, "y": 110, "w": 120, "h": 40 }, "text": "U",
                         "underline": true, "valign": "bottom", "padding": 9, "align": "justify" }),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let m = &s.active().unwrap().manifest;
        let Some(Annotation::Callout(u)) = m.node(NodeId(formatted)).unwrap().annotation() else {
            panic!()
        };
        assert!(u.text.runs[0].underline);
        assert_eq!(
            (u.text.valign, u.text.padding, u.text.align),
            (VAlign::Bottom, 9.0, TextAlign::Justify)
        );
        assert!(!is_text_box(&Callout::default()));
    }

    #[test]
    fn jumping_through_history() {
        let mut s = session();
        for x in [0.0, 10.0, 20.0] {
            arrow(&mut s, x);
        }
        let (undo, redo) = s.active().unwrap().history_labels();
        assert_eq!((undo.len(), redo.len()), (3, 0));
        s.run("edit.jump", &json!({ "steps": -2 })).unwrap();
        assert_eq!(ids(&s).len(), 1);
        let (undo, redo) = s.active().unwrap().history_labels();
        assert_eq!(
            (undo, redo),
            (vec!["Add Arrow"], vec!["Add Arrow", "Add Arrow"])
        );
        assert!(s.run("edit.jump", &json!({ "steps": 3 })).is_err());
        s.run("edit.jump", &json!({ "steps": 2 })).unwrap();
        assert_eq!(ids(&s).len(), 3);
    }
}
