//! The CaptureCraft engine: open documents plus the command registry.
//!
//! **Everything is a command.** The editor UI, `capturecraft-cli`, the JSON
//! control channel and the MCP server all call [`Session::run`] with a
//! command id and JSON parameters; nothing edits a document any other way.
//! A command either succeeds completely or leaves the document untouched:
//! [`Session::run`] snapshots the manifest first, validates it afterwards
//! and restores the snapshot on any error.
//!
//! OS-specific work (screen capture, OCR / UI segmentation) lives in the L6
//! crates, which implement the traits defined here ([`UiAnalyzer`]); the
//! app installs them into the session.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod commands;

use capturecraft_format::{Color, FormatError, Manifest};
use photocraft_geom::{Rect, Size};
use serde_json::Value;

pub use commands::{CommandSpec, registry};

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

/// The base screenshot in memory: straight-alpha RGBA8, row-major.
/// (`photocraft-raster` tiles replace this when the compositor lands.)
#[derive(Clone, Debug, PartialEq)]
pub struct Pixels {
    size: Size,
    rgba: Vec<u8>,
}

impl Pixels {
    /// `None` when `rgba` is not exactly `width × height × 4` bytes.
    pub fn new(size: Size, rgba: Vec<u8>) -> Option<Self> {
        let len = usize::try_from(size.area()).ok()?.checked_mul(4)?;
        (rgba.len() == len).then_some(Self { size, rgba })
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

/// What a UI analyzer recognised in a screenshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiElementKind {
    /// One line of text.
    Text,
    Button,
    Icon,
    /// Text field, checkbox, dropdown and similar controls.
    Input,
    /// Toolbars, cards, sidebars: large flat areas that contain others.
    Panel,
    /// Photos and other content that has no wireframe equivalent.
    Image,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiElement {
    pub kind: UiElementKind,
    /// Base-space pixels.
    pub rect: Rect,
    /// Dominant foreground (text/glyph) and background colours.
    pub fg: Color,
    pub bg: Color,
    pub confidence: f32,
    /// Recognised text, for Grab Text and search.
    pub text: Option<String>,
}

/// UI deconstruction (connected components, edges, OCR), implemented by
/// `capturecraft-ocr`. Output is treated as untrusted: the engine clamps,
/// filters and caps it.
pub trait UiAnalyzer: Send + Sync {
    fn analyze(&self, pixels: &Pixels, region: Rect)
    -> std::result::Result<Vec<UiElement>, String>;
}

/// One open document with its undo history.
pub struct DocState {
    pub manifest: Manifest,
    pub pixels: Option<Pixels>,
    history: History,
}

impl DocState {
    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.history.undo.last().map(|(l, _)| l.as_str())
    }
}

/// Labelled snapshots, like `photocraft-ops`: redo is cleared by the next
/// edit and the oldest states fall off past [`History::MAX_STATES`].
#[derive(Default)]
struct History {
    undo: Vec<(String, Manifest)>,
    redo: Vec<(String, Manifest)>,
}

impl History {
    const MAX_STATES: usize = 100;

    fn record(&mut self, label: &str, before: Manifest) {
        self.undo.push((label.to_string(), before));
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
            history: History::default(),
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

    /// Runs a command by id. On error nothing has changed.
    pub fn run(&mut self, id: &str, params: &Value) -> Result<Value> {
        let spec = registry()
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        (spec.enabled)(self).map_err(|why| EngineError::Disabled(id.to_string(), why))?;
        let before = if spec.journal {
            self.active().map(|d| d.manifest.clone())
        } else {
            None
        };
        let result = (spec.run)(self, params).and_then(|v| {
            match self.active() {
                Some(d) if spec.journal => d.manifest.validate()?,
                _ => {}
            }
            Ok(v)
        });
        if let (Some(before), Some(doc)) = (before, self.active_mut()) {
            match &result {
                Ok(_) => doc.history.record(spec.label, before),
                Err(_) => doc.manifest = before,
            }
        }
        result
    }

    pub(crate) fn undo(&mut self) -> Result<Value> {
        let doc = self.active_mut().ok_or(EngineError::NoDocument)?;
        let (label, state) = doc
            .history
            .undo
            .pop()
            .ok_or_else(|| EngineError::Other("nothing to undo".into()))?;
        let current = std::mem::replace(&mut doc.manifest, state);
        doc.history.redo.push((label.clone(), current));
        Ok(serde_json::json!({ "undone": label }))
    }

    pub(crate) fn redo(&mut self) -> Result<Value> {
        let doc = self.active_mut().ok_or(EngineError::NoDocument)?;
        let (label, state) = doc
            .history
            .redo
            .pop()
            .ok_or_else(|| EngineError::Other("nothing to redo".into()))?;
        let current = std::mem::replace(&mut doc.manifest, state);
        doc.history.undo.push((label.clone(), current));
        Ok(serde_json::json!({ "redone": label }))
    }
}
