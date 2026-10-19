//! CaptureCraft on-device UI analysis (L6): the computer vision behind
//! Smart Move, Simplify and Grab / Edit Text.
//!
//! - [`segment`]: a flattened screenshot into a [`UiSceneGraph`] of
//!   containers, buttons, inputs, icons and avatars, dividers, images and
//!   text blocks (pure Rust: flat-region and connected-component analysis,
//!   morphology, Sobel edges, MSER).
//! - [`inpaint`]: the background behind an element, from a ring of pixels
//!   around it (solid, gradient, bilinear), Telea fast marching or
//!   PatchMatch (`photocraft-algo`), written copy-on-write into the tiles.
//! - [`matte`]: soft alpha for lifted objects; [`palette`]: Simplify
//!   swatches quantized from the screenshot.
//! - [`OcrEngine`]: reads the words. [`neural::NeuralEngine`] (feature
//!   `neural`) runs the ocrs models fetched by `cargo xtask models` and
//!   checked by SHA-256 ([`models`]); without them [`MorphologicalEngine`]
//!   still finds every text line, so Smart Move and Simplify work out of the
//!   box and only Grab Text has nothing to read.
//!
//! [`Analyzer`] ties it together and implements the engine's
//! [`UiAnalyzer`]; the app installs it with `Session::set_analyzer`.
//! Pixels are read and healed per 256² `photocraft-raster` tile with rayon;
//! the engine caches analyses per `(layer, pixel revision)`.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod edges;
pub mod image;
pub mod inpaint;
pub mod label;
pub mod matte;
pub mod models;
pub mod morph;
pub mod mser;
#[cfg(feature = "neural")]
pub mod neural;
pub mod palette;
pub mod segment;

use capturecraft_engine::{Patch, Pixels, UiAnalyzer};
use capturecraft_format::Color;
use photocraft_geom::Rect;
use photocraft_raster::Surface;

pub use capturecraft_engine::{SimplifyPalette, UiElementId};
pub use image::RgbaImage;
pub use inpaint::{HealMethod, HealPatch, InpaintParams, inpaint_region};
pub use segment::{
    PointI32, RectI32, Rgba8, SegmentParams, UiElement, UiElementKind, UiSceneGraph,
};

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    #[error(
        "the text recognition models are not installed (set {} to the folder that holds them)",
        models::MODELS_ENV
    )]
    NoModels,
    #[error("loading {0}: {1}")]
    Load(String, String),
    #[error("text recognition failed: {0}")]
    Recognize(String),
    #[error("raster: {0}")]
    Raster(String),
    #[error("the region is larger than {} pixels", image::MAX_PIXELS)]
    TooLarge,
    #[error("the region is empty")]
    Empty,
}

/// A recognised word, image pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub rect: Rect,
    pub text: String,
}

/// A line of text in reading order, in the pixels of the image given to
/// [`OcrEngine::recognize`] (`(0, 0)` is its top-left).
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub rect: Rect,
    /// `None` from engines that only find text ([`MorphologicalEngine`]).
    pub text: Option<String>,
    pub words: Vec<Word>,
}

/// Finds (and, given models, reads) text lines.
pub trait OcrEngine: Send + Sync {
    fn name(&self) -> &'static str;
    /// Whether [`OcrEngine::recognize`] returns the words, not just boxes.
    fn reads_text(&self) -> bool;
    fn recognize(&self, img: &RgbaImage) -> Result<Vec<TextLine>, OcrError>;
}

/// The pure-Rust fallback: line boxes from the morphological detector.
#[derive(Clone, Debug, Default)]
pub struct MorphologicalEngine {
    pub params: SegmentParams,
}

impl OcrEngine for MorphologicalEngine {
    fn name(&self) -> &'static str {
        "morphological"
    }

    fn reads_text(&self) -> bool {
        false
    }

    fn recognize(&self, img: &RgbaImage) -> Result<Vec<TextLine>, OcrError> {
        let g = UiSceneGraph::analyze(img, &self.params);
        let (ox, oy) = (img.rect().x0, img.rect().y0);
        let mut lines: Vec<TextLine> = g
            .text_blocks()
            .flat_map(|e| match &e.kind {
                UiElementKind::TextBlock { lines, .. } => lines.clone(),
                _ => Vec::new(),
            })
            .map(|r| TextLine {
                rect: r.translate(-ox, -oy),
                text: None,
                words: Vec::new(),
            })
            .collect();
        lines.sort_by_key(|l| (l.rect.y0, l.rect.x0));
        Ok(lines)
    }
}

