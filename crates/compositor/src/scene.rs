//! The render graph handed down by the engine (L5) or the UI (L7).
//!
//! A [`RenderScene`] is a flat, bottom-to-top list of [`SceneNode`]s over a
//! canvas. Every node carries a stable [`NodeId`] and a [`RevisionId`] that
//! the caller bumps whenever the node's content changes; both compositors
//! key their caches on that pair, so an unchanged node is never re-prepared
//! or re-uploaded.
//!
//! Coordinates are canvas pixels, y down, with pixel `(x, y)` covering the
//! square `[x, x + 1) × [y, y + 1)` (so its centre is `(x + 0.5, y + 0.5)`).
//! Colours are straight (non-premultiplied) sRGB-encoded floats; the
//! compositors blend premultiplied values in that encoding, like the editor
//! and like Snagit.
//!
//! The scene is plain data: no engine, format or UI types leak in here.

use std::sync::Arc;

pub use photocraft_doc::{LineCap, LineJoin};
pub use photocraft_geom::Affine;
use photocraft_geom::Rect;
use photocraft_raster::Surface;

/// Stable identity of a node across frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

/// Content revision of a node; bump it on every change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct RevisionId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PointF32 {
    pub x: f32,
    pub y: f32,
}

impl PointF32 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub(crate) fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    pub(crate) fn f64(self) -> (f64, f64) {
        (f64::from(self.x), f64::from(self.y))
    }
}

/// Axis-aligned rectangle in canvas pixels (origin + size).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct RectF32 {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF32 {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn center(&self) -> PointF32 {
        PointF32::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub(crate) fn is_valid(&self) -> bool {
        [self.x, self.y, self.w, self.h]
            .iter()
            .all(|v| v.is_finite())
            && self.w > 0.0
            && self.h > 0.0
    }
}

/// Straight-alpha sRGB colour, every channel `0..=1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color::new(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: Color = Color::new(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Color = Color::new(1.0, 1.0, 1.0, 1.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self::new(
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            f32::from(a) / 255.0,
        )
    }

    /// Premultiplied, clamped to `0..=1` (non-finite channels become 0).
    pub fn premultiplied(&self) -> [f32; 4] {
        let c = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let a = c(self.a);
        [c(self.r) * a, c(self.g) * a, c(self.b) * a, a]
    }
}

/// One colour stop of a gradient; `offset` is `0..=1` along it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Color,
}

/// What a fill paints with. Gradient geometry is in canvas pixels (frame
/// pixels for a [`ShadowFrameSpec`] backdrop) and is not moved by the
/// node's transform: the engine passes it already placed. Stops are
/// interpolated in straight sRGB-encoded floats, like Snagit and CSS.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Solid(Color),
    /// Offset 0 at `from`, 1 at `to`, constant beyond either end.
    Linear {
        from: PointF32,
        to: PointF32,
        stops: Vec<GradientStop>,
    },
    /// Offset 0 at `center`, 1 at `radius`, constant beyond.
    Radial {
        center: PointF32,
        radius: f32,
        stops: Vec<GradientStop>,
    },
}

impl From<Color> for Paint {
    fn from(c: Color) -> Self {
        Paint::Solid(c)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextVAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

/// A run of uniformly styled text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRunSpec {
    pub text: String,
    /// Family name, resolved by photocraft-text (system fonts, its bundled
    /// fonts and craft-fonts when built with `CRAFT_FONTS_DIR`).
    pub family: String,
    pub size_px: f32,
    /// CSS weight `1..=1000`.
    pub weight: u16,
    pub italic: bool,
    pub color: Color,
    pub underline: bool,
    pub strike: bool,
}

/// Shaped, wrapped text laid out in `frame` (canvas pixels, before the
/// node's transform): callout text, step badge labels.
#[derive(Clone, Debug, PartialEq)]
pub struct TextSpec {
    pub runs: Vec<TextRunSpec>,
    pub frame: RectF32,
    pub align: TextAlign,
    pub valign: TextVAlign,
    /// Break lines at `frame.w`; otherwise one line per `\n`.
    pub wrap: bool,
}

/// Outline of a shape or annotation.
#[derive(Clone, Debug, PartialEq)]
pub struct StrokeSpec {
    pub color: Color,
    pub width: f32,
    pub join: LineJoin,
    /// Dash pattern in pixels (`on, off, on, off…`); empty = solid.
    pub dashes: Vec<f32>,
}

impl StrokeSpec {
    pub fn solid(color: Color, width: f32) -> Self {
        Self {
            color,
            width,
            join: LineJoin::Round,
            dashes: Vec::new(),
        }
    }
}

/// Outer drop shadow, cast by the merged silhouette (fill and stroke).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropShadowStyle {
    /// Shadow colour; its alpha is the shadow opacity.
    pub color: Color,
    pub offset: PointF32,
    /// Gaussian standard deviation in pixels (`0` = hard shadow).
    pub blur_sigma: f32,
    /// Grows the silhouette before blurring, in pixels.
    pub spread: f32,
}

