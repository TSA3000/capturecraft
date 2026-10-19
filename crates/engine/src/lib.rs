//! The CaptureCraft engine: open documents plus the command registry.
//!
//! **Everything is a command.** The editor UI, `capturecraft-cli`, the JSON
//! control channel and the MCP server all call [`Session::run`] with a
//! command id and JSON parameters; nothing edits a document any other way.
//! A command either succeeds completely or leaves the document untouched:
//! [`Session::run`] snapshots the manifest and the raster layers first,
//! validates the manifest afterwards and restores the snapshot on any error.
//!
//! OS-specific work (screen capture, OCR / UI segmentation) lives in the L6
//! crates, which implement the traits defined here ([`UiAnalyzer`]); the
//! app installs them into the session.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod commands;
pub mod edit_commands;
pub mod node_commands;
pub mod render;
pub mod smart_commands;
pub mod transform_commands;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use capturecraft_format::{Color, FormatError, Manifest, NodeId};
use photocraft_color::PixelFormat;
use photocraft_geom::{Rect, Size, TILE_SIZE};
use photocraft_raster::Surface;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use commands::{CommandSpec, registry};
pub use smart_commands::{SimplifyPalette, SimplifyState};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("command `{0}` is not available right now: {1}")]
    Disabled(String, String),
    #[error("invalid parameters for `{cmd}`: {msg}")]
    BadParams { cmd: String, msg: String },
    #[error("no active document")]
    NoDocument,
    #[error("no annotation with id {0}")]
    NoNode(u64),
    #[error("document is invalid after the edit: {0}")]
    Invalid(#[from] FormatError),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Source of [`Pixels::revision`]: every write gets a number never used
/// before, so an undo that brings old pixels back also brings back their
/// revision, and caches keyed by it stay valid.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

/// A raster in memory: straight-alpha RGBA8 `photocraft-raster` tiles
/// (256 × 256, copy-on-write). Cloning shares every tile; a write copies
/// only the tiles it touches, so undo snapshots and healed copies of the
/// base cost memory only where pixels differ.
#[derive(Clone, Debug)]
pub struct Pixels {
    size: Size,
    surface: Surface,
    revision: u64,
}

impl PartialEq for Pixels {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size && self.surface == other.surface
    }
}

impl Pixels {
    /// `None` when `rgba` is not exactly `width × height × 4` bytes or the
    /// tiles cannot be allocated.
    pub fn new(size: Size, rgba: Vec<u8>) -> Option<Self> {
        let len = usize::try_from(size.area()).ok()?.checked_mul(4)?;
        if rgba.len() != len {
            return None;
        }
        let mut p = Self::transparent(size);
        p.write_rgba8(Rect::from_size(size), &rgba).ok()?;
        Some(p)
    }

    /// Fully transparent pixels (no tiles allocated).
    pub fn transparent(size: Size) -> Self {
        Self {
            size,
            surface: Surface::new(PixelFormat::RGBA8),
            revision: next_revision(),
        }
    }

