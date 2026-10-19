//! `manifest.json`: the document schema.
//!
//! Coordinate spaces:
//! - **Base space**: pixels of the captured screenshot ([`RasterLayer`]),
//!   origin top-left. [`CutOutSeam`]s are given in base space, applied in
//!   document order, each in the space left by the cuts before it.
//! - **Canvas space**: the visible document after cut outs, with the base
//!   placed at [`Canvas::base_origin`] (negative when the canvas was
//!   dragged outward to add padding). Every other annotation lives here.
//!
//! Annotations are drawn bottom to top in [`Manifest::annotations`] order;
//! backdrop nodes (blur, magnifier, spotlight) sample everything below
//! them. A node's geometry is stored unrotated; [`AnnotationNode::rotation_rad`]
//! turns it about [`Annotation::pivot`] when drawn
//! ([`AnnotationNode::transform`], a `photocraft-geom` affine).
//!
//! **Frame space**: with a [`CanvasBackdrop::RoundedShadowFrame`] the
//! exported image is the canvas plus `margin_px` on every side; frame
//! space is that image, origin top-left, so canvas `(0, 0)` is frame
//! `(margin_px, margin_px)`.

use photocraft_geom::{Affine, Point, Rect, Size, TILE_SIZE, TileCoord};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::{FormatError, Result};

/// Bumped only on breaking changes; additive changes rely on `serde(default)`.
pub const FORMAT_VERSION: u32 = 1;
pub const FILE_EXTENSION: &str = "capturecraft";
pub const MANIFEST_PATH: &str = "manifest.json";
pub const CURSOR_SPRITE_PATH: &str = "cursor/sprite.png";
pub const THUMBNAIL_PATH: &str = "thumbnail.png";

/// Longest side of a canvas or raster (tall scrolling captures included).
pub const MAX_SIDE: u32 = 1 << 18;
/// Largest canvas or raster area.
pub const MAX_PIXELS: u64 = 1 << 30;
pub const MAX_NODES: usize = 100_000;
pub const MAX_TAILS: usize = 16;
pub const MAX_POLYGON_POINTS: usize = 4096;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_TAGS: usize = 256;
pub const MIN_ZOOM: f32 = 1.5;
pub const MAX_ZOOM: f32 = 8.0;
pub const MAX_BLUR_SIGMA: f32 = 256.0;
pub const MAX_BLOCK_PX: u32 = 1024;
/// Upper bound for widths, radii, offsets and other lengths in pixels.
pub const MAX_LENGTH_PX: f32 = 16_384.0;

/// Archive path of a base-layer tile.
pub fn tile_path(layer: &str, coord: TileCoord) -> String {
    format!("tiles/{layer}/{}_{}.png", coord.tx, coord.ty)
}

/// Archive path of a [`SmartObject`]'s pixels.
pub fn object_path(id: NodeId) -> String {
    format!("objects/{}.png", id.0)
}

// ---------------------------------------------------------------------------
// Document

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub format_version: u32,
    /// `capturecraft 0.18.0`, for bug reports.
    pub generator: String,
    /// Stable document id (library key, recent-tray key).
    pub id: String,
    pub title: String,
    pub canvas: Canvas,
    pub base: RasterLayer,
    /// The OS cursor at capture time, detached from the pixels.
    pub cursor: Option<CursorLayer>,
    pub annotations: Vec<AnnotationNode>,
    pub capture: CaptureMetadata,
    pub library: LibraryMeta,
    /// Next free [`NodeId`]; see [`Manifest::alloc_node_id`].
    pub next_node_id: u64,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            generator: String::new(),
            id: String::new(),
            title: String::new(),
            canvas: Canvas::default(),
            base: RasterLayer::default(),
            cursor: None,
            annotations: Vec::new(),
            capture: CaptureMetadata::default(),
            library: LibraryMeta::default(),
            next_node_id: 1,
        }
    }
}

impl Manifest {
    /// A document for a fresh capture of `size` pixels, canvas = base.
    pub fn new(size: Size) -> Self {
        Self {
            canvas: Canvas {
                size,
                ..Canvas::default()
            },
            base: RasterLayer {
                size,
                ..RasterLayer::default()
            },
            ..Self::default()
        }
    }

    /// Parses and validates `manifest.json`.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let m: Manifest = serde_json::from_slice(bytes)?;
        m.validate()?;
        Ok(m)
    }

    pub fn to_json(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// A fresh id, never reused within the document.
    pub fn alloc_node_id(&mut self) -> NodeId {
        let used = self.annotations.iter().map(|n| n.id.0).max().unwrap_or(0);
        let id = self.next_node_id.max(used.saturating_add(1)).max(1);
        self.next_node_id = id.saturating_add(1);
        NodeId(id)
    }

    pub fn node(&self, id: NodeId) -> Option<&AnnotationNode> {
        self.annotations.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut AnnotationNode> {
        self.annotations.iter_mut().find(|n| n.id == id)
    }

    /// Checks every invariant the renderer and engine rely on.
    pub fn validate(&self) -> Result<()> {
        if self.format_version > FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion {
                found: self.format_version,
                supported: FORMAT_VERSION,
            });
        }
        let v = &mut V::default();
        check_size(v, "canvas.size", self.canvas.size);
        v.at("canvas.backdrop", |v| self.canvas.backdrop.validate(v));
        check_size(
            v,
            "canvas.backdrop.margin_px",
            self.canvas.backdrop.frame_size(self.canvas.size),
        );
        check_size(v, "base.size", self.base.size);
        self.base.validate(v);
        if let Some(c) = &self.cursor {
            c.validate(v);
        }
        if self.annotations.len() > MAX_NODES {
            v.err("annotations", format!("more than {MAX_NODES} nodes"));
        }
        let mut ids: Vec<u64> = self.annotations.iter().map(|n| n.id.0).collect();
        ids.sort_unstable();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            v.err("annotations", "duplicate node id");
        }
        for (i, n) in self.annotations.iter().enumerate() {
            v.at(&format!("annotations[{i}]"), |v| n.validate(v));
        }
        if self.library.tags.len() > MAX_TAGS {
            v.err("library.tags", format!("more than {MAX_TAGS} tags"));
        }
        v.finish()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Canvas {
    pub size: Size,
    /// Where the (cut) base raster's top-left sits in canvas space.
    pub base_origin: Offset,
    /// Shown where the canvas extends past the base raster.
    pub background: Background,
    /// Presentation around the finished canvas (the export grows by its
    /// margin).
    pub backdrop: CanvasBackdrop,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            size: Size::new(0, 0),
            base_origin: Offset::default(),
            background: Background::Transparent,
            backdrop: CanvasBackdrop::None,
        }
    }
}

/// Largest backdrop margin, in pixels.
pub const MAX_BACKDROP_MARGIN_PX: f32 = 2048.0;

/// What surrounds the canvas in the exported image.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanvasBackdrop {
    /// The canvas is the image.
    #[default]
    None,
    /// The canvas with rounded corners, a drop shadow and an optional
    /// border, centred on `backdrop_fill` with `margin_px` on every side.
    /// Gradient points of `backdrop_fill` are in frame space.
    RoundedShadowFrame {
        margin_px: f32,
        corner_radius_px: f32,
        backdrop_fill: Fill,
        shadow: Option<DropShadow>,
        border: Option<Stroke>,
    },
}

impl CanvasBackdrop {
    /// Snagit's default frame: 48 px of soft grey-blue gradient, 12 px
    /// corners, a soft shadow.
    pub fn rounded_shadow_frame() -> Self {
        CanvasBackdrop::RoundedShadowFrame {
            margin_px: 48.0,
            corner_radius_px: 12.0,
            backdrop_fill: Fill::Solid {
                color: Color::rgb(0xE2, 0xE8, 0xF0),
            },
            shadow: Some(DropShadow {
                color: Color::rgba(0x0F, 0x17, 0x2A, 0x59),
                offset_x: 0.0,
                offset_y: 8.0,
                blur: 24.0,
                spread: 0.0,
            }),
            border: None,
        }
    }

    /// Margin on each side, whole pixels (0 without a frame).
    pub fn margin(&self) -> u32 {
        match self {
            CanvasBackdrop::None => 0,
            CanvasBackdrop::RoundedShadowFrame { margin_px, .. } => {
                margin_px.clamp(0.0, MAX_BACKDROP_MARGIN_PX).round() as u32
            }
        }
    }

    /// Size of the exported image for a canvas of `canvas`.
    pub fn frame_size(&self, canvas: Size) -> Size {
        let m = self.margin() * 2;
        Size::new(
            canvas.width.saturating_add(m),
            canvas.height.saturating_add(m),
        )
    }