impl Default for DropShadowStyle {
    fn default() -> Self {
        Self {
            color: Color::new(0.0, 0.0, 0.0, 0.45),
            offset: PointF32::new(3.0, 4.0),
            blur_sigma: 4.0,
            spread: 0.0,
        }
    }
}

/// Shadow cast inwards from a shape's edge (the magnifier's lens rim).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InnerShadowStyle {
    pub color: Color,
    pub offset: PointF32,
    pub blur_sigma: f32,
}

impl Default for InnerShadowStyle {
    fn default() -> Self {
        Self {
            color: Color::new(0.0, 0.0, 0.0, 0.5),
            offset: PointF32::new(0.0, 2.0),
            blur_sigma: 5.0,
        }
    }
}

/// Plain area shapes: blur/pixelate regions, spotlight targets, rectangles.
#[derive(Clone, Debug, PartialEq)]
pub enum ShapeGeometry {
    Rect(RectF32),
    RoundedRect {
        rect: RectF32,
        radius: f32,
    },
    Ellipse(RectF32),
    /// Closed polygon (non-zero fill).
    Polygon(Vec<PointF32>),
    /// Open freehand trace, drawn as the cubic Béziers PhotoCraft's
    /// Freeform Pen fits through it within `tolerance` px; stroke only,
    /// with round caps.
    Trace {
        points: Vec<PointF32>,
        tolerance: f32,
    },
}

/// A filled and/or stroked plain shape (the Shape tool, step badges).
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeSpec {
    pub geometry: ShapeGeometry,
    pub fill: Option<Paint>,
    pub stroke: Option<StrokeSpec>,
    pub shadow: Option<DropShadowStyle>,
    /// Drawn above the fill and stroke (a step badge's number).
    pub text: Option<TextSpec>,
}

/// The body of a callout.
#[derive(Clone, Debug, PartialEq)]
pub enum CalloutBody {
    RoundedRect {
        rect: RectF32,
        radius: f32,
    },
    Ellipse(RectF32),
    /// Thought cloud: overlapping puffs around an elliptical core.
    Cloud {
        rect: RectF32,
        puffs: u32,
    },
    /// Speech banner (ribbon) with V-notched ends, `notch` px deep.
    Banner {
        rect: RectF32,
        notch: f32,
    },
}

impl CalloutBody {
    pub fn rect(&self) -> RectF32 {
        match self {
            CalloutBody::RoundedRect { rect, .. }
            | CalloutBody::Cloud { rect, .. }
            | CalloutBody::Banner { rect, .. } => *rect,
            CalloutBody::Ellipse(rect) => *rect,
        }
    }
}

/// Where a tail grows from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TailAnchor {
    /// From the point of the body outline nearest to the tip.
    #[default]
    NearestEdge,
    /// From the body centre (the body hides the inner part).
    Center,
}

/// One draggable callout tail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CalloutTail {
    pub tail_tip: PointF32,
    /// Width of the tail where it meets the body, in pixels.
    pub base_width: f32,
    /// Bends both tail sides sideways (`-1..=1`; `0` = straight triangle).
    pub curve_bias: f32,
    pub anchor: TailAnchor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CalloutSpec {
    pub body: CalloutBody,
    pub tails: Vec<CalloutTail>,
    pub fill: Option<Paint>,
    pub stroke: Option<StrokeSpec>,
    pub shadow: Option<DropShadowStyle>,
    /// Drawn above the fill and stroke, inside the body.
    pub text: Option<TextSpec>,
}