/// The UI analyzer the app installs: segmentation, text, healing, matting
/// and palettes over the document's tiles.
pub struct Analyzer {
    engine: Box<dyn OcrEngine>,
    pub segment: SegmentParams,
    pub inpaint: InpaintParams,
}

impl Analyzer {
    pub fn new(engine: Box<dyn OcrEngine>) -> Self {
        Self {
            engine,
            segment: SegmentParams::default(),
            inpaint: InpaintParams::default(),
        }
    }

    /// No text recognition; deterministic and model-free.
    pub fn morphological() -> Self {
        Self::new(Box::new(MorphologicalEngine::default()))
    }

    /// The ocrs models when they are installed and intact, otherwise the
    /// morphological fallback.
    pub fn auto() -> Self {
        #[cfg(feature = "neural")]
        if let Some(m) = models::Models::find()
            && m.verify().is_ok()
            && let Ok(e) = neural::NeuralEngine::load(&m)
        {
            return Self::new(Box::new(e));
        }
        Self::morphological()
    }

    pub fn engine_name(&self) -> &'static str {
        self.engine.name()
    }

    /// The scene graph of `region` (base space) of an RGBA8 surface, with
    /// the words filled in when the engine reads text.
    pub fn scene(&self, surface: &Surface, region: Rect) -> Result<UiSceneGraph, OcrError> {
        let img = RgbaImage::from_surface(surface, region)?;
        let mut g = UiSceneGraph::analyze(&img, &self.segment);
        if self.engine.reads_text() {
            let lines = self.engine.recognize(&img)?;
            merge_text(&mut g, &img, lines);
        }
        Ok(g)
    }
}

/// Puts recognised words into the text blocks they overlap; lines the
/// detector missed (text on photos, low contrast) become blocks of their own.
fn merge_text(g: &mut UiSceneGraph, img: &RgbaImage, lines: Vec<TextLine>) {
    let mut lines: Vec<TextLine> = lines
        .into_iter()
        .filter(|l| l.text.as_deref().is_some_and(|t| !t.trim().is_empty()))
        .map(|mut l| {
            l.rect = img.global(l.rect);
            l
        })
        .collect();
    lines.sort_by_key(|l| (l.rect.y0, l.rect.x0));
    let mut used = vec![false; lines.len()];
    let mut fill = |e: &mut UiElement| {
        let UiElementKind::TextBlock { text, .. } = &mut e.kind else {
            return;
        };
        let mut got = Vec::new();
        for (k, l) in lines.iter().enumerate() {
            let a = l.rect.intersect(&e.bounds).size().area();
            if !used[k] && a * 2 >= l.rect.size().area() && a > 0 {
                used[k] = true;
                got.push(l.text.clone().unwrap_or_default());
            }
        }
        if !got.is_empty() {
            *text = Some(got.join("\n"));
            e.confidence = (e.confidence + 0.1).min(1.0);
        }
    };
    for e in &mut g.elements {
        fill(e);
        if let UiElementKind::Button { label: Some(l), .. } = &mut e.kind {
            fill(l);
        }
    }
    let containers: Vec<(Rect, UiElementId, Color)> = g
        .elements
        .iter()
        .filter_map(|e| match &e.kind {
            UiElementKind::Container { fill, .. } => Some((e.bounds, e.id, *fill)),
            UiElementKind::Button { fill_color, .. } => Some((e.bounds, e.id, *fill_color)),
            UiElementKind::InputField { bg_color, .. } => Some((e.bounds, e.id, *bg_color)),
            UiElementKind::Image { .. } => Some((e.bounds, e.id, e.background)),
            _ => None,
        })
        .collect();
    for (k, l) in lines.into_iter().enumerate() {
        if used[k] {
            continue;
        }
        let host = containers
            .iter()
            .filter(|(r, _, _)| r.contains_rect(&l.rect))
            .min_by_key(|(r, _, _)| r.size().area());
        let bg = host.map_or(g.background, |h| h.2);
        let local = img.local(l.rect);
        let px: Vec<image::Px> = (local.y0..local.y1)
            .flat_map(|y| (local.x0..local.x1).map(move |x| (x, y)))
            .map(|(x, y)| img.get(x, y))
            .collect();
        let b = segment_rgba(bg);
        let far = px.iter().map(|p| image::dist(*p, b)).max().unwrap_or(0);
        let ink: Vec<&image::Px> = px
            .iter()
            .filter(|p| u16::from(image::dist(**p, b)) * 10 >= u16::from(far) * 6)
            .collect();
        let n = ink.len().max(1) as u32;
        let fg: [u8; 4] =
            std::array::from_fn(|c| (ink.iter().map(|p| u32::from(p[c])).sum::<u32>() / n) as u8);
        let kind = UiElementKind::TextBlock {
            text: l.text,
            font_size_px: (l.rect.height() as f32 / 0.9).clamp(4.0, 400.0),
            font_weight: 400,
            dominant_fg: Color::rgba(fg[0], fg[1], fg[2], fg[3]),
            dominant_bg: bg,
            lines: vec![l.rect],
        };
        let mut e = UiElement {
            id: UiElementId::of(capturecraft_engine::UiElementKind::Text, l.rect),
            parent: host.map(|h| h.1),
            bounds: l.rect,
            background: bg,
            confidence: 0.8,
            kind,
        };
        if g.get(e.id).is_some() {
            e.id = UiElementId(e.id.0 ^ 1);
        }
        g.elements.push(e);
    }
}