    /// `None` unless `surface` is RGBA8.
    pub fn from_surface(size: Size, surface: Surface) -> Option<Self> {
        (surface.format() == PixelFormat::RGBA8).then(|| Self {
            size,
            surface,
            revision: next_revision(),
        })
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn bounds(&self) -> Rect {
        Rect::from_size(self.size)
    }

    pub fn surface(&self) -> &Surface {
        &self.surface
    }

    /// Changes on every write; equal revisions mean equal pixels.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Interleaved RGBA8 of `r` (clipped to the bounds; empty when outside).
    pub fn read_rgba8(&self, r: Rect) -> std::result::Result<Vec<u8>, String> {
        let r = r.intersect(&self.bounds());
        if r.is_empty() {
            return Ok(Vec::new());
        }
        self.surface
            .try_to_interleaved(r)
            .map_err(|e| e.to_string())
    }

    /// One pixel; transparent outside the bounds.
    pub fn pixel(&self, x: i32, y: i32) -> [u8; 4] {
        let mut px = [0u8; 4];
        if let Ok(v) = self.read_rgba8(Rect::new(x, y, x + 1, y + 1))
            && let Some(p) = v.get(..4)
        {
            px.copy_from_slice(p);
        }
        px
    }

    /// Writes interleaved RGBA8 into `r`, which must lie inside the bounds;
    /// tiles shared with a clone are copied first.
    pub fn write_rgba8(&mut self, r: Rect, rgba: &[u8]) -> std::result::Result<(), String> {
        if r.is_empty() {
            return Ok(());
        }
        if !self.bounds().contains_rect(&r) {
            return Err("write outside the raster".into());
        }
        let w = r.width() as usize;
        let len = w
            .checked_mul(r.height() as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or("raster write too large")?;
        if rgba.len() != len {
            return Err(format!("expected {len} bytes, got {}", rgba.len()));
        }
        let t = TILE_SIZE as usize;
        for tc in r.tiles() {
            let tr = tc.rect().intersect(&r);
            let (ox, oy) = (tc.tx * TILE_SIZE, tc.ty * TILE_SIZE);
            let span = tr.width() as usize * 4;
            let bytes = self
                .surface
                .try_tile_mut(tc)
                .map_err(|e| e.to_string())?
                .bytes_mut();
            for y in tr.y0..tr.y1 {
                let dst = ((y - oy) as usize * t + (tr.x0 - ox) as usize) * 4;
                let src = ((y - r.y0) as usize * w + (tr.x0 - r.x0) as usize) * 4;
                if let (Some(d), Some(s)) =
                    (bytes.get_mut(dst..dst + span), rgba.get(src..src + span))
                {
                    d.copy_from_slice(s);
                }
            }
        }
        self.revision = next_revision();
        Ok(())
    }

    /// Every pixel, interleaved RGBA8 (tests and small exports).
    pub fn to_rgba8(&self) -> std::result::Result<Vec<u8>, String> {
        self.read_rgba8(self.bounds())
    }
}

/// Stable id of a detected UI element: a hash of its kind and rectangle,
/// so the same pixels give the same ids in the UI, the CLI and over MCP.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct UiElementId(pub u64);

impl UiElementId {
    pub fn of(kind: UiElementKind, rect: Rect) -> Self {
        // FNV-1a: fixed across platforms and releases (unlike `DefaultHasher`).
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |v: u64| {
            for b in v.to_le_bytes() {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        eat(kind as u64);
        for c in [rect.x0, rect.y0, rect.x1, rect.y1] {
            eat(c as u32 as u64);
        }
        // Small enough for JSON numbers (2^53) to round-trip exactly.
        Self(h >> 11)
    }
}

/// What a UI analyzer recognised in a screenshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiElementKind {
    /// A block of text: one or more lines ([`UiElement::lines`]).
    Text,
    Button,
    /// Icons, avatars and badges ([`UiElement::circular`] for round ones).
    Icon,
    /// Text field, checkbox, dropdown and similar controls.
    Input,
    /// Toolbars, cards, sidebars: large flat areas that contain others.
    Panel,
    /// Photos and other content that has no wireframe equivalent.
    Image,
    /// A horizontal or vertical rule.
    Divider,
}

impl UiElementKind {
    pub fn name(self) -> &'static str {
        match self {
            UiElementKind::Text => "text",
            UiElementKind::Button => "button",
            UiElementKind::Icon => "icon",
            UiElementKind::Input => "input",
            UiElementKind::Panel => "panel",
            UiElementKind::Image => "image",
            UiElementKind::Divider => "divider",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UiElement {
    pub id: UiElementId,
    /// The element this one sits in (a button's label, a card's text).
    pub parent: Option<UiElementId>,
    pub kind: UiElementKind,
    /// Base-space pixels.
    pub rect: Rect,
    /// Ink (text, glyph, line) colour.
    pub fg: Color,
    /// The element's own fill for panels, buttons and inputs; the
    /// background behind it for text, icons and dividers.
    pub bg: Color,
    pub border: Option<Color>,
    pub corner_radius: f32,
    /// Round icons and avatars.
    pub circular: bool,
    /// Text lines, base space (text blocks only).
    pub lines: Vec<Rect>,
    /// Estimated font size and weight (text only; 0 when unknown).
    pub font_px: f32,
    pub font_weight: u16,
    pub confidence: f32,
    /// Recognised text, for Grab Text and search.
    pub text: Option<String>,
}

impl UiElement {
    /// An element with neutral colours; `id` derived from kind and rect.
    pub fn new(kind: UiElementKind, rect: Rect) -> Self {
        Self {
            id: UiElementId::of(kind, rect),
            parent: None,
            kind,
            rect,
            fg: Color::BLACK,
            bg: Color::WHITE,
            border: None,
            corner_radius: 0.0,
            circular: false,
            lines: Vec::new(),
            font_px: 0.0,
            font_weight: 0,
            confidence: 1.0,
            text: None,
        }
    }
}

/// Pixels for a rectangle: a healed background or a lifted object.
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    pub rect: Rect,
    /// Interleaved straight-alpha RGBA8, `rect.width() × rect.height()`.
    pub rgba: Vec<u8>,
}

/// UI deconstruction (connected components, edges, OCR) and background
/// healing, implemented by `capturecraft-ocr`. Output is treated as
/// untrusted: the engine clamps, filters and caps it.
pub trait UiAnalyzer: Send + Sync {
    /// Short name for status lines (`morphological`, `ocrs`).
    fn name(&self) -> &str {
        "analyzer"
    }

    fn analyze(&self, pixels: &Pixels, region: Rect)
    -> std::result::Result<Vec<UiElement>, String>;

    /// The background behind each hole, reconstructed from the pixels
    /// around it; pixels inside `protect` (other foreground) are not used.
    /// One patch per hole, each covering the hole clipped to the pixels.
    fn heal(
        &self,
        _pixels: &Pixels,
        _holes: &[Rect],
        _protect: &[Rect],
    ) -> std::result::Result<Vec<Patch>, String> {
        Err("this analyzer cannot heal backgrounds".into())
    }

    /// Lifts `rect` off its background with a soft alpha matte (`bg`: the
    /// background colour, or `None` to estimate it). The default copies
    /// the pixels as they are.
    fn extract(
        &self,
        pixels: &Pixels,
        rect: Rect,
        _bg: Option<Color>,
    ) -> std::result::Result<Patch, String> {
        let rect = rect.intersect(&pixels.bounds());
        Ok(Patch {
            rect,
            rgba: pixels.read_rgba8(rect)?,
        })
    }

    /// A Simplify palette quantized from the pixels in `region`.
    fn palette(
        &self,
        _pixels: &Pixels,
        _region: Rect,
    ) -> std::result::Result<SimplifyPalette, String> {
        Err("this analyzer cannot extract palettes".into())
    }
}

/// Raster state edited by commands; journaled with the manifest.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Layers {
    /// Copy-on-write copy of the base with Smart Move / Delete / Edit Text
    /// heals; `None` until the first heal.
    pub edited: Option<Pixels>,
    /// Pixels of [`capturecraft_format::SmartObject`] nodes.
    pub objects: BTreeMap<NodeId, Pixels>,
    pub simplify: Option<SimplifyState>,
}

/// Cached analyzer output, keyed by the analysed pixels' revision (the
/// `(layer, tile grid revision)` of the edited base).
struct Analysis {
    revision: u64,
    region: Rect,
    elements: Arc<Vec<UiElement>>,
}

/// One open document with its undo history.
pub struct DocState {
    pub manifest: Manifest,
    /// The capture as taken. Commands never write it: heals go to
    /// copy-on-write layers on top ([`DocState::edited_pixels`]).
    pub pixels: Option<Pixels>,
    layers: Layers,
    history: History,
    /// Most recent last; a few entries so undo/redo hit the cache too.
    analysis: Vec<Analysis>,
}

impl DocState {
    const CACHED_ANALYSES: usize = 4;

    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    /// The undo stack, oldest first (each label is an edit that was made),
    /// and the redo stack, next first (edits that were undone).
    pub fn history_labels(&self) -> (Vec<&str>, Vec<&str>) {
        let undo = self.history.undo.iter().map(|s| s.label.as_str()).collect();
        let redo = self
            .history
            .redo
            .iter()
            .rev()
            .map(|s| s.label.as_str())
            .collect();
        (undo, redo)
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.history.undo.last().map(|s| s.label.as_str())
    }

    /// The base with Smart Move / Delete / Edit Text heals: what the
    /// analyzer sees.
    pub fn edited_pixels(&self) -> Option<&Pixels> {
        self.layers.edited.as_ref().or(self.pixels.as_ref())
    }

    /// What the compositor draws under the annotations: the edited base
    /// plus the Simplify heals.
    pub fn display_pixels(&self) -> Option<&Pixels> {
        self.layers
            .simplify
            .as_ref()
            .and_then(|s| s.pixels.as_ref())
            .or_else(|| self.edited_pixels())
    }

    /// Pixels of a smart object node.
    pub fn object_pixels(&self, id: NodeId) -> Option<&Pixels> {
        self.layers.objects.get(&id)
    }

    pub fn simplify_state(&self) -> Option<&SimplifyState> {
        self.layers.simplify.as_ref()
    }

    pub(crate) fn snapshot(&self, label: &str) -> Snapshot {
        Snapshot {
            label: label.to_string(),
            manifest: self.manifest.clone(),
            layers: self.layers.clone(),
        }
    }

    fn restore(&mut self, s: Snapshot) -> Snapshot {
        Snapshot {
            label: s.label.clone(),
            manifest: std::mem::replace(&mut self.manifest, s.manifest),
            layers: std::mem::replace(&mut self.layers, s.layers),
        }
    }
}

pub(crate) struct Snapshot {
    label: String,
    manifest: Manifest,
    layers: Layers,
}

/// Labelled snapshots, like `photocraft-ops`: redo is cleared by the next
/// edit and the oldest states fall off past [`History::MAX_STATES`].
/// Raster layers share tiles with the live document, so a snapshot costs
/// only the tiles a later edit replaced.
#[derive(Default)]
struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl History {
    const MAX_STATES: usize = 100;

    fn record(&mut self, before: Snapshot) {
        self.undo.push(before);
        self.redo.clear();
        if self.undo.len() > Self::MAX_STATES {
            self.undo.remove(0);
        }
    }
}

/// All open documents (the Recent Tray) plus installed services.
#[derive(Default)]
pub struct Session {
    docs: Vec<DocState>,
    active: Option<usize>,
    analyzer: Option<Box<dyn UiAnalyzer>>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_analyzer(&mut self, analyzer: Box<dyn UiAnalyzer>) {
        self.analyzer = Some(analyzer);
        for d in &mut self.docs {
            d.analysis.clear();
        }
    }

    pub fn analyzer(&self) -> Option<&dyn UiAnalyzer> {
        self.analyzer.as_deref()
    }

    /// Opens a validated document and makes it active; returns its index.
    pub fn open(&mut self, manifest: Manifest, pixels: Option<Pixels>) -> Result<usize> {
        manifest.validate()?;
        if let Some(p) = &pixels
            && p.size() != manifest.base.size
        {
            return Err(EngineError::Other(format!(
                "pixels are {}×{}, the manifest says {}×{}",
                p.size().width,
                p.size().height,
                manifest.base.size.width,
                manifest.base.size.height
            )));
        }
        self.docs.push(DocState {
            manifest,
            pixels,
            layers: Layers::default(),
            history: History::default(),
            analysis: Vec::new(),
        });
        let i = self.docs.len() - 1;
        self.active = Some(i);
        Ok(i)
    }

    pub fn documents(&self) -> &[DocState] {
        &self.docs
    }

    /// Switches the active document (Recent Tray click): no save prompt.
    pub fn activate(&mut self, index: usize) -> Result<()> {
        if index >= self.docs.len() {
            return Err(EngineError::NoDocument);
        }
        self.active = Some(index);
        Ok(())
    }

    /// Closes the document at `index` (unsaved changes are the caller's
    /// question). The active document stays active; closing it activates
    /// its neighbour, the next one if there is one.
    pub fn close(&mut self, index: usize) -> Result<()> {
        if index >= self.docs.len() {
            return Err(EngineError::NoDocument);
        }
        self.docs.remove(index);
        self.active = match self.active {
            _ if self.docs.is_empty() => None,
            Some(a) if a > index => Some(a - 1),
            Some(a) if a == index => Some(index.min(self.docs.len() - 1)),
            other => other,
        };
        Ok(())
    }

    /// Opens a copy of the document at `index` as `id`, edits included
    /// but with a fresh undo history, and makes it active.
    pub fn duplicate(&mut self, index: usize, id: &str) -> Result<usize> {
        let src = self.docs.get(index).ok_or(EngineError::NoDocument)?;
        let mut manifest = src.manifest.clone();
        manifest.id = id.to_string();
        let copy = DocState {
            manifest,
            pixels: src.pixels.clone(),
            layers: src.layers.clone(),
            history: History::default(),
            analysis: Vec::new(),
        };
        self.docs.push(copy);
        let i = self.docs.len() - 1;
        self.active = Some(i);
        Ok(i)
    }

    pub fn active(&self) -> Option<&DocState> {
        self.active.and_then(|i| self.docs.get(i))
    }

    pub fn active_mut(&mut self) -> Option<&mut DocState> {
        self.active.and_then(|i| self.docs.get_mut(i))
    }

    pub(crate) fn manifest_mut(&mut self) -> Result<&mut Manifest> {
        self.active_mut()
            .map(|d| &mut d.manifest)
            .ok_or(EngineError::NoDocument)
    }

    pub(crate) fn layers_mut(&mut self) -> Result<&mut Layers> {
        self.active_mut()
            .map(|d| &mut d.layers)
            .ok_or(EngineError::NoDocument)
    }

    /// The analyzer's elements for `region` of the edited base, cached per
    /// pixel revision (re-running a command on unchanged pixels does not
    /// segment again).
    pub(crate) fn analysis(&mut self, region: Rect) -> Result<Arc<Vec<UiElement>>> {
        let doc = self.active().ok_or(EngineError::NoDocument)?;
        let pixels = doc
            .edited_pixels()
            .ok_or_else(|| EngineError::Other("the capture's pixels are not loaded".into()))?;
        let rev = pixels.revision();
        if let Some(a) = doc
            .analysis
            .iter()
            .find(|a| a.revision == rev && a.region == region)
        {
            return Ok(a.elements.clone());
        }
        let analyzer = self.analyzer().ok_or_else(|| {
            EngineError::Other("no UI analyzer installed (capturecraft-ocr)".into())
        })?;
        let raw = analyzer
            .analyze(pixels, region)
            .map_err(|e| EngineError::Other(format!("UI analysis failed: {e}")))?;
        let elements = Arc::new(smart_commands::sanitize(raw, region));
        if let Some(d) = self.active_mut() {
            if d.analysis.len() >= DocState::CACHED_ANALYSES {
                d.analysis.remove(0);
            }
            d.analysis.push(Analysis {
                revision: rev,
                region,
                elements: elements.clone(),
            });
        }
        Ok(elements)
    }

    /// Runs a command by id. On error nothing has changed.
    pub fn run(&mut self, id: &str, params: &Value) -> Result<Value> {
        let spec = registry()
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        (spec.enabled)(self).map_err(|why| EngineError::Disabled(id.to_string(), why))?;
        let before = if spec.journal {
            self.active().map(|d| d.snapshot(spec.label))
        } else {
            None
        };
        let result = (spec.run)(self, params).and_then(|v| {
            if let Some(d) = self.active_mut()
                && spec.journal
            {
                d.manifest.validate()?;
                // Objects whose node was deleted go with it (undo brings both back).
                let m = &d.manifest;
                d.layers.objects.retain(|id, _| m.node(*id).is_some());
            }
            Ok(v)
        });
        if let (Some(before), Some(doc)) = (before, self.active_mut()) {
            match &result {
                Ok(_) => doc.history.record(before),
                Err(_) => {
                    doc.restore(before);
                }
            }
        }
        result
    }

    pub(crate) fn undo(&mut self) -> Result<Value> {
        let doc = self.active_mut().ok_or(EngineError::NoDocument)?;
        let state = doc
            .history
            .undo
            .pop()
            .ok_or_else(|| EngineError::Other("nothing to undo".into()))?;
        let label = state.label.clone();
        let current = doc.restore(state);
        doc.history.redo.push(current);
        Ok(serde_json::json!({ "undone": label }))
    }

    pub(crate) fn redo(&mut self) -> Result<Value> {
        let doc = self.active_mut().ok_or(EngineError::NoDocument)?;
        let state = doc
            .history
            .redo
            .pop()
            .ok_or_else(|| EngineError::Other("nothing to redo".into()))?;
        let label = state.label.clone();
        let current = doc.restore(state);
        doc.history.undo.push(current);
        Ok(serde_json::json!({ "redone": label }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn pixels_round_trip_and_share_tiles() {
        let size = Size::new(300, 20);
        let rgba: Vec<u8> = (0..300 * 20 * 4).map(|i| (i % 251) as u8).collect();
        let a = Pixels::new(size, rgba.clone()).unwrap();
        assert_eq!(a.to_rgba8().unwrap(), rgba);
        assert!(Pixels::new(size, vec![0; 7]).is_none());
        let mut b = a.clone();
        assert_eq!(a.revision(), b.revision());
        b.write_rgba8(Rect::new(290, 0, 291, 1), &[1, 2, 3, 4])
            .unwrap();
        assert_ne!(a.revision(), b.revision());
        assert_eq!(b.pixel(290, 0), [1, 2, 3, 4]);
        assert_ne!(a.pixel(290, 0), [1, 2, 3, 4]);
        // Only the touched tile was copied.
        let shared = |c| Arc::ptr_eq(a.surface().tile(c).unwrap(), b.surface().tile(c).unwrap());
        assert!(shared(photocraft_geom::TileCoord::new(0, 0)));
        assert!(!shared(photocraft_geom::TileCoord::new(1, 0)));
        assert!(b.write_rgba8(Rect::new(299, 19, 301, 20), &[0; 8]).is_err());
    }

    #[test]
    fn element_ids_are_stable_and_json_safe() {
        let r = Rect::new(10, 20, 30, 40);
        let a = UiElementId::of(UiElementKind::Button, r);
        assert_eq!(a, UiElementId::of(UiElementKind::Button, r));
        assert_ne!(a, UiElementId::of(UiElementKind::Text, r));
        assert!(a.0 < 1 << 53);
    }

    #[test]
    fn close_keeps_the_active_document_and_duplicate_copies_edits() {
        let mut s = Session::new();
        let size = Size::new(8, 8);
        for id in ["a", "b", "c"] {
            let mut m = Manifest::new(size);
            m.id = id.into();
            s.open(m, Pixels::new(size, vec![200; 8 * 8 * 4])).unwrap();
        }
        let ids = |s: &Session| -> Vec<String> {
            s.documents()
                .iter()
                .map(|d| d.manifest.id.clone())
                .collect()
        };
        s.activate(2).unwrap();
        s.close(0).unwrap();
        assert_eq!(ids(&s), ["b", "c"]);
        assert_eq!(s.active().unwrap().manifest.id, "c");
        s.close(1).unwrap();
        assert_eq!(s.active().unwrap().manifest.id, "b");
        assert!(s.close(5).is_err());

        s.run("document.setTags", &serde_json::json!({ "tags": ["x"] }))
            .unwrap();
        let i = s.duplicate(0, "b2").unwrap();
        let d = &s.documents()[i];
        assert_eq!((d.manifest.id.as_str(), i), ("b2", 1));
        assert_eq!(d.manifest.library.tags, ["x"]);
        assert!(!d.can_undo(), "a copy starts a fresh history");
        s.close(0).unwrap();
        s.close(0).unwrap();
        assert!(s.active().is_none());
    }
}