/// Shaft geometry of an arrow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ArrowCurve {
    Straight,
    Quadratic {
        control: PointF32,
    },
    Cubic {
        control1: PointF32,
        control2: PointF32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrowHeadShape {
    /// Filled triangle.
    Triangle,
    /// Filled triangle with a swept-back notch.
    Notched,
    /// Open chevron drawn with the shaft width.
    Open,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrowHeadStyle {
    pub shape: ArrowHeadShape,
    /// Tip-to-base length in pixels.
    pub length: f32,
    /// Full width at the base in pixels.
    pub width: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArrowSpec {
    pub start: PointF32,
    pub end: PointF32,
    pub curve: ArrowCurve,
    pub color: Color,
    /// Shaft width in pixels.
    pub width: f32,
    /// Head at `end`.
    pub head: Option<ArrowHeadStyle>,
    /// Head at `start` (double-headed arrow).
    pub tail_head: Option<ArrowHeadStyle>,
    /// Dash pattern of the shaft in pixels; empty = solid.
    pub dashes: Vec<f32>,
    pub shadow: Option<DropShadowStyle>,
}

/// Lens outline of a magnifier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShapeBounds {
    Circle { center: PointF32, radius: f32 },
    RoundedRect { rect: RectF32, radius: f32 },
}

impl ShapeBounds {
    pub fn center(&self) -> PointF32 {
        match self {
            ShapeBounds::Circle { center, .. } => *center,
            ShapeBounds::RoundedRect { rect, .. } => rect.center(),
        }
    }
}

/// Resampling kernel of the magnifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Resample {
    /// Bicubic Catmull-Rom (4×4 taps).
    #[default]
    CatmullRom,
    /// Lanczos, a = 3 (6×6 taps).
    Lanczos3,
    Nearest,
}

/// Magnifier loupe: shows the backdrop around `source_center`, zoomed, in
/// the lens.
#[derive(Clone, Debug, PartialEq)]
pub struct MagnifierSpec {
    pub source_center: PointF32,
    pub lens_bounds: ShapeBounds,
    /// Clamped to `1.5..=8.0`.
    pub zoom_factor: f32,
    pub resample: Resample,
    /// Nearest-neighbour with a 1 px grid between source pixels once
    /// `zoom_factor >= 4`.
    pub pixel_grid_mode: bool,
    pub grid_color: Color,
    pub border_stroke: Option<StrokeSpec>,
    pub inner_shadow: Option<InnerShadowStyle>,
    pub drop_shadow: Option<DropShadowStyle>,
    /// Line from `source_center` to the lens rim, plus a ring around the
    /// magnified source area, drawn with this stroke.
    pub connector: Option<StrokeSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BackdropFilterKind {
    /// Separable Gaussian blur (`sigma` in pixels, clamped to `0..=128`).
    GaussianBlur { sigma: f32 },
    /// Mosaic of `block_size_px` squares aligned to the shape's top-left
    /// pixel (`1..=512`).
    Pixelate { block_size_px: u32 },
}

/// Non-destructive blur or pixelation of what lies beneath `shape`.
#[derive(Clone, Debug, PartialEq)]
pub struct BackdropFilterSpec {
    pub shape: ShapeGeometry,
    pub filter: BackdropFilterKind,
    /// Width of the soft edge, centred on the outline (`0` = hard edge).
    pub feather_px: f32,
}

/// Dims (and optionally blurs) everything outside the targets.
#[derive(Clone, Debug, PartialEq)]
pub struct SpotlightSpec {
    pub targets: Vec<ShapeGeometry>,
    /// `0..=1`: how much of `dim_color` is laid over the outside.
    pub dim_opacity: f32,
    pub dim_color: Color,
    /// Width of the soft edge, centred on the target outlines, in pixels.
    pub feather_px: f32,
    /// Blur of the outside (`0` = none).
    pub blur_sigma: f32,
}

/// Which strip a Cut Out removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutAxis {
    /// Removes rows `t0..t1`; the image gets shorter.
    Horizontal,
    /// Removes columns `t0..t1`; the image gets narrower.
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeamStyle {
    Straight,
    /// Torn paper: both edges are displaced by deterministic 1D fBm
    /// gradient noise.
    TornEdge {
        /// Maximum displacement into each piece, in pixels.
        amplitude_px: f32,
        /// Gain per octave (`0..=1`; higher = more jagged).
        roughness: f32,
        /// Feature size of the first octave, in pixels.
        wavelength_px: f32,
        seed: u64,
    },
}

/// Shadow the upper/left piece casts across the seam: `samples` copies of
/// its torn edge stepped along `offset`, averaged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeamShadow {
    pub color: Color,
    pub offset: PointF32,
    pub samples: u32,
}

/// Snagit's Cut Out: removes a strip and joins the rest along a seam.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CutOutSeamSpec {
    pub axis: CutAxis,
    pub t0: u32,
    pub t1: u32,
    /// Space left between the pieces (filled with `gap_fill`).
    pub gap_px: u32,
    pub gap_fill: Color,
    pub seam: SeamStyle,
    pub shadow: Option<SeamShadow>,
}