fn segment_rgba(c: Color) -> image::Px {
    [c.r, c.g, c.b, c.a]
}

impl UiAnalyzer for Analyzer {
    fn name(&self) -> &str {
        self.engine.name()
    }

    fn analyze(
        &self,
        pixels: &Pixels,
        region: Rect,
    ) -> Result<Vec<capturecraft_engine::UiElement>, String> {
        let region = region.intersect(&pixels.bounds());
        self.scene(pixels.surface(), region)
            .map(|g| g.to_engine())
            .map_err(|e| e.to_string())
    }

    fn heal(
        &self,
        pixels: &Pixels,
        holes: &[Rect],
        protect: &[Rect],
    ) -> Result<Vec<Patch>, String> {
        let bounds = pixels.bounds();
        let holes: Vec<Rect> = holes
            .iter()
            .map(|h| h.intersect(&bounds))
            .filter(|h| !h.is_empty())
            .collect();
        inpaint::heal_all(pixels.surface(), bounds, &holes, protect, &self.inpaint)
            .into_iter()
            .map(|r| {
                r.map(|p| Patch {
                    rect: p.rect,
                    rgba: p.rgba,
                })
                .map_err(|e| e.to_string())
            })
            .collect()
    }

    fn extract(&self, pixels: &Pixels, rect: Rect, bg: Option<Color>) -> Result<Patch, String> {
        let bounds = pixels.bounds();
        let rect = rect.intersect(&bounds);
        if rect.is_empty() {
            return Err("the object lies outside the capture".into());
        }
        let window = rect.inflate(2).intersect(&bounds);
        let img = RgbaImage::from_surface(pixels.surface(), window).map_err(|e| e.to_string())?;
        let local = img.local(rect);
        let bg = bg
            .map(segment_rgba)
            .or_else(|| matte::surrounding(&img, local));
        let crop =
            RgbaImage::from_rgba8(rect, &pixels.read_rgba8(rect).map_err(|e| e.to_string())?)
                .ok_or("pixel buffer size mismatch")?;
        let rgba = match bg {
            Some(b) => matte::matte(&crop, b),
            // At the capture's edge with nothing around: lift as is.
            None => crop.to_rgba8(),
        };
        Ok(Patch { rect, rgba })
    }

    fn palette(&self, pixels: &Pixels, region: Rect) -> Result<SimplifyPalette, String> {
        let img = RgbaImage::from_surface(pixels.surface(), region.intersect(&pixels.bounds()))
            .map_err(|e| e.to_string())?;
        palette::extract(&img)
    }
}
