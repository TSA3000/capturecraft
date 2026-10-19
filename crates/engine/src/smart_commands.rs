//! Smart Move, Simplify UI and Grab / Edit Text: the commands built on the
//! installed [`UiAnalyzer`] (`capturecraft-ocr`).
//!
//! - Analysis runs on the edited base ([`crate::DocState::edited_pixels`])
//!   and is cached per pixel revision, so `smart.detectObjects` followed by
//!   `smart.moveObject { object }` segments once, and ids stay valid until
//!   the pixels change.
//! - Pixels are never edited in place. Smart Move, Delete and Edit Text
//!   heal into a copy-on-write copy of the base; Simplify heals into its
//!   own copy on top of that, so re-running or undoing it never loses a
//!   Smart Move. Lifted objects keep their pixels next to their node.
//! - Everything is journaled: undo restores the manifest and the raster
//!   layers together.

use std::collections::{BTreeMap, BTreeSet};

use capturecraft_format::{
    Annotation, AnnotationNode, Callout, CalloutBody, Color, Fill, FontSpec, MAX_TEXT_BYTES,
    Manifest, NodeId, RectF, RegionShape, ShapeNode, SmartObject, Stroke, TextAlign, TextBlock,
    TextRun, VAlign, object_path,
};
use photocraft_geom::{Point, Rect};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, base_rect_to_canvas, parse};
use crate::{
    EngineError, Patch, Pixels, Result, Session, UiAnalyzer, UiElement, UiElementId, UiElementKind,
};

pub const DETECT_ID: &str = "smart.detectObjects";
pub const MOVE_ID: &str = "smart.moveObject";
pub const DELETE_ID: &str = "smart.deleteObject";
pub const SIMPLIFY_ID: &str = "smart.simplify";
pub const SET_SIMPLIFIED_ID: &str = "smart.setSimplified";
pub const SET_PALETTE_ID: &str = "smart.setPalette";
pub const GRAB_TEXT_ID: &str = "ocr.grabText";
pub const REPLACE_TEXT_ID: &str = "ocr.replaceText";

/// Upper bound on analyzer output we keep (and turn into nodes).
pub const MAX_SIMPLIFY_ELEMENTS: usize = 5_000;
/// Upper bound on text lines per element.
pub const MAX_LINES: usize = 256;
/// Whitespace that separates two text columns for Grab Text, in pixels.
pub const COLUMN_GAP_PX: i32 = 12;

pub(crate) static REGISTRY: &[CommandSpec] = &[
    CommandSpec {
        id: DETECT_ID,
        label: "Detect Objects",
        menu_path: &[],
        shortcut: None,
        params: r##"{ region?: {x0, y0, x1, y1} (base px, default: whole capture), min_confidence?: 0..1 = 0,
  kinds?: ["text"|"button"|"icon"|"input"|"panel"|"image"|"divider"] }
-> { analyzer, objects: [{ id, parent, kind, rect, fg, bg, border, corner_radius, circular, lines,
  font_px, font_weight, confidence, text }] }"##,
        enabled: needs_analyzer,
        run: detect_objects,
        journal: false,
    },
    CommandSpec {
        id: MOVE_ID,
        label: "Smart Move",
        menu_path: &["Image", "Smart Move"],
        shortcut: None,
        params: r##"{ object?: id (from smart.detectObjects) | rect?: {x0, y0, x1, y1} (base px) | node?: id (a smart object),
  to?: {x, y} (canvas top-left) | by?: {x, y} (canvas offset), size?: {w, h} (canvas px),
  matte?: bool = true (soft alpha against the background), heal?: bool = true }
-> { id, rect, source, healed: [rects] }"##,
        enabled: needs_analyzer,
        run: move_object,
        journal: true,
    },
    CommandSpec {
        id: DELETE_ID,
        label: "Smart Delete",
        menu_path: &["Image", "Smart Move"],
        shortcut: None,
        params: r##"{ object?: id | rect?: {x0, y0, x1, y1} (base px) | node?: id (a smart object) }
-> { healed: [rects], removed?: id }"##,
        enabled: needs_analyzer,
        run: delete_object,
        journal: true,
    },
    CommandSpec {
        id: SIMPLIFY_ID,
        label: "Simplify",
        menu_path: &["Image", "Simplify"],
        shortcut: None,
        params: r##"{ region?: {x0, y0, x1, y1} (base px, default: whole capture), text_style?: "bars"|"blocks" = "bars",
  palette?: "match"|"mono"|"auto"|"light"|"dark"|"high_contrast"|"blueprint"
    | { background, surface, accent, muted_text, border, on_accent } = "match",
  keep_icons?: bool = false, exclude?: [id] (left as original pixels), min_confidence?: 0..1 = 0.5,
  heal?: bool = true, background?: bool (default: true for role palettes),
  replace?: bool = true (removes the previous Simplify output) }
-> { added: [ids], removed: n, skipped: n, excluded: [ids], healed: n, palette }"##,
        enabled: needs_analyzer,
        run: simplify,
        journal: true,
    },
    CommandSpec {
        id: SET_SIMPLIFIED_ID,
        label: "Simplify: Exclude / Restore",
        menu_path: &[],
        shortcut: None,
        params: r##"{ objects: [id], simplified: bool (false: show the original pixels) } -> as smart.simplify"##,
        enabled: has_simplify,
        run: set_simplified,
        journal: true,
    },
    CommandSpec {
        id: SET_PALETTE_ID,
        label: "Simplify: Palette",
        menu_path: &[],
        shortcut: None,
        params: r##"{ palette: as smart.simplify } -> as smart.simplify"##,
        enabled: has_simplify,
        run: set_palette,
        journal: true,
    },
    CommandSpec {
        id: GRAB_TEXT_ID,
        label: "Grab Text",
        menu_path: &["Image", "Grab Text"],
        shortcut: None,
        params: r##"{ region?: {x0, y0, x1, y1} (base px, default: whole capture) }
-> { text, recognized: bool, lines: [{ id, rect, text }] } (reading order, columns kept apart)"##,
        enabled: needs_analyzer,
        run: grab_text,
        journal: false,
    },
    CommandSpec {
        id: REPLACE_TEXT_ID,
        label: "Edit Text",
        menu_path: &["Image", "Edit Text"],
        shortcut: None,
        params: r##"{ object?: id (a text object) | rect?: {x0, y0, x1, y1} (base px), text: string,
  font?: { family?, size?, weight?, italic? } (default: estimated), color?: "#rrggbb[aa]" (default: estimated),
  heal?: bool = true }
-> { id, rect, font, color, healed: [rects], original }"##,
        enabled: needs_analyzer,
        run: replace_text,
        journal: true,
    },
];

// ---------------------------------------------------------------------------
// Enabled checks

fn needs_analyzer(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("no document open")?;
    if d.pixels.is_none() {
        return Err("the capture's pixels are not loaded".into());
    }
    if s.analyzer().is_none() {
        return Err("no UI analyzer installed (capturecraft-ocr)".into());
    }
    Ok(())
}

fn has_simplify(s: &Session) -> std::result::Result<(), String> {
    needs_analyzer(s)?;
    s.active()
        .and_then(|d| d.simplify_state())
        .map(|_| ())
        .ok_or_else(|| "run Simplify first".into())
}

// ---------------------------------------------------------------------------
// Analyzer output and shared helpers