    fn validate(&self, v: &mut V) {
        if let CanvasBackdrop::RoundedShadowFrame {
            margin_px,
            corner_radius_px,
            backdrop_fill,
            shadow,
            border,
        } = self
        {
            check_range(v, "margin_px", *margin_px, 0.0, MAX_BACKDROP_MARGIN_PX);
            check_len(v, "corner_radius_px", *corner_radius_px);
            backdrop_fill.validate(v, "backdrop_fill");
            if let Some(s) = shadow {
                s.validate(v, "shadow");
            }
            if let Some(s) = border {
                s.validate(v, "border");
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Offset {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Background {
    #[default]
    Transparent,
    Solid {
        color: Color,
    },
}

// ---------------------------------------------------------------------------
// Rasters

/// The screenshot: a sparse grid of `photocraft-raster` tiles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RasterLayer {
    pub size: Size,
    /// Always [`TILE_SIZE`] today; stored so a reader can refuse others.
    pub tile_size: u32,
    pub pixel_format: PixelFormat,
    pub color_profile: ColorProfile,
    /// Physical pixels per logical pixel of the captured monitor (HiDPI).
    pub scale_factor: f64,
    /// Tiles not listed are fully transparent.
    pub tiles: Vec<TileEntry>,
}

impl Default for RasterLayer {
    fn default() -> Self {
        Self {
            size: Size::new(0, 0),
            tile_size: TILE_SIZE as u32,
            pixel_format: PixelFormat::Rgba8,
            color_profile: ColorProfile::Srgb,
            scale_factor: 1.0,
            tiles: Vec::new(),
        }
    }
}

impl RasterLayer {
    /// The tile grid covering `size`, in tiles.
    pub fn grid(&self) -> (i32, i32) {
        let t = TILE_SIZE as u32;
        (
            self.size.width.div_ceil(t) as i32,
            self.size.height.div_ceil(t) as i32,
        )
    }

    fn validate(&self, v: &mut V) {
        if self.tile_size != TILE_SIZE as u32 {
            v.err("base.tile_size", format!("must be {TILE_SIZE}"));
        }
        if !(self.scale_factor.is_finite() && (0.25..=16.0).contains(&self.scale_factor)) {
            v.err("base.scale_factor", "must be within 0.25..=16");
        }
        if let ColorProfile::Icc { path } = &self.color_profile {
            check_archive_path(v, "base.color_profile.path", path);
        }
        let (gw, gh) = self.grid();
        let mut seen: Vec<TileCoord> = Vec::with_capacity(self.tiles.len());
        for (i, t) in self.tiles.iter().enumerate() {
            let p = format!("base.tiles[{i}]");
            let c = t.coord;
            if c.tx < 0 || c.ty < 0 || c.tx >= gw || c.ty >= gh {
                v.err(
                    &p,
                    format!("tile ({}, {}) outside the {gw}×{gh} grid", c.tx, c.ty),
                );
            }
            if let TileData::File { path, .. } = &t.data {
                check_archive_path(v, &p, path);
            }
            seen.push(c);
        }
        seen.sort_unstable();
        if seen.windows(2).any(|w| w[0] == w[1]) {
            v.err("base.tiles", "duplicate tile coordinate");
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    /// 8-bit straight-alpha RGBA, the capture format on every platform.
    #[default]
    Rgba8,
    /// Half-float RGBA for HDR monitor captures.
    Rgba16F,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ColorProfile {
    #[default]
    Srgb,
    DisplayP3,
    /// The monitor's ICC profile, stored in the archive (`photocraft-cms`).
    Icc {
        path: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TileEntry {
    pub coord: TileCoord,
    #[serde(flatten)]
    pub data: TileData,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "storage", rename_all = "snake_case")]
pub enum TileData {
    /// A PNG in the archive, normally [`tile_path`].
    File {
        path: String,
        /// Hex SHA-256 of the PNG; lets the library dedupe across documents.
        #[serde(default)]
        sha256: Option<String>,
    },
    /// Every pixel the same (flat UI backgrounds compress to nothing).
    Solid { color: Color },
}

/// The OS mouse cursor as its own movable sprite layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorLayer {
    pub visible: bool,
    /// Hotspot position in canvas space (moves with the user's drag).
    pub position: Point,
    /// Hotspot inside the sprite, in sprite pixels.
    pub hotspot: Point,
    pub sprite_path: String,
    pub sprite_size: Size,
    pub kind: CursorKind,
    pub scale: f32,
    /// Optional click/emphasis halo drawn under the cursor.
    pub halo: Option<CursorHalo>,
}

impl Default for CursorLayer {
    fn default() -> Self {
        Self {
            visible: true,
            position: Point::new(0.0, 0.0),
            hotspot: Point::new(0.0, 0.0),
            sprite_path: CURSOR_SPRITE_PATH.into(),
            sprite_size: Size::new(0, 0),
            kind: CursorKind::Arrow,
            scale: 1.0,
            halo: None,
        }
    }
}

impl CursorLayer {
    fn validate(&self, v: &mut V) {
        check_point(v, "cursor.position", self.position);
        check_point(v, "cursor.hotspot", self.hotspot);
        check_archive_path(v, "cursor.sprite_path", &self.sprite_path);
        if self.sprite_size.width > 1024 || self.sprite_size.height > 1024 {
            v.err("cursor.sprite_size", "cursor sprites are at most 1024 px");
        }
        check_range(v, "cursor.scale", self.scale, 0.1, 16.0);
        if let Some(h) = &self.halo {
            check_len(v, "cursor.halo.radius", h.radius);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorKind {
    #[default]
    Arrow,
    IBeam,
    Hand,
    Wait,
    Crosshair,
    ResizeNs,
    ResizeEw,
    ResizeNwse,
    ResizeNesw,
    Move,
    NotAllowed,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CursorHalo {
    pub color: Color,
    pub radius: f32,
}

// ---------------------------------------------------------------------------
// Capture metadata and library

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureMetadata {
    /// RFC 3339 local time with offset, e.g. `2026-10-19T14:03:11+02:00`.
    pub captured_at: String,
    pub mode: CaptureMode,
    /// Captured area in virtual-desktop physical pixels.
    pub region: Rect,
    /// The window under the capture, from the OS window/accessibility APIs.
    pub source: Option<SourceInfo>,
    pub monitors: Vec<MonitorInfo>,
    /// `windows 11 24H2`, `macos 15.1`, `linux wayland gnome`.
    pub os: String,
    pub delay_ms: u32,
    /// Present for scrolling/panoramic captures.
    pub scroll: Option<ScrollInfo>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    #[default]
    Region,
    Window,
    Fullscreen,
    Scrolling,
    Menu,
    /// Imported from a file or the clipboard.
    Import,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceInfo {
    /// Display name (`Firefox`).
    pub app_name: String,
    /// Executable (`firefox.exe`), for grouping in the library.
    pub process_name: String,
    pub window_title: String,
    /// Page URL when the browser exposes it (accessibility tree).
    pub url: Option<String>,
    pub window_rect: Rect,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorInfo {
    pub name: String,
    /// Virtual-desktop physical pixels.
    pub rect: Rect,
    pub scale_factor: f64,
    pub primary: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrollInfo {
    /// Scroll direction: `Vertical` for a page scrolled down (a tall result).
    pub axis: Axis,
    /// Frames stitched into the result.
    pub frames: u32,
    /// `true` when CaptureCraft scrolled the window itself.
    pub automatic: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryMeta {
    pub tags: Vec<String>,
    pub favorite: bool,
    pub notes: String,
}

// ---------------------------------------------------------------------------
// Annotation tree

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct NodeId(pub u64);

/// One live, editable object above the base raster.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnnotationNode {
    pub id: NodeId,
    /// Bumped on every edit; the compositor caches by `(id, revision)`.
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub locked: bool,
    #[serde(default = "one")]
    pub opacity: f32,
    /// Clockwise rotation (y down) about [`Annotation::pivot`], radians,
    /// within `-π..=π`. The stored geometry is unrotated.
    #[serde(default)]
    pub rotation_rad: f32,
    /// Command that generated the node (`smart.simplify`), so re-running it
    /// replaces its own output.
    #[serde(default)]
    pub generated_by: Option<String>,
    /// Nodes sharing a group id select, move and arrange together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u64>,
    pub kind: NodeKind,
}

fn yes() -> bool {
    true
}

/// Rotation by `rad` (clockwise on screen, y down) about `c`.
pub fn rotation_about(c: Point, rad: f32) -> Affine {
    // `a.then(b)` applies `a` first.
    Affine::translate(-c.x, -c.y)
        .then(&Affine::rotate(f64::from(rad)))
        .then(&Affine::translate(c.x, c.y))
}

/// `rad` wrapped into `-π..=π`.
pub fn normalize_angle(rad: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    if !rad.is_finite() {
        return 0.0;
    }
    let r = (rad + PI).rem_euclid(TAU) - PI;
    if r <= -PI { PI } else { r }
}
fn one() -> f32 {
    1.0
}

impl AnnotationNode {
    pub fn new(id: NodeId, annotation: Annotation) -> Self {
        Self {
            id,
            revision: 0,
            name: String::new(),
            visible: true,
            locked: false,
            opacity: 1.0,
            rotation_rad: 0.0,
            generated_by: None,
            group: None,
            kind: NodeKind::Known(annotation),
        }
    }

    pub fn annotation(&self) -> Option<&Annotation> {
        match &self.kind {
            NodeKind::Known(a) => Some(a),
            NodeKind::Unknown(_) => None,
        }
    }

    pub fn annotation_mut(&mut self) -> Option<&mut Annotation> {
        match &mut self.kind {
            NodeKind::Known(a) => Some(a),
            NodeKind::Unknown(_) => None,
        }
    }

    /// Marks an edit for the compositor's cache.
    pub fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Geometry space → canvas space: the rotation about the pivot, or the
    /// identity for unrotated nodes, unknown nodes and cut outs (which are
    /// never rotated).
    pub fn transform(&self) -> Affine {
        match self.annotation() {
            Some(a) if self.rotation_rad != 0.0 && self.rotation_rad.is_finite() => a
                .pivot()
                .map_or(Affine::IDENTITY, |c| rotation_about(c, self.rotation_rad)),
            _ => Affine::IDENTITY,
        }
    }

    fn validate(&self, v: &mut V) {
        check_range(v, "opacity", self.opacity, 0.0, 1.0);
        check_range(
            v,
            "rotation_rad",
            self.rotation_rad,
            -std::f32::consts::PI,
            std::f32::consts::PI,
        );
        if let NodeKind::Known(a) = &self.kind {
            v.at("kind", |v| a.validate(v));
        }
    }
}

/// A node body: a type this build knows, or one from a newer build that is
/// kept verbatim (and drawn as nothing) so saving does not lose it.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    Known(Annotation),
    Unknown(Value),
}

impl Serialize for NodeKind {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            NodeKind::Known(a) => a.serialize(s),
            NodeKind::Unknown(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for NodeKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let ty = v.get("type").and_then(Value::as_str).unwrap_or_default();
        if Annotation::TYPES.contains(&ty) {
            // A known type that does not parse is an error, not "unknown".
            serde_json::from_value(v)
                .map(NodeKind::Known)
                .map_err(serde::de::Error::custom)
        } else if v.is_object() {
            Ok(NodeKind::Unknown(v))
        } else {
            Err(serde::de::Error::custom(
                "annotation kind must be an object",
            ))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Annotation {
    Callout(Callout),
    StepBadge(StepBadge),
    CurvedArrow(CurvedArrow),
    BlurRegion(BlurRegion),
    MagnifierLens(MagnifierLens),
    Spotlight(Spotlight),
    CutOutSeam(CutOutSeam),
    Shape(ShapeNode),
    SmartObject(SmartObject),
    Pen(PenStroke),
}

impl Annotation {
    /// Every `type` tag above; keep in sync (a test checks it).
    pub const TYPES: &'static [&'static str] = &[
        "callout",
        "step_badge",
        "curved_arrow",
        "blur_region",
        "magnifier_lens",
        "spotlight",
        "cut_out_seam",
        "shape",
        "smart_object",
        "pen",
    ];

    pub fn type_name(&self) -> &'static str {
        match self {
            Annotation::Callout(_) => "callout",
            Annotation::StepBadge(_) => "step_badge",
            Annotation::CurvedArrow(_) => "curved_arrow",
            Annotation::BlurRegion(_) => "blur_region",
            Annotation::MagnifierLens(_) => "magnifier_lens",
            Annotation::Spotlight(_) => "spotlight",
            Annotation::CutOutSeam(_) => "cut_out_seam",
            Annotation::Shape(_) => "shape",
            Annotation::SmartObject(_) => "smart_object",
            Annotation::Pen(_) => "pen",
        }
    }

    /// Moves the node in canvas space. Cut outs live in base space and stay.
    pub fn translate(&mut self, dx: f64, dy: f64) {
        let mv = |p: &mut Point| {
            p.x += dx;
            p.y += dy;
        };
        match self {
            Annotation::Callout(c) => {
                c.bounds.translate(dx, dy);
                for t in &mut c.tails {
                    mv(&mut t.tip);
                }
            }
            Annotation::StepBadge(b) => mv(&mut b.center),
            Annotation::CurvedArrow(a) => {
                mv(&mut a.start);
                mv(&mut a.end);
                match &mut a.curve {
                    ArrowCurve::Straight => {}
                    ArrowCurve::Quadratic { control } => mv(control),
                    ArrowCurve::Cubic { c0, c1 } => {
                        mv(c0);
                        mv(c1);
                    }
                }
            }
            Annotation::BlurRegion(b) => b.shape.translate(dx, dy),
            Annotation::MagnifierLens(m) => {
                mv(&mut m.source_center);
                m.lens.translate(dx, dy);
            }
            Annotation::Spotlight(s) => {
                for t in &mut s.targets {
                    t.translate(dx, dy);
                }
            }
            Annotation::CutOutSeam(_) => {}
            Annotation::Shape(s) => s.shape.translate(dx, dy),
            Annotation::SmartObject(o) => o.rect.translate(dx, dy),
            Annotation::Pen(p) => p.points.iter_mut().for_each(mv),
        }
    }

    /// Canvas-space bounds of the geometry (strokes and shadows excluded);
    /// `None` for cut outs, which span the whole canvas.
    pub fn bounds(&self) -> Option<RectF> {
        match self {
            Annotation::Callout(c) => Some(
                c.tails
                    .iter()
                    .fold(c.bounds, |r, t| r.union(RectF::point(t.tip))),
            ),
            Annotation::StepBadge(b) => {
                let r = f64::from(b.radius);
                Some(RectF::new(b.center.x - r, b.center.y - r, 2.0 * r, 2.0 * r))
            }
            Annotation::CurvedArrow(a) => {
                let r = RectF::point(a.start).union(RectF::point(a.end));
                Some(match &a.curve {
                    ArrowCurve::Straight => r,
                    ArrowCurve::Quadratic { control } => r.union(RectF::point(*control)),
                    ArrowCurve::Cubic { c0, c1 } => {
                        r.union(RectF::point(*c0)).union(RectF::point(*c1))
                    }
                })
            }
            Annotation::BlurRegion(b) => Some(b.shape.bounds()),
            Annotation::MagnifierLens(m) => Some(m.lens.bounds()),
            Annotation::Spotlight(s) => s
                .targets
                .iter()
                .map(RegionShape::bounds)
                .reduce(RectF::union),
            Annotation::CutOutSeam(_) => None,
            Annotation::Shape(s) => Some(s.shape.bounds()),
            Annotation::SmartObject(o) => Some(o.rect),
            Annotation::Pen(p) => p
                .points
                .iter()
                .map(|q| RectF::point(*q))
                .reduce(RectF::union),
        }
    }

    /// The point rotation turns about: the callout body's centre (so
    /// dragging a tail does not move the body), the centre of
    /// [`Annotation::bounds`] otherwise; `None` for cut outs.
    pub fn pivot(&self) -> Option<Point> {
        match self {
            Annotation::Callout(c) => Some(c.bounds.center()),
            _ => self.bounds().map(|b| b.center()),
        }
    }

    /// Scales and moves the geometry so [`Annotation::bounds`] `from` maps
    /// onto `to` (corner to corner; a negative `to` size mirrors). Radii
    /// scale with the geometric mean of the two factors; stroke widths,
    /// font sizes and effect strengths stay. Cut outs do not move.
    pub fn resize(&mut self, from: RectF, to: RectF) {
        let sx = if from.w.abs() > 1e-9 {
            to.w / from.w
        } else {
            1.0
        };
        let sy = if from.h.abs() > 1e-9 {
            to.h / from.h
        } else {
            1.0
        };
        let map = |p: Point| Point::new(to.x + (p.x - from.x) * sx, to.y + (p.y - from.y) * sy);
        let k = (sx * sy).abs().sqrt();
        self.map_geometry(&map, k);
    }

    /// Applies `f` to every canvas-space point (rectangles map corner to
    /// corner) and scales radii by `k`.
    pub fn map_geometry(&mut self, f: &dyn Fn(Point) -> Point, k: f64) {
        let len = |r: &mut f32| *r = (f64::from(*r) * k) as f32;
        match self {
            Annotation::Callout(c) => {
                c.bounds = c.bounds.map(f);
                for t in &mut c.tails {
                    t.tip = f(t.tip);
                }
                match &mut c.body {
                    CalloutBody::RoundedRect { radius } => len(radius),
                    CalloutBody::Banner { fold } => len(fold),
                    CalloutBody::Rect | CalloutBody::Ellipse | CalloutBody::Cloud { .. } => {}
                }
                c.fill.map(f);
            }
            Annotation::StepBadge(b) => {
                b.center = f(b.center);
                len(&mut b.radius);
            }
            Annotation::CurvedArrow(a) => {
                a.start = f(a.start);
                a.end = f(a.end);
                match &mut a.curve {
                    ArrowCurve::Straight => {}
                    ArrowCurve::Quadratic { control } => *control = f(*control),
                    ArrowCurve::Cubic { c0, c1 } => {
                        *c0 = f(*c0);
                        *c1 = f(*c1);
                    }
                }
            }
            Annotation::BlurRegion(b) => b.shape.map(f, k),
            Annotation::MagnifierLens(m) => {
                m.source_center = f(m.source_center);
                match &mut m.lens {
                    LensShape::Circle { center, radius } => {
                        *center = f(*center);
                        len(radius);
                    }
                    LensShape::RoundedRect { rect, radius } => {
                        *rect = rect.map(f);
                        len(radius);
                    }
                }
            }
            Annotation::Spotlight(s) => {
                for t in &mut s.targets {
                    t.map(f, k);
                }
            }
            Annotation::CutOutSeam(_) => {}
            Annotation::Shape(s) => {
                s.shape.map(f, k);
                s.fill.map(f);
            }
            Annotation::SmartObject(o) => o.rect = o.rect.map(f),
            Annotation::Pen(p) => p.points.iter_mut().for_each(|q| *q = f(*q)),
        }
    }

    fn validate(&self, v: &mut V) {
        match self {
            Annotation::Callout(c) => c.validate(v),
            Annotation::StepBadge(b) => b.validate(v),
            Annotation::CurvedArrow(a) => a.validate(v),
            Annotation::BlurRegion(b) => b.validate(v),
            Annotation::MagnifierLens(m) => m.validate(v),
            Annotation::Spotlight(s) => s.validate(v),
            Annotation::CutOutSeam(c) => c.validate(v),
            Annotation::Shape(s) => s.validate(v),
            Annotation::SmartObject(o) => o.validate(v),
            Annotation::Pen(p) => p.validate(v),
        }
    }
}

// --- Callout ---------------------------------------------------------------

/// A speech bubble or banner with rich text and any number of tails. The
/// body and tails are boolean-unioned (`photocraft-vector`) before the
/// stroke and shadow are built, so the outline runs around the merged
/// perimeter without seams.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Callout {
    pub bounds: RectF,
    pub body: CalloutBody,
    pub tails: Vec<CalloutTail>,
    pub fill: Fill,
    pub stroke: Option<Stroke>,
    pub shadow: Option<DropShadow>,
    pub text: TextBlock,
    /// Rounds the concave corner where a tail meets the body.
    pub fillet_px: f32,
}

impl Default for Callout {
    fn default() -> Self {
        Self {
            bounds: RectF::new(0.0, 0.0, 200.0, 100.0),
            body: CalloutBody::RoundedRect { radius: 12.0 },
            tails: Vec::new(),
            fill: Fill::Solid {
                color: Color::WHITE,
            },
            stroke: Some(Stroke::default()),
            shadow: Some(DropShadow::default()),
            text: TextBlock::default(),
            fillet_px: 0.0,
        }
    }
}

impl Callout {
    fn validate(&self, v: &mut V) {
        self.bounds.validate(v, "bounds");
        match self.body {
            CalloutBody::RoundedRect { radius } => check_len(v, "body.radius", radius),
            CalloutBody::Cloud { lobes } => {
                if !(3..=64).contains(&lobes) {
                    v.err("body.lobes", "must be within 3..=64");
                }
            }
            CalloutBody::Banner { fold } => check_len(v, "body.fold", fold),
            CalloutBody::Rect | CalloutBody::Ellipse => {}
        }
        if self.tails.len() > MAX_TAILS {
            v.err("tails", format!("more than {MAX_TAILS} tails"));
        }
        for (i, t) in self.tails.iter().enumerate() {
            v.at(&format!("tails[{i}]"), |v| t.validate(v));
        }
        self.fill.validate(v, "fill");
        if let Some(s) = &self.stroke {
            s.validate(v, "stroke");
        }
        if let Some(s) = &self.shadow {
            s.validate(v, "shadow");
        }
        self.text.validate(v, "text");
        check_len(v, "fillet_px", self.fillet_px);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CalloutBody {
    Rect,
    RoundedRect {
        radius: f32,
    },
    Ellipse,
    /// Thought cloud with `lobes` bumps around the ellipse.
    Cloud {
        lobes: u32,
    },
    /// Ribbon banner with folded ends of `fold` pixels.
    Banner {
        fold: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalloutTail {
    /// The draggable point, canvas space.
    pub tip: Point,
    /// Width where the tail meets the body.
    pub base_width: f32,
    /// Bends the tail: 0 straight, ±1 strongly curved to either side.
    pub curve_bias: f32,
    pub anchor: TailAnchor,
}

impl Default for CalloutTail {
    fn default() -> Self {
        Self {
            tip: Point::new(0.0, 0.0),
            base_width: 24.0,
            curve_bias: 0.0,
            anchor: TailAnchor::Nearest,
        }
    }
}

impl CalloutTail {
    fn validate(&self, v: &mut V) {
        check_point(v, "tip", self.tip);
        check_len(v, "base_width", self.base_width);
        check_range(v, "curve_bias", self.curve_bias, -1.0, 1.0);
        if let TailAnchor::Edge { t, .. } = self.anchor {
            check_range(v, "anchor.t", t, 0.0, 1.0);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TailAnchor {
    /// Base on the body edge nearest the tip (follows the drag).
    #[default]
    Nearest,
    /// Base at the body centre (the union hides the inside).
    Center,
    /// Pinned at `t` (0..1) along one side.
    Edge { side: Side, t: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

// --- Step badge -------------------------------------------------------------

/// Auto-incrementing sequence badge (`1, 2, 3` or `A, B, C`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StepBadge {
    pub center: Point,
    pub radius: f32,
    pub shape: BadgeShape,
    /// Direction of the teardrop/shield pointer, degrees clockwise from up.
    pub pointer_deg: Option<f32>,
    /// Badges in one sequence count together; a document may have several.
    pub sequence: String,
    /// 1-based position in the sequence; [`Numbering::label`] renders it.
    pub value: u32,
    pub numbering: Numbering,
    pub fill: Color,
    pub text_color: Color,
    pub font: FontSpec,
    /// White inner ring.
    pub ring: Option<Stroke>,
    pub shadow: Option<DropShadow>,
}

impl Default for StepBadge {
    fn default() -> Self {
        Self {
            center: Point::new(0.0, 0.0),
            radius: 16.0,
            shape: BadgeShape::Circle,
            pointer_deg: None,
            sequence: DEFAULT_SEQUENCE.into(),
            value: 1,
            numbering: Numbering::Decimal,
            fill: Color::rgb(0xE5, 0x3E, 0x30),
            text_color: Color::WHITE,
            font: FontSpec {
                weight: 700,
                ..FontSpec::default()
            },
            ring: None,
            shadow: Some(DropShadow::default()),
        }
    }
}

pub const DEFAULT_SEQUENCE: &str = "default";

impl StepBadge {
    pub fn label(&self) -> String {
        self.numbering.label(self.value)
    }

    fn validate(&self, v: &mut V) {
        check_point(v, "center", self.center);
        check_range(v, "radius", self.radius, 2.0, 1024.0);
        if let Some(d) = self.pointer_deg {
            check_range(v, "pointer_deg", d, -360.0, 360.0);
        }
        if self.value == 0 {
            v.err("value", "steps start at 1");
        }
        if self.sequence.len() > 256 {
            v.err("sequence", "longer than 256 bytes");
        }
        self.font.validate(v, "font");
        if let Some(s) = &self.ring {
            s.validate(v, "ring");
        }
        if let Some(s) = &self.shadow {
            s.validate(v, "shadow");
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BadgeShape {
    #[default]
    Circle,
    RoundedSquare,
    Teardrop,
    Shield,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Numbering {
    #[default]
    Decimal,
    UpperAlpha,
    LowerAlpha,
    UpperRoman,
    LowerRoman,
}

impl Numbering {
    /// `1 → "1" / "A" / "a" / "I" / "i"`; `27 → "AA"`. Roman numerals past
    /// 3999 fall back to decimal.
    pub fn label(self, value: u32) -> String {
        match self {
            Numbering::Decimal => value.to_string(),
            Numbering::UpperAlpha => alpha(value),
            Numbering::LowerAlpha => alpha(value).to_lowercase(),
            Numbering::UpperRoman => roman(value),
            Numbering::LowerRoman => roman(value).to_lowercase(),
        }
    }
}

/// Bijective base-26: 1 = A, 26 = Z, 27 = AA.
fn alpha(mut n: u32) -> String {
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn roman(mut n: u32) -> String {
    if n == 0 || n > 3999 {
        return n.to_string();
    }
    const T: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

// --- Arrow ------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CurvedArrow {
    pub start: Point,
    pub end: Point,
    /// Dragging the centre handle turns a straight arrow into a curve.
    pub curve: ArrowCurve,
    pub stroke: Stroke,
    pub start_head: ArrowHead,
    pub end_head: ArrowHead,
    /// Shaft narrows from the tail towards the head.
    pub taper: bool,
    pub shadow: Option<DropShadow>,
}

impl Default for CurvedArrow {
    fn default() -> Self {
        Self {
            start: Point::new(0.0, 0.0),
            end: Point::new(100.0, 0.0),
            curve: ArrowCurve::Straight,
            stroke: Stroke {
                color: Color::rgb(0xE5, 0x3E, 0x30),
                width: 6.0,
                ..Stroke::default()
            },
            start_head: ArrowHead {
                style: HeadStyle::None,
                ..ArrowHead::default()
            },
            end_head: ArrowHead::default(),
            taper: false,
            shadow: Some(DropShadow::default()),
        }
    }
}

impl CurvedArrow {
    fn validate(&self, v: &mut V) {
        check_point(v, "start", self.start);
        check_point(v, "end", self.end);
        match &self.curve {
            ArrowCurve::Straight => {}
            ArrowCurve::Quadratic { control } => check_point(v, "curve.control", *control),
            ArrowCurve::Cubic { c0, c1 } => {
                check_point(v, "curve.c0", *c0);
                check_point(v, "curve.c1", *c1);
            }
        }
        self.stroke.validate(v, "stroke");
        self.start_head.validate(v, "start_head");
        self.end_head.validate(v, "end_head");
        if let Some(s) = &self.shadow {
            s.validate(v, "shadow");
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArrowCurve {
    #[default]
    Straight,
    Quadratic {
        control: Point,
    },
    Cubic {
        c0: Point,
        c1: Point,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArrowHead {
    pub style: HeadStyle,
    /// Multiples of the stroke width.
    pub length: f32,
    pub width: f32,
}

impl Default for ArrowHead {
    fn default() -> Self {
        Self {
            style: HeadStyle::Triangle,
            length: 3.0,
            width: 3.0,
        }
    }
}

impl ArrowHead {
    fn validate(&self, v: &mut V, path: &str) {
        check_range(v, &format!("{path}.length"), self.length, 0.0, 32.0);
        check_range(v, &format!("{path}.width"), self.width, 0.0, 32.0);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadStyle {
    None,
    #[default]
    Triangle,
    OpenV,
    Line,
    Circle,
    Square,
    Diamond,
}

// --- Backdrop nodes -----------------------------------------------------------

/// Non-destructive blur or pixelation of everything below, inside `shape`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlurRegion {
    pub shape: RegionShape,
    pub effect: BackdropEffect,
    pub feather_px: f32,
}

impl Default for BlurRegion {
    fn default() -> Self {
        Self {
            shape: RegionShape::Rect {
                rect: RectF::new(0.0, 0.0, 100.0, 40.0),
            },
            effect: BackdropEffect::Pixelate { block_size_px: 12 },
            feather_px: 0.0,
        }
    }
}

impl BlurRegion {
    fn validate(&self, v: &mut V) {
        self.shape.validate(v, "shape");
        self.effect.validate(v, "effect");
        check_len(v, "feather_px", self.feather_px);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackdropEffect {
    Gaussian {
        sigma: f32,
    },
    /// Mosaic aligned to a `block_size_px` grid in canvas space.
    Pixelate {
        block_size_px: u32,
    },
}

impl BackdropEffect {
    fn validate(&self, v: &mut V, path: &str) {
        match *self {
            BackdropEffect::Gaussian { sigma } => {
                check_range(v, &format!("{path}.sigma"), sigma, 0.0, MAX_BLUR_SIGMA)
            }
            BackdropEffect::Pixelate { block_size_px } => {
                if !(2..=MAX_BLOCK_PX).contains(&block_size_px) {
                    v.err(
                        &format!("{path}.block_size_px"),
                        format!("must be within 2..={MAX_BLOCK_PX}"),
                    );
                }
            }
        }
    }
}

/// Loupe that scales up the pixels below it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MagnifierLens {
    /// Centre of the magnified area, canvas space.
    pub source_center: Point,
    pub lens: LensShape,
    /// [`MIN_ZOOM`]..=[`MAX_ZOOM`].
    pub zoom: f32,
    pub sampling: Sampling,
    /// Nearest-neighbour with a 1 px grid; used when `zoom >= 4`.
    pub pixel_grid: bool,
    pub border: Option<Stroke>,
    pub inner_shadow: Option<DropShadow>,
    /// Line or cone from the source area to the lens when they are apart.
    pub connector: Option<Connector>,
}

impl Default for MagnifierLens {
    fn default() -> Self {
        Self {
            source_center: Point::new(0.0, 0.0),
            lens: LensShape::Circle {
                center: Point::new(0.0, 0.0),
                radius: 80.0,
            },
            zoom: 2.0,
            sampling: Sampling::Bicubic,
            pixel_grid: false,
            border: Some(Stroke::default()),
            inner_shadow: None,
            connector: None,
        }
    }
}

impl MagnifierLens {
    fn validate(&self, v: &mut V) {
        check_point(v, "source_center", self.source_center);
        self.lens.validate(v);
        check_range(v, "zoom", self.zoom, MIN_ZOOM, MAX_ZOOM);
        if let Some(s) = &self.border {
            s.validate(v, "border");
        }
        if let Some(s) = &self.inner_shadow {
            s.validate(v, "inner_shadow");
        }
        if let Some(c) = &self.connector {
            c.stroke.validate(v, "connector.stroke");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LensShape {
    Circle { center: Point, radius: f32 },
    RoundedRect { rect: RectF, radius: f32 },
}

impl LensShape {
    pub fn bounds(&self) -> RectF {
        match *self {
            LensShape::Circle { center, radius } => {
                let r = f64::from(radius);
                RectF::new(center.x - r, center.y - r, 2.0 * r, 2.0 * r)
            }
            LensShape::RoundedRect { rect, .. } => rect,
        }
    }

    fn translate(&mut self, dx: f64, dy: f64) {
        match self {
            LensShape::Circle { center, .. } => {
                center.x += dx;
                center.y += dy;
            }
            LensShape::RoundedRect { rect, .. } => rect.translate(dx, dy),
        }
    }

    fn validate(&self, v: &mut V) {
        match *self {
            LensShape::Circle { center, radius } => {
                check_point(v, "lens.center", center);
                check_range(v, "lens.radius", radius, 4.0, MAX_LENGTH_PX);
            }
            LensShape::RoundedRect { rect, radius } => {
                rect.validate(v, "lens.rect");
                check_len(v, "lens.radius", radius);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sampling {
    #[default]
    Bicubic,
    Lanczos3,
    Nearest,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Connector {
    pub style: ConnectorStyle,
    pub stroke: Stroke,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorStyle {
    #[default]
    Line,
    /// Two tangent lines from the source ring to the lens.
    Cone,
}

/// Dims (and optionally blurs) everything outside the targets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spotlight {
    pub targets: Vec<RegionShape>,
    pub dim_color: Color,
    pub dim_opacity: f32,
    pub feather_px: f32,
    pub blur_sigma: Option<f32>,
}

impl Default for Spotlight {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            dim_color: Color::BLACK,
            dim_opacity: 0.55,
            feather_px: 4.0,
            blur_sigma: None,
        }
    }
}

impl Spotlight {
    fn validate(&self, v: &mut V) {
        if self.targets.is_empty() || self.targets.len() > 64 {
            v.err("targets", "needs 1..=64 target shapes");
        }
        for (i, t) in self.targets.iter().enumerate() {
            t.validate(v, &format!("targets[{i}]"));
        }
        check_range(v, "dim_opacity", self.dim_opacity, 0.0, 1.0);
        check_len(v, "feather_px", self.feather_px);
        if let Some(s) = self.blur_sigma {
            check_range(v, "blur_sigma", s, 0.0, MAX_BLUR_SIGMA);
        }
    }
}

// --- Cut out ------------------------------------------------------------------

/// Removes the strip `[start, end)` (base space, along `axis`) and joins
/// the two halves with a straight or torn seam.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CutOutSeam {
    pub axis: Axis,
    pub start: f64,
    pub end: f64,
    pub edge: SeamEdge,
    /// Cast from the first half onto the second.
    pub shadow: Option<DropShadow>,
    /// Transparent gap left between the halves (0 = snapped together).
    pub gap_px: f32,
}

impl Default for CutOutSeam {
    fn default() -> Self {
        Self {
            axis: Axis::Horizontal,
            start: 0.0,
            end: 0.0,
            edge: SeamEdge::Straight,
            shadow: None,
            gap_px: 0.0,
        }
    }
}

impl CutOutSeam {
    /// Removed length in pixels (0 for an inverted range).
    pub fn removed(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }

    fn validate(&self, v: &mut V) {
        if !(self.start.is_finite() && self.end.is_finite() && self.start >= 0.0) {
            v.err("start", "start/end must be finite and non-negative");
        } else if self.end <= self.start {
            v.err("end", "end must be greater than start");
        }
        if let SeamEdge::Torn {
            amplitude_px,
            roughness,
            octaves,
            ..
        } = self.edge
        {
            check_range(v, "edge.amplitude_px", amplitude_px, 0.0, 256.0);
            check_range(v, "edge.roughness", roughness, 0.0, 1.0);
            if !(1..=8).contains(&octaves) {
                v.err("edge.octaves", "must be within 1..=8");
            }
        }
        if let Some(s) = &self.shadow {
            s.validate(v, "shadow");
        }
        check_len(v, "gap_px", self.gap_px);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Removes rows: a horizontal strip, halves join vertically.
    #[default]
    Horizontal,
    /// Removes columns.
    Vertical,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SeamEdge {
    #[default]
    Straight,
    /// fBm displacement profile, deterministic for a given `seed`.
    Torn {
        seed: u64,
        amplitude_px: f32,
        roughness: f32,
        octaves: u8,
    },
}

// --- Plain shape ----------------------------------------------------------------

/// Rectangle, ellipse or polygon with fill and stroke (Shape tool, the
/// Simplify output blocks, highlighter boxes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShapeNode {
    pub shape: RegionShape,
    pub fill: Fill,
    pub stroke: Option<Stroke>,
    pub shadow: Option<DropShadow>,
    pub blend: BlendMode,
}

impl Default for ShapeNode {
    fn default() -> Self {
        Self {
            shape: RegionShape::Rect {
                rect: RectF::new(0.0, 0.0, 100.0, 60.0),
            },
            fill: Fill::None,
            stroke: Some(Stroke::default()),
            shadow: None,
            blend: BlendMode::Normal,
        }
    }
}

impl ShapeNode {
    fn validate(&self, v: &mut V) {
        self.shape.validate(v, "shape");
        self.fill.validate(v, "fill");
        if let Some(s) = &self.stroke {
            s.validate(v, "stroke");
        }
        if let Some(s) = &self.shadow {
            s.validate(v, "shadow");
        }
    }
}

// --- Pen ------------------------------------------------------------------------

/// A freehand stroke (the Pen tool): the pointer trace, drawn as the
/// smooth Bézier curve PhotoCraft's Freeform Pen fits through it
/// (`photocraft_vector::trace::fit_trace`, within `smoothing` px).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PenStroke {
    pub points: Vec<Point>,
    pub stroke: Stroke,
    /// Curve-fit tolerance, canvas px (0.1..=100); larger is smoother.
    pub smoothing: f32,
}

impl Default for PenStroke {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            stroke: Stroke {
                width: 4.0,
                join: LineJoin::Round,
                cap: LineCap::Round,
                ..Stroke::default()
            },
            smoothing: 2.0,
        }
    }
}

impl PenStroke {
    fn validate(&self, v: &mut V) {
        if !(2..=MAX_POLYGON_POINTS).contains(&self.points.len()) {
            v.err("points", format!("needs 2..={MAX_POLYGON_POINTS} points"));
        }
        for p in &self.points {
            check_point(v, "points", *p);
        }
        self.stroke.validate(v, "stroke");
        check_range(v, "smoothing", self.smoothing, 0.1, 100.0);
    }
}

// --- Smart object ----------------------------------------------------------

/// A piece of the screenshot lifted out by Smart Move: its own straight-alpha
/// RGBA8 pixels ([`object_path`], soft-matted against the background it sat
/// on), drawn scaled into `rect`. The hole it left is healed in the base
/// layer, so the object can be moved, scaled or deleted later.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartObject {
    /// Canvas-space placement; the pixels are scaled to fill it.
    pub rect: RectF,
    pub image_path: String,
    pub image_size: Size,
    /// Base-space rectangle the pixels were lifted from.
    pub source: Rect,
    /// What the UI analyzer took it for (`button`, `icon`, ...).
    pub element: String,
}

impl Default for SmartObject {
    fn default() -> Self {
        Self {
            rect: RectF::new(0.0, 0.0, 1.0, 1.0),
            image_path: object_path(NodeId(0)),
            image_size: Size::new(1, 1),
            source: Rect::new(0, 0, 1, 1),
            element: String::new(),
        }
    }
}

impl SmartObject {
    fn validate(&self, v: &mut V) {
        self.rect.validate(v, "rect");
        check_archive_path(v, "image_path", &self.image_path);
        check_size(v, "image_size", self.image_size);
        if self.image_size.is_empty() {
            v.err("image_size", "must not be empty");
        }
        if self.source.is_empty() {
            v.err("source", "must not be empty");
        }
        if self.element.len() > 64 {
            v.err("element", "longer than 64 bytes");
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    /// Highlighter.
    Multiply,
}

// ---------------------------------------------------------------------------
// Shared geometry and style

/// Sub-pixel rectangle: origin plus size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RectF {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RectF {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn point(p: Point) -> Self {
        Self::new(p.x, p.y, 0.0, 0.0)
    }

    pub fn from_corners(a: Point, b: Point) -> Self {
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        Self::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn union(self, o: RectF) -> RectF {
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.w).max(o.x + o.w);
        let y1 = (self.y + self.h).max(o.y + o.h);
        RectF::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn translate(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
    }

    /// The rectangle spanned by the two mapped corners.
    pub fn map(self, f: &dyn Fn(Point) -> Point) -> RectF {
        RectF::from_corners(
            f(Point::new(self.x, self.y)),
            f(Point::new(self.x + self.w, self.y + self.h)),
        )
    }

    fn validate(&self, v: &mut V, path: &str) {
        let lim = f64::from(MAX_SIDE) * 2.0;
        let ok = [self.x, self.y, self.w, self.h]
            .iter()
            .all(|c| c.is_finite() && c.abs() <= lim);
        if !ok || self.w < 0.0 || self.h < 0.0 {
            v.err(path, "rectangle must be finite with non-negative size");
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegionShape {
    Rect { rect: RectF },
    RoundedRect { rect: RectF, radius: f32 },
    Ellipse { rect: RectF },
    Polygon { points: Vec<Point> },
}

impl RegionShape {
    pub fn bounds(&self) -> RectF {
        match self {
            RegionShape::Rect { rect }
            | RegionShape::RoundedRect { rect, .. }
            | RegionShape::Ellipse { rect } => *rect,
            RegionShape::Polygon { points } => points
                .iter()
                .map(|p| RectF::point(*p))
                .reduce(RectF::union)
                .unwrap_or_default(),
        }
    }

    pub fn translate(&mut self, dx: f64, dy: f64) {
        match self {
            RegionShape::Rect { rect }
            | RegionShape::RoundedRect { rect, .. }
            | RegionShape::Ellipse { rect } => rect.translate(dx, dy),
            RegionShape::Polygon { points } => {
                for p in points {
                    p.x += dx;
                    p.y += dy;
                }
            }
        }
    }

    /// See [`Annotation::map_geometry`].
    pub fn map(&mut self, f: &dyn Fn(Point) -> Point, k: f64) {
        match self {
            RegionShape::Rect { rect } | RegionShape::Ellipse { rect } => *rect = rect.map(f),
            RegionShape::RoundedRect { rect, radius } => {
                *rect = rect.map(f);
                *radius = (f64::from(*radius) * k) as f32;
            }
            RegionShape::Polygon { points } => {
                for p in points {
                    *p = f(*p);
                }
            }
        }
    }

    fn validate(&self, v: &mut V, path: &str) {
        match self {
            RegionShape::Rect { rect } | RegionShape::Ellipse { rect } => {
                rect.validate(v, &format!("{path}.rect"))
            }
            RegionShape::RoundedRect { rect, radius } => {
                rect.validate(v, &format!("{path}.rect"));
                check_len(v, &format!("{path}.radius"), *radius);
            }
            RegionShape::Polygon { points } => {
                if !(3..=MAX_POLYGON_POINTS).contains(&points.len()) {
                    v.err(
                        &format!("{path}.points"),
                        format!("needs 3..={MAX_POLYGON_POINTS} points"),
                    );
                }
                for p in points {
                    check_point(v, &format!("{path}.points"), *p);
                }
            }
        }
    }
}

/// Straight-alpha sRGB colour, written as `"#rrggbbaa"` (or `"#rrggbb"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

impl From<Color> for String {
    fn from(c: Color) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
    }
}

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, String> {
        let hex = s.strip_prefix('#').unwrap_or(&s);
        let byte = |i: usize| {
            hex.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| format!("bad colour `{s}`: expected #rrggbb or #rrggbbaa"))
        };
        match hex.len() {
            6 => Ok(Color::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Ok(Color::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => Err(format!("bad colour `{s}`: expected #rrggbb or #rrggbbaa")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stroke {
    pub color: Color,
    pub width: f32,
    pub dash: DashStyle,
    pub join: LineJoin,
    pub cap: LineCap,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            color: Color::rgb(0xE5, 0x3E, 0x30),
            width: 3.0,
            dash: DashStyle::Solid,
            join: LineJoin::Round,
            cap: LineCap::Round,
        }
    }
}

impl Stroke {
    fn validate(&self, v: &mut V, path: &str) {
        check_range(v, &format!("{path}.width"), self.width, 0.0, 512.0);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineJoin {
    Miter,
    #[default]
    Round,
    Bevel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineCap {
    Butt,
    #[default]
    Round,
    Square,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fill {
    #[default]
    None,
    Solid {
        color: Color,
    },
    Linear {
        from: Point,
        to: Point,
        stops: Vec<GradientStop>,
    },
    /// Circular gradient: `stops` run from `center` (offset 0) to
    /// `radius` (offset 1).
    Radial {
        center: Point,
        radius: f32,
        stops: Vec<GradientStop>,
    },
}

impl Fill {
    /// Moves gradient points with the geometry (see
    /// [`Annotation::map_geometry`]); the radius follows the mapped x axis.
    pub fn map(&mut self, f: &dyn Fn(Point) -> Point) {
        match self {
            Fill::None | Fill::Solid { .. } => {}
            Fill::Linear { from, to, .. } => {
                *from = f(*from);
                *to = f(*to);
            }
            Fill::Radial { center, radius, .. } => {
                let edge = f(Point::new(center.x + f64::from(*radius), center.y));
                *center = f(*center);
                *radius = (edge.x - center.x).hypot(edge.y - center.y) as f32;
            }
        }
    }

    fn validate(&self, v: &mut V, path: &str) {
        let stops = match self {
            Fill::None | Fill::Solid { .. } => return,
            Fill::Linear { from, to, stops } => {
                check_point(v, &format!("{path}.from"), *from);
                check_point(v, &format!("{path}.to"), *to);
                stops
            }
            Fill::Radial {
                center,
                radius,
                stops,
            } => {
                check_point(v, &format!("{path}.center"), *center);
                check_len(v, &format!("{path}.radius"), *radius);
                stops
            }
        };
        if !(2..=16).contains(&stops.len()) {
            v.err(&format!("{path}.stops"), "needs 2..=16 stops");
        }
        for s in stops {
            check_range(v, &format!("{path}.stops.offset"), s.offset, 0.0, 1.0);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Color,
}

/// Outer drop shadow; `color.a` is its opacity.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DropShadow {
    pub color: Color,
    pub offset_x: f32,
    pub offset_y: f32,
    /// Gaussian blur radius (≈ 2σ).
    pub blur: f32,
    pub spread: f32,
}

impl Default for DropShadow {
    fn default() -> Self {
        Self {
            color: Color::rgba(0, 0, 0, 0x66),
            offset_x: 3.0,
            offset_y: 3.0,
            blur: 6.0,
            spread: 0.0,
        }
    }
}

impl DropShadow {
    fn validate(&self, v: &mut V, path: &str) {
        for (n, x) in [("offset_x", self.offset_x), ("offset_y", self.offset_y)] {
            check_range(v, &format!("{path}.{n}"), x, -MAX_LENGTH_PX, MAX_LENGTH_PX);
        }
        check_range(v, &format!("{path}.blur"), self.blur, 0.0, 512.0);
        check_range(v, &format!("{path}.spread"), self.spread, -512.0, 512.0);
    }
}

/// Rich text inside a callout (shaped by `photocraft-text`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextBlock {
    pub runs: Vec<TextRun>,
    pub align: TextAlign,
    pub valign: VAlign,
    pub padding: f32,
}

impl Default for TextBlock {
    fn default() -> Self {
        Self {
            runs: Vec::new(),
            align: TextAlign::Center,
            valign: VAlign::Middle,
            padding: 10.0,
        }
    }
}

impl TextBlock {
    pub fn plain(text: &str, font: FontSpec, color: Color) -> Self {
        Self {
            runs: vec![TextRun {
                text: text.into(),
                font,
                color,
                underline: false,
                strike: false,
            }],
            ..Self::default()
        }
    }

    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    fn validate(&self, v: &mut V, path: &str) {
        let bytes: usize = self.runs.iter().map(|r| r.text.len()).sum();
        if bytes > MAX_TEXT_BYTES {
            v.err(path, format!("text longer than {MAX_TEXT_BYTES} bytes"));
        }
        for r in &self.runs {
            r.font.validate(v, &format!("{path}.font"));
        }
        check_len(v, &format!("{path}.padding"), self.padding);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextRun {
    pub text: String,
    pub font: FontSpec,
    pub color: Color,
    pub underline: bool,
    pub strike: bool,
}

impl Default for TextRun {
    fn default() -> Self {
        Self {
            text: String::new(),
            font: FontSpec::default(),
            color: Color::BLACK,
            underline: false,
            strike: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontSpec {
    /// Family name; resolved through `photocraft-text` (and `CRAFT_FONTS_DIR`).
    pub family: String,
    pub size: f32,
    pub weight: u16,
    pub italic: bool,
}

impl Default for FontSpec {
    fn default() -> Self {
        Self {
            family: "Inter".into(),
            size: 16.0,
            weight: 400,
            italic: false,
        }
    }
}

impl FontSpec {
    fn validate(&self, v: &mut V, path: &str) {
        check_range(v, &format!("{path}.size"), self.size, 1.0, 1024.0);
        if !(1..=1000).contains(&self.weight) {
            v.err(&format!("{path}.weight"), "must be within 1..=1000");
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

// ---------------------------------------------------------------------------
// Validation plumbing

/// Collects the first error with its JSON path.
#[derive(Default)]
struct V {
    prefix: Vec<String>,
    first: Option<FormatError>,
}

impl V {
    fn err(&mut self, path: &str, msg: impl Into<String>) {
        if self.first.is_none() {
            let mut full = self.prefix.join(".");
            if !path.is_empty() {
                if !full.is_empty() {
                    full.push('.');
                }
                full.push_str(path);
            }
            self.first = Some(FormatError::Invalid {
                path: full,
                msg: msg.into(),
            });
        }
    }

    fn at(&mut self, segment: &str, f: impl FnOnce(&mut V)) {
        self.prefix.push(segment.to_string());
        f(self);
        self.prefix.pop();
    }

    fn finish(&mut self) -> Result<()> {
        self.first.take().map_or(Ok(()), Err)
    }
}

fn check_size(v: &mut V, path: &str, s: Size) {
    if s.width > MAX_SIDE || s.height > MAX_SIDE || s.area() > MAX_PIXELS {
        v.err(
            path,
            format!(
                "{}×{} exceeds the {MAX_SIDE} px / {MAX_PIXELS} px² limit",
                s.width, s.height
            ),
        );
    }
}

fn check_point(v: &mut V, path: &str, p: Point) {
    let lim = f64::from(MAX_SIDE) * 2.0;
    if !(p.x.is_finite() && p.y.is_finite() && p.x.abs() <= lim && p.y.abs() <= lim) {
        v.err(path, "point must be finite and within the canvas range");
    }
}

fn check_range(v: &mut V, path: &str, x: f32, lo: f32, hi: f32) {
    // `contains` is false for NaN.
    if !(lo..=hi).contains(&x) {
        v.err(path, format!("{x} is outside {lo}..={hi}"));
    }
}

fn check_len(v: &mut V, path: &str, x: f32) {
    check_range(v, path, x, 0.0, MAX_LENGTH_PX);
}

/// Archive member names: relative, `/`-separated, no `..`, no drive letters.
fn check_archive_path(v: &mut V, path: &str, member: &str) {
    let bad = member.is_empty()
        || member.len() > 512
        || member.starts_with('/')
        || member.contains('\\')
        || member.contains(':')
        || member
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..");
    if bad {
        v.err(path, format!("`{member}` is not a safe archive path"));
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        let mut m = Manifest::new(Size::new(600, 300));
        m.base.tiles.push(TileEntry {
            coord: TileCoord::new(1, 0),
            data: TileData::File {
                path: tile_path("base", TileCoord::new(1, 0)),
                sha256: None,
            },
        });
        m.cursor = Some(CursorLayer::default());
        let id = m.alloc_node_id();
        m.annotations.push(AnnotationNode::new(
            id,
            Annotation::Callout(Callout {
                tails: vec![CalloutTail {
                    tip: Point::new(300.0, 250.0),
                    ..CalloutTail::default()
                }],
                ..Callout::default()
            }),
        ));
        let id = m.alloc_node_id();
        m.annotations.push(AnnotationNode::new(
            id,
            Annotation::CutOutSeam(CutOutSeam {
                start: 100.0,
                end: 140.0,
                edge: SeamEdge::Torn {
                    seed: 7,
                    amplitude_px: 6.0,
                    roughness: 0.5,
                    octaves: 4,
                },
                ..CutOutSeam::default()
            }),
        ));
        m
    }

    #[test]
    fn round_trips() {
        let m = sample();
        let json = m.to_json().unwrap_or_default();
        let back = Manifest::from_json(&json);
        assert_eq!(back.ok(), Some(m));
    }

    #[test]
    fn type_table_matches_variants() {
        let all = [
            Annotation::Callout(Callout::default()),
            Annotation::StepBadge(StepBadge::default()),
            Annotation::CurvedArrow(CurvedArrow::default()),
            Annotation::BlurRegion(BlurRegion::default()),
            Annotation::MagnifierLens(MagnifierLens::default()),
            Annotation::Spotlight(Spotlight::default()),
            Annotation::CutOutSeam(CutOutSeam::default()),
            Annotation::Shape(ShapeNode::default()),
            Annotation::SmartObject(SmartObject::default()),
            Annotation::Pen(PenStroke::default()),
        ];
        let names: Vec<_> = all.iter().map(Annotation::type_name).collect();
        assert_eq!(names, Annotation::TYPES);
        for a in all {
            let v = serde_json::to_value(&a).unwrap_or_default();
            assert_eq!(v["type"], a.type_name());
        }
    }

    #[test]
    fn missing_fields_take_defaults() {
        let m = Manifest::from_json(
            br#"{"canvas":{"size":{"width":10,"height":10}},
            "annotations":[{"id":4,"kind":{"type":"step_badge","value":3}}]}"#,
        );
        let m = m.ok();
        let node = m.as_ref().and_then(|m| m.annotations.first());
        assert_eq!(node.map(|n| n.visible), Some(true));
        assert_eq!(node.map(|n| n.opacity), Some(1.0));
        let Some(Annotation::StepBadge(b)) = node.and_then(AnnotationNode::annotation) else {
            panic!("step badge expected");
        };
        assert_eq!(b.value, 3);
        assert_eq!(b.sequence, DEFAULT_SEQUENCE);
    }

    #[test]
    fn unknown_types_survive_a_save() {
        let src = br#"{"annotations":[{"id":1,"kind":{"type":"hologram","glow":9}}]}"#;
        let m = Manifest::from_json(src).ok();
        let json = m.and_then(|m| m.to_json().ok()).unwrap_or_default();
        let v: Value = serde_json::from_slice(&json).unwrap_or_default();
        assert_eq!(v["annotations"][0]["kind"]["glow"], 9);
    }

    #[test]
    fn rejects_bad_input() {
        let cases: [&[u8]; 8] = [
            br#"{"format_version":99}"#,
            br#"{"canvas":{"size":{"width":300000,"height":1}}}"#,
            br#"{"annotations":[{"id":1,"kind":{"type":"magnifier_lens","zoom":20}}]}"#,
            br#"{"annotations":[{"id":1,"kind":{"type":"callout","fill":7}}]}"#,
            br#"{"annotations":[{"id":1,"kind":{"type":"shape"}},{"id":1,"kind":{"type":"shape"}}]}"#,
            br##"{"base":{"size":{"width":10,"height":10},"tiles":[{"coord":{"tx":3,"ty":0},"storage":"solid","color":"#ffffff"}]}}"##,
            br#"{"base":{"size":{"width":10,"height":10},"tiles":[{"coord":{"tx":0,"ty":0},"storage":"file","path":"../../etc/passwd"}]}}"#,
            br#"{"annotations":[{"id":1,"kind":{"type":"cut_out_seam","start":50,"end":10}}]}"#,
        ];
        for c in cases {
            assert!(
                Manifest::from_json(c).is_err(),
                "{}",
                String::from_utf8_lossy(c)
            );
        }
    }

    #[test]
    fn validation_reports_the_path() {
        let mut m = sample();
        if let Some(Annotation::Callout(c)) = m.annotations[0].annotation_mut() {
            c.tails[0].curve_bias = f32::NAN;
        }
        let err = m
            .validate()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err.starts_with("annotations[0].kind.tails[0].curve_bias"),
            "{err}"
        );
    }

    #[test]
    fn numbering_labels() {
        assert_eq!(Numbering::UpperAlpha.label(1), "A");
        assert_eq!(Numbering::UpperAlpha.label(26), "Z");
        assert_eq!(Numbering::LowerAlpha.label(28), "ab");
        assert_eq!(Numbering::UpperRoman.label(14), "XIV");
        assert_eq!(Numbering::LowerRoman.label(4000), "4000");
    }

    #[test]
    fn colours_parse_and_print() {
        assert_eq!(
            Color::try_from("#11223344".to_string()),
            Ok(Color::rgba(0x11, 0x22, 0x33, 0x44))
        );
        assert_eq!(
            Color::try_from("#abcdef".to_string()),
            Ok(Color::rgb(0xab, 0xcd, 0xef))
        );
        assert!(Color::try_from("#ab€def".to_string()).is_err());
        assert_eq!(String::from(Color::WHITE), "#ffffffff");
    }

    #[test]
    fn node_ids_are_never_reused() {
        let mut m = sample();
        m.annotations.clear();
        let a = m.alloc_node_id();
        let b = m.alloc_node_id();
        assert!(b > a);
    }

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
    }

    #[test]
    fn rotation_turns_clockwise_about_the_pivot() {
        let mut n = AnnotationNode::new(
            NodeId(1),
            Annotation::Shape(ShapeNode {
                shape: RegionShape::Rect {
                    rect: RectF::new(0.0, 0.0, 100.0, 60.0),
                },
                ..ShapeNode::default()
            }),
        );
        assert_eq!(n.transform(), Affine::IDENTITY);
        n.rotation_rad = std::f32::consts::FRAC_PI_2;
        let t = n.transform();
        // The centre stays; the right-edge midpoint swings down (y down).
        assert!(close(
            t.apply(Point::new(50.0, 30.0)),
            Point::new(50.0, 30.0)
        ));
        let p = t.apply(Point::new(100.0, 30.0));
        assert!(
            (p.x - 50.0).abs() < 1e-5 && (p.y - 80.0).abs() < 1e-5,
            "{p:?}"
        );
    }

    #[test]
    fn rotation_is_optional_and_bounded() {
        let mut m = sample();
        let json = String::from_utf8(m.to_json().unwrap_or_default()).unwrap_or_default();
        let old = json.replace("\"rotation_rad\": 0.0,", "");
        assert!(!old.contains("rotation_rad"));
        assert_eq!(Manifest::from_json(old.as_bytes()).ok(), Some(m.clone()));
        m.annotations[0].rotation_rad = 4.0;
        assert!(m.validate().is_err());
        m.annotations[0].rotation_rad = normalize_angle(4.0);
        assert!(m.validate().is_ok());
    }

    #[test]
    fn angles_wrap_into_minus_pi_to_pi() {
        use std::f32::consts::PI;
        for (a, want) in [
            (0.0, 0.0),
            (PI, PI),
            (-PI, PI),
            (1.5 * PI, -0.5 * PI),
            (f32::NAN, 0.0),
        ] {
            assert!(
                (normalize_angle(a) - want).abs() < 1e-5,
                "{a} -> {}",
                normalize_angle(a)
            );
        }
    }

    #[test]
    fn resize_maps_bounds_onto_the_target() {
        let mut a = Annotation::Callout(Callout {
            bounds: RectF::new(10.0, 10.0, 100.0, 50.0),
            body: CalloutBody::RoundedRect { radius: 10.0 },
            tails: vec![CalloutTail {
                tip: Point::new(110.0, 110.0),
                ..CalloutTail::default()
            }],
            ..Callout::default()
        });
        let Some(from) = a.bounds() else {
            panic!("no bounds")
        };
        let to = RectF::new(0.0, 0.0, 200.0, 50.0);
        a.resize(from, to);
        let Some(b) = a.bounds() else {
            panic!("no bounds")
        };
        assert!(close(Point::new(b.x, b.y), Point::new(0.0, 0.0)));
        assert!(close(Point::new(b.w, b.h), Point::new(200.0, 50.0)));
        let Annotation::Callout(c) = &a else {
            panic!("kind changed")
        };
        assert_eq!(c.bounds, RectF::new(0.0, 0.0, 200.0, 25.0));
        assert_eq!(c.body, CalloutBody::RoundedRect { radius: 10.0 });
    }

    #[test]
    fn resize_scales_radii_and_gradients() {
        let mut a = Annotation::StepBadge(StepBadge {
            center: Point::new(50.0, 50.0),
            radius: 10.0,
            ..StepBadge::default()
        });
        a.resize(
            RectF::new(40.0, 40.0, 20.0, 20.0),
            RectF::new(0.0, 0.0, 40.0, 40.0),
        );
        let Annotation::StepBadge(b) = &a else {
            panic!("kind changed")
        };
        assert!(close(b.center, Point::new(20.0, 20.0)));
        assert_eq!(b.radius, 20.0);

        let mut f = Fill::Radial {
            center: Point::new(10.0, 10.0),
            radius: 5.0,
            stops: Vec::new(),
        };
        f.map(&|p| Point::new(p.x * 2.0, p.y * 2.0));
        assert_eq!(
            f,
            Fill::Radial {
                center: Point::new(20.0, 20.0),
                radius: 10.0,
                stops: Vec::new()
            }
        );
    }

    #[test]
    fn backdrop_defaults_to_none_and_grows_the_frame() {
        let mut m = sample();
        assert_eq!(m.canvas.backdrop, CanvasBackdrop::None);
        assert_eq!(m.canvas.backdrop.frame_size(m.canvas.size), m.canvas.size);
        m.canvas.backdrop = CanvasBackdrop::rounded_shadow_frame();
        assert!(m.validate().is_ok());
        assert_eq!(
            m.canvas.backdrop.frame_size(m.canvas.size),
            Size::new(600 + 96, 300 + 96)
        );
        let back = Manifest::from_json(&m.to_json().unwrap_or_default()).ok();
        assert_eq!(
            back.map(|b| b.canvas.backdrop),
            Some(m.canvas.backdrop.clone())
        );
        if let CanvasBackdrop::RoundedShadowFrame { margin_px, .. } = &mut m.canvas.backdrop {
            *margin_px = -1.0;
        }
        assert!(m.validate().is_err());
    }

    #[test]
    fn radial_fills_need_stops() {
        let mut m = sample();
        let Some(Annotation::Callout(c)) = m.annotations[0].annotation_mut() else {
            panic!("sample changed")
        };
        c.fill = Fill::Radial {
            center: Point::new(0.0, 0.0),
            radius: 40.0,
            stops: vec![GradientStop {
                offset: 0.0,
                color: Color::WHITE,
            }],
        };
        assert!(m.validate().is_err());
    }
}