/// Pixels (the capture itself, pasted images), placed at `origin`.
#[derive(Clone, Debug)]
pub struct RasterSpec {
    /// Shared copy-on-write tiles: the compositors compare tile pointers
    /// to find what changed between revisions.
    pub surface: Arc<Surface>,
    pub origin: [i32; 2],
    pub opacity: f32,
}

impl PartialEq for RasterSpec {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.surface, &other.surface)
            && self.origin == other.origin
            && self.opacity == other.opacity
    }
}

/// Presents the finished canvas: grows it by `margin` on every side,
/// paints `backdrop` there, and draws the canvas over it clipped to a
/// rounded rectangle, with a drop shadow and an optional border (centred
/// on the clip edge). Later nodes are in frame coordinates, so it is
/// normally last.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowFrameSpec {
    pub margin: u32,
    pub corner_radius: f32,
    /// In frame coordinates (canvas `(0, 0)` is frame `(margin, margin)`).
    pub backdrop: Paint,
    pub shadow: Option<DropShadowStyle>,
    pub border: Option<StrokeSpec>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenderNode {
    Raster(RasterSpec),
    Shape(ShapeSpec),
    Callout(CalloutSpec),
    Arrow(ArrowSpec),
    Magnifier(MagnifierSpec),
    BackdropFilter(BackdropFilterSpec),
    Spotlight(SpotlightSpec),
    /// Restructures the canvas: later nodes are in post-cut coordinates.
    CutOut(CutOutSeamSpec),
    /// Restructures the canvas: later nodes are in frame coordinates.
    ShadowFrame(ShadowFrameSpec),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneNode {
    pub id: NodeId,
    pub revision: RevisionId,
    pub node: RenderNode,
    /// Node space → canvas space (rotation, the scale of a resized
    /// raster). Applies to the node's geometry, text and pixels; a
    /// magnifier's lens and a spotlight's targets turn but what they show
    /// does not. Ignored by cut outs and shadow frames. Part of the cache
    /// key together with the revision.
    pub transform: Option<Affine>,
}

impl SceneNode {
    pub fn new(id: u64, revision: u64, node: RenderNode) -> Self {
        Self {
            id: NodeId(id),
            revision: RevisionId(revision),
            node,
            transform: None,
        }
    }

    /// The same node drawn through `t` (`None` for the identity).
    pub fn with_transform(mut self, t: Affine) -> Self {
        self.transform = (t != Affine::IDENTITY).then_some(t);
        self
    }
}

/// What to draw, bottom to top.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderScene {
    pub width: u32,
    pub height: u32,
    pub background: Color,
    pub nodes: Vec<SceneNode>,
}

/// Areas the caller knows changed without a revision bump (in canvas
/// coordinates before any Cut Out), e.g. pixels painted into a shared
/// surface in place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirtyRegion {
    pub rects: Vec<Rect>,
    /// Everything: drop all caches and redraw.
    pub all: bool,
}

impl DirtyRegion {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn all() -> Self {
        Self {
            rects: Vec::new(),
            all: true,
        }
    }

    pub fn add(&mut self, r: Rect) {
        if !r.is_empty() {
            self.rects.push(r);
        }
    }
}