/// Clamps untrusted analyzer output to `region` and to the document limits:
/// drops empty or non-finite elements, fixes duplicate ids and dangling or
/// cyclic parents.
pub(crate) fn sanitize(raw: Vec<UiElement>, region: Rect) -> Vec<UiElement> {
    let mut seen = BTreeSet::new();
    let mut out: Vec<UiElement> = Vec::with_capacity(raw.len().min(MAX_SIMPLIFY_ELEMENTS));
    for mut e in raw {
        if out.len() >= MAX_SIMPLIFY_ELEMENTS {
            break;
        }
        e.rect = e.rect.intersect(&region);
        if e.rect.is_empty() || !e.confidence.is_finite() {
            continue;
        }
        e.confidence = e.confidence.clamp(0.0, 1.0);
        let half = e.rect.width().min(e.rect.height()) as f32 / 2.0;
        e.corner_radius = finite_or_zero(e.corner_radius).clamp(0.0, half);
        e.font_px = finite_or_zero(e.font_px).clamp(0.0, 1024.0);
        if e.font_weight != 0 {
            e.font_weight = e.font_weight.clamp(100, 900);
        }
        e.lines.truncate(MAX_LINES);
        let r = e.rect;
        e.lines.retain_mut(|l| {
            *l = l.intersect(&r);
            !l.is_empty()
        });
        if let Some(t) = &mut e.text {
            truncate_utf8(t, MAX_TEXT_BYTES);
        }
        let mut salt = 0u64;
        while !seen.insert(e.id) {
            salt += 1;
            e.id = UiElementId((e.id.0.wrapping_add(salt)) & ((1 << 53) - 1));
        }
        out.push(e);
    }
    let ids: BTreeSet<UiElementId> = out.iter().map(|e| e.id).collect();
    for e in &mut out {
        if e.parent.is_some_and(|p| p == e.id || !ids.contains(&p)) {
            e.parent = None;
        }
    }
    // Break parent cycles: an element whose chain does not end within
    // `len` steps loses its parent.
    let parent: BTreeMap<UiElementId, UiElementId> = out
        .iter()
        .filter_map(|e| e.parent.map(|p| (e.id, p)))
        .collect();
    for e in &mut out {
        let mut cur = e.id;
        let mut steps = 0;
        while let Some(p) = parent.get(&cur) {
            cur = *p;
            steps += 1;
            if steps > ids.len() {
                e.parent = None;
                break;
            }
        }
    }
    out
}

fn finite_or_zero(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

fn truncate_utf8(s: &mut String, max: usize) {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
}

fn analyzer(s: &Session) -> Result<&dyn UiAnalyzer> {
    s.analyzer()
        .ok_or_else(|| EngineError::Other("no UI analyzer installed (capturecraft-ocr)".into()))
}

fn edited(s: &Session) -> Result<&Pixels> {
    s.active()
        .ok_or(EngineError::NoDocument)?
        .edited_pixels()
        .ok_or_else(|| EngineError::Other("the capture's pixels are not loaded".into()))
}

fn region_param(s: &Session, cmd: &str, region: Option<Rect>) -> Result<Rect> {
    let full = edited(s)?.bounds();
    let r = region.map_or(full, |r| r.intersect(&full));
    if r.is_empty() {
        return Err(bad(cmd, "region does not overlap the capture"));
    }
    Ok(r)
}

fn element<'a>(cmd: &str, all: &'a [UiElement], id: UiElementId) -> Result<&'a UiElement> {
    all.iter().find(|e| e.id == id).ok_or_else(|| {
        bad(
            cmd,
            format!(
                "no detected object {} (ids change when the pixels do; run smart.detectObjects)",
                id.0
            ),
        )
    })
}

fn parent_of<'a>(all: &'a [UiElement], e: &UiElement) -> Option<&'a UiElement> {
    e.parent.and_then(|p| all.iter().find(|x| x.id == p))
}

fn depth_of(all: &[UiElement], e: &UiElement) -> usize {
    let mut d = 0;
    let mut cur = parent_of(all, e);
    while let Some(p) = cur {
        d += 1;
        if d > all.len() {
            break;
        }
        cur = parent_of(all, p);
    }
    d
}

/// The colour behind `e`: what its pixels are matted against when lifted.
fn backdrop(all: &[UiElement], e: &UiElement) -> Option<Color> {
    match e.kind {
        UiElementKind::Text | UiElementKind::Icon | UiElementKind::Divider => Some(e.bg),
        _ => parent_of(all, e).map(|p| p.bg),
    }
}

/// Foreground near `hole` that must not be sampled while healing it:
/// elements overlapping its surroundings that neither contain it nor lie
/// inside it.
fn protect_around(all: &[UiElement], hole: Rect, skip: &BTreeSet<UiElementId>) -> Vec<Rect> {
    let zone = hole.inflate(8);
    all.iter()
        .filter(|e| e.kind != UiElementKind::Panel && !skip.contains(&e.id))
        .filter(|e| !e.rect.intersect(&zone).is_empty())
        .filter(|e| !hole.contains_rect(&e.rect) && !e.rect.contains_rect(&hole))
        .map(|e| e.rect)
        .collect()
}

/// Writes analyzer patches into `pixels`, checking each against the bounds.
fn apply_patches(pixels: &mut Pixels, patches: &[Patch]) -> Result<Vec<Rect>> {
    let mut done = Vec::with_capacity(patches.len());
    for p in patches {
        if p.rect.is_empty() {
            continue;
        }
        pixels
            .write_rgba8(p.rect, &p.rgba)
            .map_err(|e| EngineError::Other(format!("bad heal patch: {e}")))?;
        done.push(p.rect);
    }
    Ok(done)
}

/// Heals `holes` into the edited base (a copy-on-write copy of the capture).
fn heal_edited(s: &mut Session, holes: &[Rect], protect: &[Rect]) -> Result<Vec<Rect>> {
    let mut px = edited(s)?.clone();
    let patches = analyzer(s)?
        .heal(&px, holes, protect)
        .map_err(|e| EngineError::Other(format!("healing the background failed: {e}")))?;
    let healed = apply_patches(&mut px, &patches)?;
    s.layers_mut()?.edited = Some(px);
    Ok(healed)
}

fn canvas_rect(m: &Manifest, cmd: &str, r: Rect) -> Result<RectF> {
    base_rect_to_canvas(m, r).ok_or_else(|| bad(cmd, "the object lies inside a cut out strip"))
}

fn rect_json(r: Rect) -> Value {
    json!({ "x0": r.x0, "y0": r.y0, "x1": r.x1, "y1": r.y1 })
}

fn smart_object_mut<'a>(cmd: &str, m: &'a mut Manifest, id: u64) -> Result<&'a mut AnnotationNode> {
    let node = crate::commands::node_annotation(m, id)?;
    match node.annotation() {
        Some(Annotation::SmartObject(_)) => Ok(node),
        _ => Err(bad(cmd, format!("annotation {id} is not a smart object"))),
    }
}

// ---------------------------------------------------------------------------
// smart.detectObjects

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DetectParams {
    #[serde(default)]
    region: Option<Rect>,
    #[serde(default)]
    min_confidence: Option<f32>,
    #[serde(default)]
    kinds: Option<Vec<UiElementKind>>,
}

fn detect_objects(s: &mut Session, p: &Value) -> Result<Value> {
    let p: DetectParams = parse(DETECT_ID, p)?;
    let min_conf = p.min_confidence.unwrap_or(0.0);
    if !(0.0..=1.0).contains(&min_conf) {
        return Err(bad(DETECT_ID, "min_confidence must be within 0..=1"));
    }
    let region = region_param(s, DETECT_ID, p.region)?;
    let all = s.analysis(region)?;
    let objects: Vec<&UiElement> = all
        .iter()
        .filter(|e| e.confidence >= min_conf)
        .filter(|e| p.kinds.as_ref().is_none_or(|k| k.contains(&e.kind)))
        .collect();
    Ok(json!({
        "analyzer": analyzer(s)?.name(),
        "region": rect_json(region),
        "objects": serde_json::to_value(objects).map_err(|e| EngineError::Other(e.to_string()))?,
    }))
}

// ---------------------------------------------------------------------------
// smart.moveObject / smart.deleteObject

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SizeF {
    w: f64,
    h: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveParams {
    #[serde(default)]
    object: Option<UiElementId>,
    #[serde(default)]
    rect: Option<Rect>,
    #[serde(default)]
    node: Option<u64>,
    #[serde(default)]
    to: Option<Point>,
    #[serde(default)]
    by: Option<Point>,
    #[serde(default)]
    size: Option<SizeF>,
    #[serde(default)]
    matte: Option<bool>,
    #[serde(default)]
    heal: Option<bool>,
}

/// What a move or delete acts on.
enum Target {
    Element(UiElementId),
    Region(Rect),
    Node(u64),
}

fn target(
    cmd: &str,
    object: Option<UiElementId>,
    rect: Option<Rect>,
    node: Option<u64>,
) -> Result<Target> {
    match (object, rect, node) {
        (Some(o), None, None) => Ok(Target::Element(o)),
        (None, Some(r), None) => Ok(Target::Region(r)),
        (None, None, Some(n)) => Ok(Target::Node(n)),
        _ => Err(bad(cmd, "give exactly one of `object`, `rect` or `node`")),
    }
}

/// The rectangle to lift or erase, its backdrop and its kind.
fn resolve(
    s: &mut Session,
    cmd: &str,
    t: &Target,
) -> Result<(Rect, Option<Color>, String, BTreeSet<UiElementId>)> {
    let full = edited(s)?.bounds();
    let all = s.analysis(full)?;
    match *t {
        Target::Element(id) => {
            let e = element(cmd, &all, id)?;
            let skip = BTreeSet::from([id]);
            Ok((e.rect, backdrop(&all, e), e.kind.name().into(), skip))
        }
        Target::Region(r) => {
            let r = r.intersect(&full);
            if r.is_empty() {
                return Err(bad(cmd, "rect does not overlap the capture"));
            }
            Ok((r, None, "region".into(), BTreeSet::new()))
        }
        Target::Node(_) => Err(bad(cmd, "a node is not a region of the capture")),
    }
}

fn place(
    cmd: &str,
    mut r: RectF,
    to: Option<Point>,
    by: Option<Point>,
    size: Option<SizeF>,
) -> Result<RectF> {
    match (to, by) {
        (Some(_), Some(_)) => return Err(bad(cmd, "give `to` or `by`, not both")),
        (Some(p), None) => (r.x, r.y) = (p.x, p.y),
        (None, Some(d)) => r.translate(d.x, d.y),
        (None, None) => {}
    }
    if let Some(sz) = size {
        if !(sz.w.is_finite() && sz.h.is_finite() && sz.w > 0.0 && sz.h > 0.0) {
            return Err(bad(cmd, "size must be positive"));
        }
        (r.w, r.h) = (sz.w, sz.h);
    }
    Ok(r)
}

fn move_object(s: &mut Session, p: &Value) -> Result<Value> {
    let p: MoveParams = parse(MOVE_ID, p)?;
    let t = target(MOVE_ID, p.object, p.rect, p.node)?;
    if let Target::Node(id) = t {
        let m = s.manifest_mut()?;
        let node = smart_object_mut(MOVE_ID, m, id)?;
        let Some(Annotation::SmartObject(o)) = node.annotation_mut() else {
            return Err(bad(MOVE_ID, "not a smart object"));
        };
        o.rect = place(MOVE_ID, o.rect, p.to, p.by, p.size)?;
        let rect = o.rect;
        node.touch();
        return Ok(json!({ "id": id, "rect": rect, "healed": [] }));
    }
    let (rect, bg, kind, skip) = resolve(s, MOVE_ID, &t)?;
    let full = edited(s)?.bounds();
    let all = s.analysis(full)?;

    let base = edited(s)?.clone();
    let patch = if p.matte.unwrap_or(true) {
        analyzer(s)?
            .extract(&base, rect, bg)
            .map_err(|e| EngineError::Other(format!("lifting the object failed: {e}")))?
    } else {
        Patch {
            rect,
            rgba: base.read_rgba8(rect).map_err(EngineError::Other)?,
        }
    };
    if !full.contains_rect(&patch.rect) || patch.rect.is_empty() {
        return Err(EngineError::Other(
            "the analyzer lifted pixels outside the capture".into(),
        ));
    }
    let pixels = Pixels::new(patch.rect.size(), patch.rgba)
        .ok_or_else(|| EngineError::Other("the analyzer returned a malformed object".into()))?;
    let healed = if p.heal.unwrap_or(true) {
        heal_edited(s, &[patch.rect], &protect_around(&all, patch.rect, &skip))?
    } else {
        Vec::new()
    };

    let m = s.manifest_mut()?;
    let src = canvas_rect(m, MOVE_ID, patch.rect)?;
    let dst = place(MOVE_ID, src, p.to, p.by, p.size)?;
    let id = m.alloc_node_id();
    let mut node = AnnotationNode::new(
        id,
        Annotation::SmartObject(SmartObject {
            rect: dst,
            image_path: object_path(id),
            image_size: patch.rect.size(),
            source: patch.rect,
            element: kind.clone(),
        }),
    );
    node.name = format!("Smart Object ({kind})");
    node.generated_by = Some(MOVE_ID.into());
    m.annotations.push(node);
    s.layers_mut()?.objects.insert(id, pixels);
    Ok(json!({
        "id": id.0,
        "rect": dst,
        "source": rect_json(patch.rect),
        "healed": healed.into_iter().map(rect_json).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteParams {
    #[serde(default)]
    object: Option<UiElementId>,
    #[serde(default)]
    rect: Option<Rect>,
    #[serde(default)]
    node: Option<u64>,
}

fn delete_object(s: &mut Session, p: &Value) -> Result<Value> {
    let p: DeleteParams = parse(DELETE_ID, p)?;
    let t = target(DELETE_ID, p.object, p.rect, p.node)?;
    if let Target::Node(id) = t {
        let m = s.manifest_mut()?;
        smart_object_mut(DELETE_ID, m, id)?;
        m.annotations.retain(|n| n.id != NodeId(id));
        return Ok(json!({ "healed": [], "removed": id }));
    }
    let (rect, _, _, skip) = resolve(s, DELETE_ID, &t)?;
    let all = s.analysis(edited(s)?.bounds())?;
    let healed = heal_edited(s, &[rect], &protect_around(&all, rect, &skip))?;
    Ok(json!({ "healed": healed.into_iter().map(rect_json).collect::<Vec<_>>() }))
}

// ---------------------------------------------------------------------------
// Simplify UI

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextStyle {
    /// One rounded bar per text line, the "wireframe" look.
    #[default]
    Bars,
    /// A solid block over the whole text area.
    Blocks,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Palette {
    /// Each element keeps its own sampled colours.
    #[default]
    Match,
    /// Greys of the same luminance.
    Mono,
    /// Swatches quantized from the screenshot ([`UiAnalyzer::palette`]).
    Auto,
    Light,
    Dark,
    /// Black, white and greys only.
    HighContrast,
    /// Light lines on navy.
    Blueprint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PaletteChoice {
    Named(Palette),
    Custom(SimplifyPalette),
}

impl Default for PaletteChoice {
    fn default() -> Self {
        PaletteChoice::Named(Palette::Match)
    }
}

/// The swatches a role-based Simplify paints with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimplifyPalette {
    /// Behind everything (the region).
    pub background: Color,
    /// Panels, cards, toolbars.
    pub surface: Color,
    /// Buttons, avatars, the primary action.
    pub accent: Color,
    /// Text bars and plain icons.
    pub muted_text: Color,
    /// Outlines and dividers.
    pub border: Color,
    /// Text bars on an accent fill.
    pub on_accent: Color,
}

impl SimplifyPalette {
    pub const LIGHT: Self = Self {
        background: Color::rgb(0xF5, 0xF6, 0xF8),
        surface: Color::rgb(0xFF, 0xFF, 0xFF),
        accent: Color::rgb(0x3B, 0x82, 0xF6),
        muted_text: Color::rgb(0xC9, 0xCE, 0xD6),
        border: Color::rgb(0xDD, 0xE1, 0xE6),
        on_accent: Color::rgb(0xFF, 0xFF, 0xFF),
    };
    pub const DARK: Self = Self {
        background: Color::rgb(0x1E, 0x1F, 0x22),
        surface: Color::rgb(0x2B, 0x2D, 0x31),
        accent: Color::rgb(0x58, 0x65, 0xF2),
        muted_text: Color::rgb(0x4E, 0x50, 0x58),
        border: Color::rgb(0x3F, 0x41, 0x47),
        on_accent: Color::rgb(0xFF, 0xFF, 0xFF),
    };
    pub const HIGH_CONTRAST: Self = Self {
        background: Color::rgb(0xFF, 0xFF, 0xFF),
        surface: Color::rgb(0xFF, 0xFF, 0xFF),
        accent: Color::rgb(0x00, 0x00, 0x00),
        muted_text: Color::rgb(0x59, 0x59, 0x59),
        border: Color::rgb(0x00, 0x00, 0x00),
        on_accent: Color::rgb(0xFF, 0xFF, 0xFF),
    };
    pub const BLUEPRINT: Self = Self {
        background: Color::rgb(0x0B, 0x3D, 0x91),
        surface: Color::rgb(0x11, 0x4F, 0xB8),
        accent: Color::rgb(0x8F, 0xD3, 0xFF),
        muted_text: Color::rgb(0x5E, 0x8F, 0xD6),
        border: Color::rgb(0x8F, 0xD3, 0xFF),
        on_accent: Color::rgb(0x0B, 0x3D, 0x91),
    };
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimplifyParams {
    #[serde(default)]
    pub region: Option<Rect>,
    #[serde(default)]
    pub text_style: TextStyle,
    #[serde(default)]
    pub palette: PaletteChoice,
    #[serde(default)]
    pub keep_icons: bool,
    #[serde(default)]
    pub exclude: Vec<UiElementId>,
    #[serde(default)]
    pub min_confidence: Option<f32>,
    #[serde(default)]
    pub heal: Option<bool>,
    #[serde(default)]
    pub background: Option<bool>,
    #[serde(default)]
    pub replace: Option<bool>,
}

/// The last Simplify pass, so exclude/restore and palette switches can
/// re-run it. Journaled with the document.
#[derive(Clone, Debug, PartialEq)]
pub struct SimplifyState {
    pub params: SimplifyParams,
    /// The swatches used (`None` for `match` and `mono`).
    pub palette: Option<SimplifyPalette>,
    /// Generated nodes per element.
    pub nodes: BTreeMap<UiElementId, Vec<NodeId>>,
    /// Elements left as original pixels (with their descendants).
    pub excluded: BTreeSet<UiElementId>,
    /// The edited base plus the heals behind simplified elements.
    pub(crate) pixels: Option<Pixels>,
}

/// How wireframes are coloured.
#[derive(Clone, Copy)]
enum Scheme {
    Match,
    Mono,
    Roles(SimplifyPalette),
}

fn simplify(s: &mut Session, p: &Value) -> Result<Value> {
    let p: SimplifyParams = parse(SIMPLIFY_ID, p)?;
    run_simplify(s, SIMPLIFY_ID, p)
}

fn set_simplified(s: &mut Session, p: &Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        objects: Vec<UiElementId>,
        simplified: bool,
    }
    let p: P = parse(SET_SIMPLIFIED_ID, p)?;
    let mut params = last_params(s, SET_SIMPLIFIED_ID)?;
    let mut set: BTreeSet<UiElementId> = params.exclude.iter().copied().collect();
    for id in p.objects {
        if p.simplified {
            set.remove(&id);
        } else {
            set.insert(id);
        }
    }
    params.exclude = set.into_iter().collect();
    params.replace = Some(true);
    run_simplify(s, SET_SIMPLIFIED_ID, params)
}

fn set_palette(s: &mut Session, p: &Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        palette: PaletteChoice,
    }
    let p: P = parse(SET_PALETTE_ID, p)?;
    let mut params = last_params(s, SET_PALETTE_ID)?;
    params.palette = p.palette;
    params.replace = Some(true);
    run_simplify(s, SET_PALETTE_ID, params)
}

fn last_params(s: &Session, cmd: &str) -> Result<SimplifyParams> {
    s.active()
        .and_then(|d| d.simplify_state())
        .map(|st| st.params.clone())
        .ok_or_else(|| EngineError::Disabled(cmd.into(), "run Simplify first".into()))
}

fn run_simplify(s: &mut Session, cmd: &str, p: SimplifyParams) -> Result<Value> {
    let min_conf = p.min_confidence.unwrap_or(0.5);
    if !(0.0..=1.0).contains(&min_conf) {
        return Err(bad(cmd, "min_confidence must be within 0..=1"));
    }
    let region = region_param(s, cmd, p.region)?;
    let all = s.analysis(region)?;
    let scheme = match p.palette {
        PaletteChoice::Named(Palette::Match) => Scheme::Match,
        PaletteChoice::Named(Palette::Mono) => Scheme::Mono,
        PaletteChoice::Named(Palette::Auto) => Scheme::Roles(
            analyzer(s)?
                .palette(edited(s)?, region)
                .map_err(|e| EngineError::Other(format!("palette extraction failed: {e}")))?,
        ),
        PaletteChoice::Named(Palette::Light) => Scheme::Roles(SimplifyPalette::LIGHT),
        PaletteChoice::Named(Palette::Dark) => Scheme::Roles(SimplifyPalette::DARK),
        PaletteChoice::Named(Palette::HighContrast) => {
            Scheme::Roles(SimplifyPalette::HIGH_CONTRAST)
        }
        PaletteChoice::Named(Palette::Blueprint) => Scheme::Roles(SimplifyPalette::BLUEPRINT),
        PaletteChoice::Custom(c) => Scheme::Roles(c),
    };

    let kept: Vec<&UiElement> = all.iter().filter(|e| e.confidence >= min_conf).collect();
    // Excluded elements and everything inside them stay original pixels.
    let mut roots: BTreeSet<UiElementId> = p.exclude.iter().copied().collect();
    if p.keep_icons {
        roots.extend(
            kept.iter()
                .filter(|e| matches!(e.kind, UiElementKind::Icon | UiElementKind::Image))
                .map(|e| e.id),
        );
    }
    let excluded_by = |e: &UiElement| {
        let mut cur = Some(e);
        let mut steps = 0;
        while let Some(x) = cur {
            if roots.contains(&x.id) {
                return true;
            }
            steps += 1;
            if steps > all.len() {
                break;
            }
            cur = parent_of(&all, x);
        }
        false
    };
    let (excluded, mut simplified): (Vec<&UiElement>, Vec<&UiElement>) =
        kept.iter().partition(|e| excluded_by(e));
    // Containers first, then the largest; text and icons end on top.
    simplified.sort_by_key(|e| {
        (
            z_rank(e.kind),
            depth_of(&all, e),
            std::cmp::Reverse(e.rect.size().area()),
        )
    });

    // Heal behind what gets simplified; the excluded stay sharp.
    let heal_on = p.heal.unwrap_or(true);
    let mut healed = 0usize;
    let mut heal_error = None;
    let mut heal_pixels = None;
    if heal_on {
        let mut holes: Vec<Rect> = simplified
            .iter()
            .filter(|e| !matches!(e.kind, UiElementKind::Panel | UiElementKind::Image))
            .map(|e| e.rect.inflate(1).intersect(&region))
            .collect();
        holes.sort_by_key(|r| std::cmp::Reverse(r.size().area()));
        let mut outer: Vec<Rect> = Vec::new();
        for h in holes {
            if !outer.iter().any(|o| o.contains_rect(&h)) {
                outer.push(h);
            }
        }
        let protect: Vec<Rect> = excluded.iter().map(|e| e.rect).collect();
        if !outer.is_empty() {
            let mut px = edited(s)?.clone();
            match analyzer(s)?.heal(&px, &outer, &protect) {
                Ok(patches) => {
                    healed = apply_patches(&mut px, &patches)?.len();
                    heal_pixels = Some(px);
                }
                Err(e) => heal_error = Some(e),
            }
        }
    }
    let base = edited(s)?.clone();

    let m = s.manifest_mut()?;
    let mut removed = 0usize;
    if p.replace.unwrap_or(true) {
        let before = m.annotations.len();
        m.annotations
            .retain(|n| n.generated_by.as_deref() != Some(SIMPLIFY_ID));
        removed = before - m.annotations.len();
    }
    let mut added = Vec::with_capacity(simplified.len());
    let mut nodes: BTreeMap<UiElementId, Vec<NodeId>> = BTreeMap::new();
    let mut skipped = all.len().saturating_sub(kept.len());
    let mut push = |m: &mut Manifest, a: Annotation, name: String| {
        let id = m.alloc_node_id();
        let mut node = AnnotationNode::new(id, a);
        node.generated_by = Some(SIMPLIFY_ID.into());
        node.name = name;
        m.annotations.push(node);
        added.push(id.0);
        id
    };
    if let Scheme::Roles(pal) = scheme
        && p.background.unwrap_or(true)
        && let Some(r) = base_rect_to_canvas(m, region)
    {
        let bg = ShapeNode {
            shape: RegionShape::Rect { rect: r },
            fill: Fill::Solid {
                color: pal.background,
            },
            stroke: None,
            ..ShapeNode::default()
        };
        push(m, Annotation::Shape(bg), "Simplified Background".into());
    } else if matches!(scheme, Scheme::Match | Scheme::Mono)
        && p.background == Some(true)
        && let Some(r) = base_rect_to_canvas(m, region)
    {
        let color = all
            .iter()
            .find(|e| e.parent.is_none())
            .map_or(Color::WHITE, |e| e.bg);
        let bg = ShapeNode {
            shape: RegionShape::Rect { rect: r },
            fill: Fill::Solid {
                color: pick(scheme, color),
            },
            stroke: None,
            ..ShapeNode::default()
        };
        push(m, Annotation::Shape(bg), "Simplified Background".into());
    }
    for e in &simplified {
        let Some(rect) = base_rect_to_canvas(m, e.rect) else {
            skipped += 1;
            continue;
        };
        let lines: Vec<RectF> = e
            .lines
            .iter()
            .filter_map(|l| base_rect_to_canvas(m, *l))
            .collect();
        let look = look(scheme, e, parent_of(&all, e), depth_of(&all, e));
        for shape in wireframe(e, rect, &lines, p.text_style, scheme, look) {
            let id = push(
                m,
                Annotation::Shape(shape),
                format!("Simplified {:?}", e.kind),
            );
            nodes.entry(e.id).or_default().push(id);
        }
    }
    // Excluded elements are shown as their original pixels above the
    // wireframes (only the outermost of each excluded subtree).
    let mut originals = Vec::new();
    for e in &excluded {
        if parent_of(&all, e).is_some_and(|p| excluded.iter().any(|x| x.id == p.id)) {
            continue;
        }
        let Some(rect) = base_rect_to_canvas(m, e.rect) else {
            skipped += 1;
            continue;
        };
        let rgba = base.read_rgba8(e.rect).map_err(EngineError::Other)?;
        let Some(px) = Pixels::new(e.rect.size(), rgba) else {
            continue;
        };
        let id = m.alloc_node_id();
        let obj = SmartObject {
            rect,
            image_path: object_path(id),
            image_size: e.rect.size(),
            source: e.rect,
            element: e.kind.name().into(),
        };
        let mut node = AnnotationNode::new(id, Annotation::SmartObject(obj));
        node.generated_by = Some(SIMPLIFY_ID.into());
        node.name = format!("Original {}", e.kind.name());
        m.annotations.push(node);
        added.push(id.0);
        nodes.entry(e.id).or_default().push(id);
        originals.push((id, px));
    }

    let palette = match scheme {
        Scheme::Roles(p) => Some(p),
        _ => None,
    };
    let excluded_ids: BTreeSet<UiElementId> = excluded.iter().map(|e| e.id).collect();
    let layers = s.layers_mut()?;
    layers.objects.extend(originals);
    layers.simplify = Some(SimplifyState {
        params: p,
        palette,
        nodes,
        excluded: excluded_ids.clone(),
        pixels: heal_pixels,
    });
    let mut out = json!({
        "added": added,
        "removed": removed,
        "skipped": skipped,
        "excluded": excluded_ids.iter().map(|i| i.0).collect::<Vec<_>>(),
        "healed": healed,
        "palette": palette,
    });
    if let Some(e) = heal_error {
        out["heal_error"] = Value::String(e);
    }
    Ok(out)
}

fn z_rank(k: UiElementKind) -> u8 {
    match k {
        UiElementKind::Panel => 0,
        UiElementKind::Image => 1,
        UiElementKind::Divider => 2,
        UiElementKind::Input | UiElementKind::Button => 3,
        UiElementKind::Icon => 4,
        UiElementKind::Text => 5,
    }
}

fn mono(c: Color) -> Color {
    // Rec. 709 luma, integer.
    let y = (2126 * u32::from(c.r) + 7152 * u32::from(c.g) + 722 * u32::from(c.b)) / 10_000;
    let y = y.min(255) as u8;
    Color::rgba(y, y, y, c.a)
}

fn with_alpha(c: Color, a: u8) -> Color {
    Color::rgba(c.r, c.g, c.b, a)
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color::rgba(l(a.r, b.r), l(a.g, b.g), l(a.b, b.b), l(a.a, b.a))
}

fn pick(scheme: Scheme, c: Color) -> Color {
    match scheme {
        Scheme::Mono => mono(c),
        _ => c,
    }
}

/// The colours one element's wireframe is drawn with.
#[derive(Clone, Copy)]
struct Look {
    fill: Color,
    ink: Color,
    border: Option<Color>,
}

fn look(scheme: Scheme, e: &UiElement, parent: Option<&UiElement>, depth: usize) -> Look {
    let Scheme::Roles(pal) = scheme else {
        return Look {
            fill: pick(scheme, e.bg),
            ink: pick(scheme, e.fg),
            border: e.border.map(|b| pick(scheme, b)),
        };
    };
    let on_button = parent.is_some_and(|p| p.kind == UiElementKind::Button);
    let alt = mix(pal.surface, pal.border, 0.5);
    match e.kind {
        UiElementKind::Panel => Look {
            fill: if depth.is_multiple_of(2) {
                pal.surface
            } else {
                alt
            },
            ink: pal.muted_text,
            border: e.border.map(|_| pal.border),
        },
        UiElementKind::Button => Look {
            fill: pal.accent,
            ink: pal.on_accent,
            border: None,
        },
        UiElementKind::Input => Look {
            fill: pal.background,
            ink: pal.muted_text,
            border: Some(pal.border),
        },
        UiElementKind::Text => Look {
            fill: pal.surface,
            ink: if on_button {
                pal.on_accent
            } else {
                pal.muted_text
            },
            border: None,
        },
        UiElementKind::Icon => Look {
            fill: pal.surface,
            ink: if e.circular {
                pal.accent
            } else {
                pal.muted_text
            },
            border: None,
        },
        UiElementKind::Divider => Look {
            fill: pal.border,
            ink: pal.border,
            border: None,
        },
        UiElementKind::Image => Look {
            fill: alt,
            ink: pal.muted_text,
            border: None,
        },
    }
}

/// The low-fidelity vector shapes for one element (canvas space).
fn wireframe(
    e: &UiElement,
    r: RectF,
    lines: &[RectF],
    text: TextStyle,
    scheme: Scheme,
    look: Look,
) -> Vec<ShapeNode> {
    let roles = matches!(scheme, Scheme::Roles(_));
    // The sampled-colour schemes keep the classic translucent wireframe look.
    let soft = |c: Color, a: u8| if roles { c } else { with_alpha(c, a) };
    let block = |rect: RectF, radius: f32, fill: Color, stroke: Option<Color>| ShapeNode {
        shape: if radius > 0.0 {
            RegionShape::RoundedRect { rect, radius }
        } else {
            RegionShape::Rect { rect }
        },
        fill: Fill::Solid { color: fill },
        stroke: stroke.map(|color| Stroke {
            color,
            width: 1.0,
            ..Stroke::default()
        }),
        shadow: None,
        ..ShapeNode::default()
    };
    let radius = |fallback: f32| {
        if e.corner_radius > 0.0 {
            e.corner_radius
        } else {
            fallback
        }
    };
    match e.kind {
        UiElementKind::Text => match text {
            TextStyle::Bars => {
                let one = [r];
                let lines = if lines.is_empty() { &one[..] } else { lines };
                lines
                    .iter()
                    .map(|l| block(*l, (l.h * 0.25) as f32, soft(look.ink, 0x99), None))
                    .collect()
            }
            TextStyle::Blocks => vec![block(r, 2.0, soft(look.ink, 0x66), None)],
        },
        UiElementKind::Button | UiElementKind::Input => {
            let edge = if roles {
                look.border
            } else {
                Some(look.border.unwrap_or(with_alpha(look.ink, 0x80)))
            };
            vec![block(r, radius(4.0), look.fill, edge)]
        }
        UiElementKind::Icon => {
            let side = r.w.min(r.h);
            let sq = RectF::new(
                r.x + (r.w - side) / 2.0,
                r.y + (r.h - side) / 2.0,
                side,
                side,
            );
            let fill = soft(look.ink, 0xB3);
            if e.circular {
                vec![ShapeNode {
                    shape: RegionShape::Ellipse { rect: sq },
                    fill: Fill::Solid { color: fill },
                    stroke: None,
                    shadow: None,
                    ..ShapeNode::default()
                }]
            } else {
                vec![block(sq, (side * 0.2) as f32, fill, None)]
            }
        }
        UiElementKind::Panel => vec![block(r, e.corner_radius, look.fill, look.border)],
        UiElementKind::Image => {
            let fill = if roles {
                look.fill
            } else {
                pick(scheme, Color::rgb(0xC8, 0xC8, 0xC8))
            };
            vec![block(r, 0.0, fill, None)]
        }
        UiElementKind::Divider => vec![block(r, 0.0, look.ink, None)],
    }
}

// ---------------------------------------------------------------------------
// Grab Text / Edit Text

/// Reading order of text boxes by recursive XY-cut: columns (vertical
/// whitespace at least [`COLUMN_GAP_PX`] wide running through every box)
/// are read one after the other, then rows top to bottom.
pub fn reading_order(rects: &[Rect]) -> Vec<usize> {
    let mut out = Vec::with_capacity(rects.len());
    // An explicit stack keeps adversarial layouts from recursing deeply.
    let mut stack = vec![(0..rects.len()).collect::<Vec<_>>()];
    while let Some(mut idx) = stack.pop() {
        if idx.len() <= 1 {
            out.extend(idx);
            continue;
        }
        let cut = split(&mut idx, rects, true, COLUMN_GAP_PX)
            .or_else(|| split(&mut idx, rects, false, 0));
        match cut {
            Some(at) => {
                let rest = idx.split_off(at);
                stack.push(rest);
                stack.push(idx);
            }
            None => {
                idx.sort_by_key(|&i| rects.get(i).map(|r| (r.y0, r.x0)));
                out.extend(idx);
            }
        }
    }
    out
}

/// Sorts `idx` along the axis and returns the first position where
/// whitespace of at least `min_gap` runs through every box, if any.
fn split(idx: &mut [usize], rects: &[Rect], columns: bool, min_gap: i32) -> Option<usize> {
    let span = |i: usize| {
        rects.get(i).map_or(
            (0, 0),
            |r| {
                if columns { (r.x0, r.x1) } else { (r.y0, r.y1) }
            },
        )
    };
    idx.sort_by_key(|&i| span(i));
    let mut end = span(*idx.first()?).1;
    for (k, &i) in idx.iter().enumerate().skip(1) {
        let (a, b) = span(i);
        if a >= end && a - end >= min_gap {
            return Some(k);
        }
        end = end.max(b);
    }
    None
}

fn grab_text(s: &mut Session, p: &Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        #[serde(default)]
        region: Option<Rect>,
    }
    let p: P = parse(GRAB_TEXT_ID, p)?;
    let region = region_param(s, GRAB_TEXT_ID, p.region)?;
    let all = s.analysis(region)?;
    let texts: Vec<&UiElement> = all
        .iter()
        .filter(|e| e.kind == UiElementKind::Text)
        .collect();
    let rects: Vec<Rect> = texts.iter().map(|e| e.rect).collect();
    let order = reading_order(&rects);
    let lines: Vec<&UiElement> = order
        .iter()
        .filter_map(|&i| texts.get(i).copied())
        .collect();
    let recognized = lines.iter().any(|e| e.text.is_some());
    let text = lines
        .iter()
        .filter_map(|e| e.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(json!({
        "text": text,
        "recognized": recognized,
        "analyzer": analyzer(s)?.name(),
        "lines": lines.iter().map(|e| json!({
            "id": e.id,
            "rect": rect_json(e.rect),
            "text": e.text,
        })).collect::<Vec<_>>(),
    }))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FontOverride {
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    size: Option<f32>,
    #[serde(default)]
    weight: Option<u16>,
    #[serde(default)]
    italic: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceParams {
    #[serde(default)]
    object: Option<UiElementId>,
    #[serde(default)]
    rect: Option<Rect>,
    text: String,
    #[serde(default)]
    font: Option<FontOverride>,
    #[serde(default)]
    color: Option<Color>,
    #[serde(default)]
    heal: Option<bool>,
}

fn replace_text(s: &mut Session, p: &Value) -> Result<Value> {
    let p: ReplaceParams = parse(REPLACE_TEXT_ID, p)?;
    let full = edited(s)?.bounds();
    let all = s.analysis(full)?;
    let e = match (p.object, p.rect) {
        (Some(id), None) => {
            let e = element(REPLACE_TEXT_ID, &all, id)?;
            if e.kind != UiElementKind::Text {
                return Err(bad(
                    REPLACE_TEXT_ID,
                    format!("object {} is a {}, not text", id.0, e.kind.name()),
                ));
            }
            e
        }
        (None, Some(r)) => all
            .iter()
            .filter(|e| e.kind == UiElementKind::Text)
            .map(|e| (e, e.rect.intersect(&r).size().area()))
            .filter(|(e, a)| *a * 2 >= e.rect.size().area() && *a > 0)
            .max_by_key(|(_, a)| *a)
            .map(|(e, _)| e)
            .ok_or_else(|| bad(REPLACE_TEXT_ID, "no text found in rect"))?,
        _ => {
            return Err(bad(
                REPLACE_TEXT_ID,
                "give exactly one of `object` or `rect`",
            ));
        }
    };
    let e = e.clone();
    let f = p.font.unwrap_or_default();
    let estimated = if e.font_px > 0.0 {
        e.font_px
    } else {
        e.rect.height() as f32 * 0.8
    };
    let font = FontSpec {
        family: f.family.unwrap_or_else(|| FontSpec::default().family),
        size: f.size.unwrap_or(estimated.clamp(1.0, 1024.0)),
        weight: f.weight.unwrap_or(if e.font_weight > 0 {
            e.font_weight
        } else {
            400
        }),
        italic: f.italic.unwrap_or(false),
    };
    let color = p.color.unwrap_or(e.fg);

    let healed = if p.heal.unwrap_or(true) {
        let hole = e.rect.inflate(1).intersect(&full);
        let skip = BTreeSet::from([e.id]);
        heal_edited(s, &[hole], &protect_around(&all, hole, &skip))?
    } else {
        Vec::new()
    };
    let m = s.manifest_mut()?;
    let bounds = canvas_rect(m, REPLACE_TEXT_ID, e.rect)?;
    let callout = Callout {
        bounds,
        body: CalloutBody::Rect,
        tails: Vec::new(),
        fill: Fill::None,
        stroke: None,
        shadow: None,
        text: TextBlock {
            runs: vec![TextRun {
                text: p.text,
                font: font.clone(),
                color,
                underline: false,
                strike: false,
            }],
            align: TextAlign::Left,
            valign: VAlign::Middle,
            padding: 0.0,
        },
        fillet_px: 0.0,
    };
    let id = m.alloc_node_id();
    let mut node = AnnotationNode::new(id, Annotation::Callout(callout));
    node.name = "Text".into();
    node.generated_by = Some(REPLACE_TEXT_ID.into());
    m.annotations.push(node);
    Ok(json!({
        "id": id.0,
        "rect": bounds,
        "font": { "family": font.family, "size": font.size, "weight": font.weight, "italic": font.italic },
        "color": color,
        "healed": healed.into_iter().map(rect_json).collect::<Vec<_>>(),
        "original": e.text,
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
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

    /// Returns fixed elements and heals every hole with opaque grey.
    struct FakeAnalyzer(
        Vec<UiElement>,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    );

    impl UiAnalyzer for FakeAnalyzer {
        fn analyze(&self, _: &Pixels, _: Rect) -> std::result::Result<Vec<UiElement>, String> {
            self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(self.0.clone())
        }

        fn heal(
            &self,
            px: &Pixels,
            holes: &[Rect],
            _: &[Rect],
        ) -> std::result::Result<Vec<Patch>, String> {
            Ok(holes
                .iter()
                .map(|h| h.intersect(&px.bounds()))
                .map(|rect| Patch {
                    rect,
                    rgba: [0x80, 0x80, 0x80, 0xFF].repeat(rect.size().area() as usize),
                })
                .collect())
        }
    }

    fn fake(
        els: Vec<UiElement>,
    ) -> (
        Box<FakeAnalyzer>,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (Box::new(FakeAnalyzer(els, n.clone())), n)
    }

    /// The default trait methods: analysis only, no healing.
    struct AnalyzeOnly(Vec<UiElement>);

    impl UiAnalyzer for AnalyzeOnly {
        fn analyze(&self, _: &Pixels, _: Rect) -> std::result::Result<Vec<UiElement>, String> {
            Ok(self.0.clone())
        }
    }

    fn el(kind: UiElementKind, rect: Rect, confidence: f32) -> UiElement {
        UiElement {
            fg: Color::rgb(0x20, 0x20, 0x20),
            bg: Color::rgb(0xF0, 0xF0, 0xF8),
            confidence,
            ..UiElement::new(kind, rect)
        }
    }

    #[test]
    fn simplify_needs_an_analyzer() {
        let mut s = session(100, 100);
        assert!(matches!(
            s.run(SIMPLIFY_ID, &json!({})),
            Err(EngineError::Disabled(..))
        ));
    }

    #[test]
    fn simplify_builds_and_replaces_wireframes() {
        let mut s = session(300, 200);
        s.set_analyzer(Box::new(AnalyzeOnly(vec![
            el(UiElementKind::Text, Rect::new(10, 10, 120, 24), 0.9),
            el(UiElementKind::Panel, Rect::new(0, 0, 300, 200), 0.8),
            el(UiElementKind::Button, Rect::new(200, 150, 280, 180), 0.95),
            el(UiElementKind::Icon, Rect::new(250, 10, 270, 30), 0.2),
            el(UiElementKind::Text, Rect::new(500, 500, 600, 520), 0.9),
            el(UiElementKind::Text, Rect::new(10, 40, 90, 54), f32::NAN),
        ])));
        let r = s.run(SIMPLIFY_ID, &json!({})).unwrap();
        assert_eq!(r["added"].as_array().map(Vec::len), Some(3));
        // No healing support: the wireframes are still built.
        assert!(r["heal_error"].is_string());
        let first = &manifest(&s).annotations[0];
        assert_eq!(first.name, "Simplified Panel");
        let r = s.run(SIMPLIFY_ID, &json!({ "palette": "mono" })).unwrap();
        assert_eq!(r["removed"], 3);
        assert_eq!(manifest(&s).annotations.len(), 3);
    }

    #[test]
    fn analysis_is_cached_per_pixel_revision() {
        let mut s = session(200, 100);
        let b = el(UiElementKind::Button, Rect::new(20, 20, 80, 50), 0.9);
        let (a, calls) = fake(vec![b.clone()]);
        s.set_analyzer(a);
        let r = s.run(DETECT_ID, &json!({})).unwrap();
        assert_eq!(r["objects"][0]["id"], b.id.0);
        assert_eq!(r["objects"][0]["kind"], "button");
        s.run(DETECT_ID, &json!({ "kinds": ["text"] })).unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        // A heal changes the pixels, so the next query analyses again.
        s.run(DELETE_ID, &json!({ "object": b.id })).unwrap();
        s.run(DETECT_ID, &json!({})).unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
        // Undo brings the old pixels back, and with them the cached analysis.
        s.run("edit.undo", &Value::Null).unwrap();
        s.run(DETECT_ID, &json!({})).unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn move_lifts_heals_and_undoes() {
        let mut s = session(200, 100);
        let b = el(UiElementKind::Button, Rect::new(20, 20, 80, 50), 0.9);
        let (a, _) = fake(vec![b.clone()]);
        s.set_analyzer(a);
        let r = s
            .run(
                MOVE_ID,
                &json!({ "object": b.id, "by": { "x": 100, "y": 10 } }),
            )
            .unwrap();
        let id = NodeId(r["id"].as_u64().unwrap());
        let doc = s.active().unwrap();
        let Some(Annotation::SmartObject(o)) = doc.manifest.node(id).and_then(|n| n.annotation())
        else {
            panic!("smart object expected");
        };
        assert_eq!((o.rect.x, o.rect.y, o.rect.w), (120.0, 30.0, 60.0));
        assert_eq!(o.source, b.rect);
        assert_eq!(doc.object_pixels(id).unwrap().size(), Size::new(60, 30));
        // Healed in the edited copy; the capture itself is untouched.
        assert_eq!(
            doc.edited_pixels().unwrap().pixel(30, 30),
            [0x80, 0x80, 0x80, 0xFF]
        );
        assert_eq!(doc.pixels.as_ref().unwrap().pixel(30, 30), [255; 4]);
        // Move it again by node, then delete it.
        s.run(
            MOVE_ID,
            &json!({ "node": id.0, "to": { "x": 0, "y": 0 }, "size": { "w": 30, "h": 15 } }),
        )
        .unwrap();
        s.run(DELETE_ID, &json!({ "node": id.0 })).unwrap();
        assert!(s.active().unwrap().object_pixels(id).is_none());
        for _ in 0..3 {
            s.run("edit.undo", &Value::Null).unwrap();
        }
        let doc = s.active().unwrap();
        assert!(doc.manifest.annotations.is_empty());
        assert_eq!(doc.edited_pixels().unwrap().pixel(30, 30), [255; 4]);
        s.run("edit.redo", &Value::Null).unwrap();
        assert!(s.active().unwrap().object_pixels(id).is_some());
    }

    #[test]
    fn bad_targets_change_nothing() {
        let mut s = session(100, 100);
        let (a, _) = fake(vec![el(UiElementKind::Text, Rect::new(5, 5, 50, 15), 0.9)]);
        s.set_analyzer(a);
        let bad = [
            (MOVE_ID, json!({})),
            (
                MOVE_ID,
                json!({ "object": 1, "rect": { "x0": 0, "y0": 0, "x1": 5, "y1": 5 } }),
            ),
            (MOVE_ID, json!({ "object": 12345 })),
            (
                MOVE_ID,
                json!({ "rect": { "x0": 200, "y0": 200, "x1": 300, "y1": 300 } }),
            ),
            (
                MOVE_ID,
                json!({ "rect": { "x0": 0, "y0": 0, "x1": 5, "y1": 5 }, "to": { "x": 1, "y": 1 }, "by": { "x": 1, "y": 1 } }),
            ),
            (
                MOVE_ID,
                json!({ "rect": { "x0": 0, "y0": 0, "x1": 5, "y1": 5 }, "size": { "w": 0, "h": 1 } }),
            ),
            (MOVE_ID, json!({ "node": 99 })),
            (DELETE_ID, json!({ "node": 99 })),
            (
                REPLACE_TEXT_ID,
                json!({ "rect": { "x0": 60, "y0": 60, "x1": 90, "y1": 90 }, "text": "x" }),
            ),
            (
                REPLACE_TEXT_ID,
                json!({ "rect": { "x0": 0, "y0": 0, "x1": 60, "y1": 20 }, "text": "x", "font": { "size": -1 } }),
            ),
            (SET_PALETTE_ID, json!({ "palette": "dark" })),
            (SIMPLIFY_ID, json!({ "palette": "sepia" })),
            (SIMPLIFY_ID, json!({ "palette": "auto" })),
        ];
        for (cmd, p) in bad {
            assert!(s.run(cmd, &p).is_err(), "{cmd} {p}");
            let d = s.active().unwrap();
            assert!(d.manifest.annotations.is_empty(), "{cmd} {p}");
            assert!(
                d.layers.edited.is_none() && d.layers.simplify.is_none(),
                "{cmd} {p}"
            );
        }
        assert!(!s.active().unwrap().can_undo());
    }

    #[test]
    fn replace_text_heals_and_adds_a_text_box() {
        let mut s = session(200, 100);
        let mut t = el(UiElementKind::Text, Rect::new(10, 10, 110, 26), 0.9);
        t.font_px = 14.0;
        t.font_weight = 600;
        t.fg = Color::rgb(0xEE, 0x11, 0x22);
        t.text = Some("Helo world".into());
        let (a, _) = fake(vec![t.clone()]);
        s.set_analyzer(a);
        let r = s
            .run(REPLACE_TEXT_ID, &json!({ "rect": { "x0": 0, "y0": 0, "x1": 120, "y1": 30 }, "text": "Hello world" }))
            .unwrap();
        assert_eq!(r["font"]["size"], 14.0);
        assert_eq!(r["font"]["weight"], 600);
        assert_eq!(r["color"], "#ee1122ff");
        assert_eq!(r["original"], "Helo world");
        let n = manifest(&s)
            .node(NodeId(r["id"].as_u64().unwrap()))
            .unwrap();
        let Some(Annotation::Callout(c)) = n.annotation() else {
            panic!("text box expected");
        };
        assert_eq!(c.text.text(), "Hello world");
        assert_eq!((c.bounds.x, c.bounds.w), (10.0, 100.0));
        assert_eq!(
            s.active().unwrap().edited_pixels().unwrap().pixel(9, 9),
            [0x80, 0x80, 0x80, 0xFF]
        );
    }

    #[test]
    fn grab_text_reads_columns_in_order() {
        let mut s = session(400, 200);
        let line = |r: Rect, t: &str| UiElement {
            text: Some(t.into()),
            ..el(UiElementKind::Text, r, 0.9)
        };
        let (a, _) = fake(vec![
            line(Rect::new(220, 40, 380, 56), "right 1"),
            line(Rect::new(10, 60, 180, 76), "left 2"),
            line(Rect::new(10, 10, 380, 26), "title"),
            line(Rect::new(10, 40, 180, 56), "left 1"),
            line(Rect::new(220, 60, 380, 76), "right 2"),
            el(UiElementKind::Button, Rect::new(10, 100, 60, 120), 0.9),
        ]);
        s.set_analyzer(a);
        let r = s.run(GRAB_TEXT_ID, &json!({})).unwrap();
        assert_eq!(r["text"], "title\nleft 1\nleft 2\nright 1\nright 2");
        assert_eq!(r["recognized"], true);
    }

    #[test]
    fn reading_order_survives_deep_staircases() {
        let rects: Vec<Rect> = (0..5000)
            .map(|i| Rect::new(i * 20, i * 20, i * 20 + 10, i * 20 + 10))
            .collect();
        let order = reading_order(&rects);
        assert_eq!(order, (0..5000).collect::<Vec<_>>());
    }

    #[test]
    fn simplify_palettes_exclude_and_restore() {
        let mut s = session(300, 200);
        let panel = el(UiElementKind::Panel, Rect::new(10, 10, 290, 190), 0.9);
        let mut button = el(UiElementKind::Button, Rect::new(200, 150, 280, 180), 0.95);
        button.parent = Some(panel.id);
        let mut label = el(UiElementKind::Text, Rect::new(215, 158, 265, 172), 0.9);
        label.parent = Some(button.id);
        let mut avatar = el(UiElementKind::Icon, Rect::new(20, 20, 60, 60), 0.9);
        avatar.circular = true;
        avatar.parent = Some(panel.id);
        let (a, _) = fake(vec![
            panel.clone(),
            button.clone(),
            label.clone(),
            avatar.clone(),
        ]);
        s.set_analyzer(a);

        let r = s.run(SIMPLIFY_ID, &json!({ "palette": "dark" })).unwrap();
        // Background, panel, button, label bar, avatar.
        assert_eq!(r["added"].as_array().map(Vec::len), Some(5));
        assert_eq!(r["healed"], 2);
        let fills = |s: &Session| -> Vec<(String, String)> {
            manifest(s)
                .annotations
                .iter()
                .filter_map(|n| match n.annotation() {
                    Some(Annotation::Shape(sh)) => match &sh.fill {
                        Fill::Solid { color } => Some((n.name.clone(), String::from(*color))),
                        _ => None,
                    },
                    _ => None,
                })
                .collect()
        };
        let f = fills(&s);
        assert!(
            f.contains(&("Simplified Button".into(), "#5865f2ff".into())),
            "{f:?}"
        );
        assert!(
            f.contains(&("Simplified Text".into(), "#ffffffff".into())),
            "{f:?}"
        );
        assert!(matches!(
            manifest(&s)
                .annotations
                .iter()
                .find(|n| n.name == "Simplified Icon")
                .and_then(|n| n.annotation()),
            Some(Annotation::Shape(ShapeNode {
                shape: RegionShape::Ellipse { .. },
                ..
            }))
        ));

        // Switching the palette recolours without touching the layout.
        let r = s
            .run(SET_PALETTE_ID, &json!({ "palette": "blueprint" }))
            .unwrap();
        assert_eq!(r["removed"], 5);
        let f = fills(&s);
        assert!(
            f.contains(&("Simplified Button".into(), "#8fd3ffff".into())),
            "{f:?}"
        );

        // Keep the button as it is: its subtree becomes one original object.
        let r = s
            .run(
                SET_SIMPLIFIED_ID,
                &json!({ "objects": [button.id], "simplified": false }),
            )
            .unwrap();
        let mut ex: Vec<u64> = vec![button.id.0, label.id.0];
        ex.sort_unstable();
        assert_eq!(r["excluded"], json!(ex));
        let originals: Vec<_> = manifest(&s)
            .annotations
            .iter()
            .filter(|n| matches!(n.annotation(), Some(Annotation::SmartObject(_))))
            .collect();
        assert_eq!(originals.len(), 1);
        assert_eq!(originals[0].name, "Original button");
        assert!(s.active().unwrap().object_pixels(originals[0].id).is_some());
        let st = s.active().unwrap().simplify_state().unwrap();
        assert_eq!(st.palette, Some(SimplifyPalette::BLUEPRINT));

        s.run(
            SET_SIMPLIFIED_ID,
            &json!({ "objects": [button.id], "simplified": true }),
        )
        .unwrap();
        assert!(
            s.active()
                .unwrap()
                .simplify_state()
                .unwrap()
                .excluded
                .is_empty()
        );
        // Undo walks back through restore, exclude, palette and the first pass.
        for _ in 0..4 {
            s.run("edit.undo", &Value::Null).unwrap();
        }
        assert!(s.active().unwrap().simplify_state().is_none());
        assert!(manifest(&s).annotations.is_empty());
    }

    #[test]
    fn sanitize_fixes_untrusted_output() {
        let region = Rect::new(0, 0, 100, 100);
        let a = el(UiElementKind::Panel, Rect::new(-50, -50, 50, 50), 0.9);
        let mut b = a.clone();
        b.corner_radius = f32::INFINITY;
        b.parent = Some(b.id);
        let mut c = el(UiElementKind::Text, Rect::new(10, 10, 20, 20), 2.0);
        c.parent = Some(UiElementId(7));
        c.lines = vec![Rect::new(0, 0, 500, 15), Rect::new(90, 90, 95, 95)];
        c.text = Some("é".repeat(MAX_TEXT_BYTES));
        let out = sanitize(vec![a, b, c], region);
        assert_eq!(out.len(), 3);
        assert_ne!(out[0].id, out[1].id);
        assert_eq!(out[0].rect, Rect::new(0, 0, 50, 50));
        assert_eq!(out[1].corner_radius, 0.0);
        // `b` pointed at its own id, which after renaming is `a`'s.
        assert_eq!(out[1].parent, Some(out[0].id));
        assert_eq!(out[2].parent, None);
        assert_eq!(out[2].confidence, 1.0);
        assert_eq!(out[2].lines, [Rect::new(10, 10, 20, 15)]);
        assert!(out[2].text.as_ref().unwrap().len() <= MAX_TEXT_BYTES);
    }
}
