//! Serializable UI state: the floating Capture HUD, the crosshair overlay
//! preferences and the five-zone editor (toolbar, Quick Styles sidebar,
//! canvas, Recent Tray, Library).
//!
//! Everything here is plain data, persisted between runs with
//! `#[serde(default)]` so older state files keep loading. Mid-gesture state
//! (a canvas resize drag) is `#[serde(skip)]`. Document content is not
//! here: the engine owns it, and the UI refers to documents by id and to
//! annotations by [`NodeId`]. Colours and sizes of the chrome come from
//! `theme::Tokens`, selected by [`SidebarState::theme`].
//!
//! Names used by the design docs: `CaptureHudState` is [`CaptureHud`],
//! `ActiveTool` is [`ToolId`], `QuickStylePreset` is [`QuickStyle`] (with
//! its [`StylePreset`]) and `RecentTrayItem` is [`TrayItem`].

use capturecraft_format::{
    Axis, Color, DropShadow, Fill, FontSpec, HeadStyle, NodeId, Numbering, Offset, RectF, Stroke,
};
use photocraft_geom::{Point, Rect, Size};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    pub hud: CaptureHud,
    pub crosshair: CrosshairPrefs,
    /// The crosshair session in progress; starts fresh on every capture.
    /// The control channel serializes it on its own.
    #[serde(skip)]
    pub overlay: CrosshairOverlayState,
    pub editor: EditorState,
}

// ---------------------------------------------------------------------------
// Capture HUD

/// The compact floating capture window (also reachable from the tray).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureHud {
    pub tab: CaptureTab,
    /// Each tab keeps its own settings, as Snagit does.
    pub all_in_one: CaptureSettings,
    pub image: CaptureSettings,
    pub video: VideoSettings,
    pub window: HudWindow,
    /// Global hotkey for the Capture button, egui notation.
    pub hotkey: String,
    pub presets: Vec<CapturePreset>,
}

impl Default for CaptureHud {
    fn default() -> Self {
        Self {
            tab: CaptureTab::AllInOne,
            all_in_one: CaptureSettings::default(),
            image: CaptureSettings::default(),
            video: VideoSettings::default(),
            window: HudWindow::default(),
            hotkey: "PrintScreen".into(),
            presets: Vec::new(),
        }
    }
}

impl CaptureHud {
    /// The settings the Capture button uses right now.
    pub fn current(&self) -> &CaptureSettings {
        match self.tab {
            CaptureTab::AllInOne => &self.all_in_one,
            CaptureTab::Image => &self.image,
            CaptureTab::Video => &self.video.capture,
        }
    }

    pub fn current_mut(&mut self) -> &mut CaptureSettings {
        match self.tab {
            CaptureTab::AllInOne => &mut self.all_in_one,
            CaptureTab::Image => &mut self.image,
            CaptureTab::Video => &mut self.video.capture,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureTab {
    #[default]
    AllInOne,
    Image,
    Video,
}

impl CaptureTab {
    pub const ALL: [CaptureTab; 3] = [CaptureTab::AllInOne, CaptureTab::Image, CaptureTab::Video];

    pub fn label(self) -> &'static str {
        match self {
            CaptureTab::AllInOne => "All-in-One",
            CaptureTab::Image => "Image",
            CaptureTab::Video => "Video",
        }
    }
}

/// The configuration rows of a HUD tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    pub selection: SelectionType,
    pub effects: Vec<CaptureEffect>,
    pub share: ShareDestination,
    /// Capture the cursor as its own layer.
    pub capture_cursor: bool,
    /// 0 = capture immediately.
    pub delay_secs: u32,
    /// Copy to the clipboard as well as the destination.
    pub copy_to_clipboard: bool,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            selection: SelectionType::Region,
            effects: Vec::new(),
            share: ShareDestination::Editor,
            capture_cursor: false,
            delay_secs: 0,
            copy_to_clipboard: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SelectionType {
    /// Drag a region; hovering snaps to windows and controls.
    #[default]
    Region,
    Window,
    Fullscreen,
    /// Panoramic capture of content taller/wider than the screen.
    Scrolling {
        axis: Axis,
        mode: ScrollMode,
    },
    Menu,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollMode {
    /// CaptureCraft sends scroll events.
    #[default]
    Auto,
    /// The user scrolls; frames are stitched by optical flow.
    Manual,
}

/// Effects applied to the capture before it reaches the destination.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureEffect {
    Border {
        color: Color,
        width: f32,
    },
    Shadow {
        shadow: DropShadow,
    },
    /// Torn edges on the chosen sides (top, right, bottom, left).
    TornEdge {
        sides: [bool; 4],
        amplitude_px: f32,
    },
    /// The rounded shadow frame (`CanvasBackdrop::rounded_shadow_frame`).
    RoundedFrame,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShareDestination {
    #[default]
    Editor,
    Clipboard,
    File {
        directory: String,
        /// `chrono` pattern, e.g. `Capture %Y-%m-%d %H%M%S`.
        name_pattern: String,
        format: ImageFormat,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
    Avif,
    Gif,
    Pdf,
    /// The flattened capture as an `<image>` in an SVG (raster inside).
    Svg,
}

impl ImageFormat {
    /// What Share / Export offers, in menu order.
    pub const EXPORT: [ImageFormat; 5] = [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Webp,
        ImageFormat::Svg,
        ImageFormat::Pdf,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Webp => "WebP",
            ImageFormat::Avif => "AVIF",
            ImageFormat::Gif => "GIF",
            ImageFormat::Pdf => "PDF",
            ImageFormat::Svg => "SVG",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
            ImageFormat::Avif => "avif",
            ImageFormat::Gif => "gif",
            ImageFormat::Pdf => "pdf",
            ImageFormat::Svg => "svg",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoSettings {
    pub capture: CaptureSettings,
    pub record_microphone: bool,
    /// `None` = system default input.
    pub microphone_device: Option<String>,
    pub record_system_audio: bool,
    pub webcam: Option<WebcamPip>,
    pub output: VideoOutput,
    pub fps: u32,
    /// Draw on screen while recording.
    pub live_annotations: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            capture: CaptureSettings {
                capture_cursor: true,
                ..CaptureSettings::default()
            },
            record_microphone: false,
            microphone_device: None,
            record_system_audio: false,
            webcam: None,
            output: VideoOutput::Mp4,
            fps: 30,
            live_annotations: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebcamPip {
    pub device: Option<String>,
    pub corner: Corner,
    /// Width as a percentage of the recording.
    pub size_pct: f32,
}

impl Default for WebcamPip {
    fn default() -> Self {
        Self {
            device: None,
            corner: Corner::BottomRight,
            size_pct: 20.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoOutput {
    #[default]
    Mp4,
    Gif,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudWindow {
    /// Logical pixels; `None` = centred on the primary monitor.
    pub position: Option<[f32; 2]>,
    pub collapsed: bool,
    pub always_on_top: bool,
    /// Hide the HUD while the crosshair is up.
    pub hide_during_capture: bool,
}

impl Default for HudWindow {
    fn default() -> Self {
        Self {
            position: None,
            collapsed: false,
            always_on_top: true,
            hide_during_capture: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CapturePreset {
    pub name: String,
    pub tab: CaptureTab,
    pub settings: CaptureSettings,
    pub hotkey: Option<String>,
}

impl Default for CapturePreset {
    fn default() -> Self {
        Self {
            name: String::new(),
            tab: CaptureTab::Image,
            settings: CaptureSettings::default(),
            hotkey: None,
        }
    }
}

/// Preferences of the crosshair overlay and its pixel loupe.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CrosshairPrefs {
    pub loupe: bool,
    /// Points per desktop pixel in the loupe (nearest neighbour, pixel grid).
    pub loupe_zoom: u8,
    /// The loupe shows `2 × radius + 1` pixels square (7 → 15 × 15).
    pub loupe_radius: u32,
    pub color_code: ColorCode,
    /// `W × H` next to the cursor while dragging.
    pub show_dimensions: bool,
    /// Highlight windows and controls under the cursor.
    pub snap_to_windows: bool,
    /// Full-screen crosshair lines.
    pub crosshair_lines: bool,
}

impl Default for CrosshairPrefs {
    fn default() -> Self {
        Self {
            loupe: true,
            loupe_zoom: 10,
            loupe_radius: 7,
            color_code: ColorCode::Hex,
            show_dimensions: true,
            snap_to_windows: true,
            crosshair_lines: true,
        }
    }
}

impl CrosshairPrefs {
    /// Loupe edge in desktop pixels (radius capped as `FrozenDesktop::loupe` caps it).
    pub fn loupe_cells(&self) -> u32 {
        2 * self.loupe_radius.min(64) + 1
    }

    /// Loupe edge in points at `ppp` (cells snap to whole hardware pixels).
    pub fn loupe_side(&self, ppp: f32) -> f32 {
        self.loupe_cells() as f32 * crate::dpi::loupe_cell_pt(self.loupe_zoom, ppp)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorCode {
    #[default]
    Hex,
    Rgb,
}

/// One crosshair session over the frozen desktop. Coordinates are global
/// desktop units (`capturecraft_capture` docs), not egui points.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CrosshairOverlayState {
    pub phase: OverlayPhase,
    pub cursor: Option<[i32; 2]>,
    /// What a click would capture right now.
    pub hover: Option<HoverTarget>,
    /// Capture as soon as a selection locks (mouse release, window
    /// click) instead of showing the action bar: the Image tab.
    pub capture_on_select: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OverlayPhase {
    #[default]
    Hovering,
    Dragging {
        from: [i32; 2],
        to: [i32; 2],
    },
    /// Locked; the action bar is up.
    Selected {
        rect: Rect,
        source: SelectionSource,
    },
}

impl OverlayPhase {
    /// The rectangle a drag covers, both corner pixels included.
    pub fn drag_rect(from: [i32; 2], to: [i32; 2]) -> Rect {
        Rect::new(
            from[0].min(to[0]),
            from[1].min(to[1]),
            from[0].max(to[0]).saturating_add(1),
            from[1].max(to[1]).saturating_add(1),
        )
    }

    /// The selection on screen: the dragged rect or the locked one.
    pub fn selection(&self) -> Option<Rect> {
        match self {
            OverlayPhase::Dragging { from, to } => Some(Self::drag_rect(*from, *to)),
            OverlayPhase::Selected { rect, .. } => Some(*rect),
            OverlayPhase::Hovering => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SelectionSource {
    #[default]
    Region,
    /// A window or a control inside it (`element`).
    Window {
        id: u64,
        element: bool,
    },
    Display {
        id: u32,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HoverTarget {
    pub rect: Rect,
    pub source: SelectionSource,
    /// `App — Title`, the element role, or the monitor name.
    pub label: String,
}

// ---------------------------------------------------------------------------
// Editor

/// Tool settings the Property Inspector shows besides the style.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolOptions {
    pub selection: SelectionOptions,
    pub step: StepOptions,
    pub callout: CalloutOptions,
    pub cut_out: CutOutOptions,
    pub simplify: SimplifyOptions,
    pub shape: ShapeOptions,
    pub blur: BlurOptions,
    pub pixelate: PixelateOptions,
    pub eraser: EraserOptions,
    pub magnify: MagnifyOptions,
    pub spotlight: SpotlightOptions,
}

/// The Shape tool's outline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShapeOptions {
    pub kind: ShapeKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeKind {
    #[default]
    Rectangle,
    Ellipse,
    /// Click the corners; double-click or Enter closes it.
    Polygon,
}

impl ShapeOptions {
    /// `annotation.addShape`'s `kind` for a dragged shape.
    pub fn kind(self) -> &'static str {
        if self.kind == ShapeKind::Ellipse {
            "ellipse"
        } else {
            "rect"
        }
    }
}

/// The Eraser's brush.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EraserOptions {
    /// Canvas pixels.
    pub radius: f32,
    pub hardness: f32,
}

impl Default for EraserOptions {
    fn default() -> Self {
        Self {
            radius: 12.0,
            hardness: 0.8,
        }
    }
}

/// The Magnify tool's lens.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MagnifyOptions {
    pub zoom: f32,
    /// Lens radius a click places, canvas pixels (a drag sets it).
    pub radius: f32,
}

impl MagnifyOptions {
    pub const ZOOMS: [f32; 4] = [1.5, 2.0, 3.0, 4.0];
}

impl Default for MagnifyOptions {
    fn default() -> Self {
        Self {
            zoom: 2.0,
            radius: 80.0,
        }
    }
}

/// The Spotlight tool's target and dimming.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpotlightOptions {
    pub ellipse: bool,
    pub dim_opacity: f32,
}

impl Default for SpotlightOptions {
    fn default() -> Self {
        Self {
            ellipse: false,
            dim_opacity: 0.55,
        }
    }
}

/// The Blur tool's strength.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlurOptions {
    /// Gaussian sigma, canvas pixels.
    pub sigma: f32,
}

impl BlurOptions {
    /// The Light / Medium / Strong cards.
    pub const PRESETS: [(&'static str, f32); 3] =
        [("Light", 4.0), ("Medium", 8.0), ("Strong", 16.0)];
}

impl Default for BlurOptions {
    fn default() -> Self {
        Self { sigma: 8.0 }
    }
}

/// The Pixelate tool's block size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PixelateOptions {
    pub block_size_px: u32,
}

impl PixelateOptions {
    /// The Fine / Medium / Coarse cards.
    pub const PRESETS: [(&'static str, u32); 3] = [("Fine", 6), ("Medium", 12), ("Coarse", 24)];
}

impl Default for PixelateOptions {
    fn default() -> Self {
        Self { block_size_px: 12 }
    }
}

/// The Selection tool's mode tiles and options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SelectionOptions {
    pub mode: SelectionMode,
    /// Snap the marquee to the UI element under the cursor (the analyzer's).
    pub snap_to_object: bool,
    /// What fills the hole a moved or deleted selection leaves.
    pub background: SelectionFill,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    #[default]
    Rectangle,
    Ellipse,
    Polygon,
    Lasso,
}

impl SelectionMode {
    pub const ALL: [SelectionMode; 4] = [
        SelectionMode::Rectangle,
        SelectionMode::Ellipse,
        SelectionMode::Polygon,
        SelectionMode::Lasso,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SelectionMode::Rectangle => "Rectangle",
            SelectionMode::Ellipse => "Ellipse",
            SelectionMode::Polygon => "Polygon",
            SelectionMode::Lasso => "Freehand Lasso",
        }
    }

    /// The engine moves and heals rectangles only; the shaped modes wait
    /// for masked regions.
    pub fn available(self) -> bool {
        self == SelectionMode::Rectangle
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionFill {
    #[default]
    Transparent,
    /// Inpainted by the UI analyzer (`capturecraft-ocr`).
    AutoFill,
}

impl SelectionFill {
    pub fn label(self) -> &'static str {
        match self {
            SelectionFill::Transparent => "Transparent",
            SelectionFill::AutoFill => "Auto Fill",
        }
    }

    /// The `background` parameter of `smart.moveObject` / `smart.deleteObject`.
    pub fn param(self) -> &'static str {
        match self {
            SelectionFill::Transparent => "transparent",
            SelectionFill::AutoFill => "heal",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StepOptions {
    pub numbering: Numbering,
    pub sequence: String,
}

impl Default for StepOptions {
    fn default() -> Self {
        Self {
            numbering: Numbering::Decimal,
            sequence: capturecraft_format::DEFAULT_SEQUENCE.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalloutOptions {
    /// Tail bend of new tails, `-1..=1` (0 = straight).
    pub tail_curve: f32,
    pub tail_width: f32,
}

impl Default for CalloutOptions {
    fn default() -> Self {
        Self {
            tail_curve: 0.0,
            tail_width: 24.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CutOutOptions {
    pub axis: Axis,
    pub torn: bool,
    /// `0..=1`, gain per noise octave.
    pub roughness: f32,
    pub amplitude_px: f32,
}

impl Default for CutOutOptions {
    fn default() -> Self {
        Self {
            axis: Axis::Horizontal,
            torn: false,
            roughness: 0.5,
            amplitude_px: 8.0,
        }
    }
}

impl CutOutOptions {
    /// `canvas.cutOut` parameters removing `[a, b)` (either order).
    pub fn params(&self, a: f64, b: f64) -> Value {
        let mut p = json!({
            "axis": self.axis,
            "start": a.min(b),
            "end": a.max(b),
            "edge": if self.torn { "torn" } else { "straight" },
        });
        if self.torn {
            p["roughness"] = json!(self.roughness);
            p["amplitude_px"] = json!(self.amplitude_px);
        }
        p
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SimplifyOptions {
    /// `smart.simplify` palette name.
    pub palette: String,
}

impl Default for SimplifyOptions {
    fn default() -> Self {
        Self {
            palette: "match".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorState {
    /// Zone 1, top centre.
    pub toolbar: ToolbarState,
    /// Zone 2, right.
    pub sidebar: SidebarState,
    /// Zone 3, centre.
    pub canvas: CanvasViewState,
    /// Zone 4, bottom.
    pub tray: RecentTray,
    /// Zone 5, toggled over the canvas.
    pub library: LibraryState,
    /// Per-tool options that are not style (numbering, cut-out edge, ...).
    pub tools: ToolOptions,
    /// The Grab Text result the modal shows.
    #[serde(skip)]
    pub grab: Option<crate::views::ocr_modal::GrabResult>,
}

/// Editor tools: the default pinned ones in toolbar order, then the ones
/// behind More in menu order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolId {
    Arrow,
    Text,
    Callout,
    Shape,
    Stamp,
    Fill,
    #[default]
    Move,
    /// Marquee a region of the capture to move or delete it.
    Selection,
    Step,
    Blur,
    Pixelate,
    Highlighter,
    Pen,
    Eraser,
    Magnify,
    Spotlight,
    CutOut,
    SmartMove,
    Simplify,
    Crop,
    /// Drag over the capture to read its text (OCR).
    GrabText,
}

impl ToolId {
    pub const ALL: [ToolId; 21] = [
        ToolId::Arrow,
        ToolId::Text,
        ToolId::Callout,
        ToolId::Shape,
        ToolId::Stamp,
        ToolId::Fill,
        ToolId::Move,
        ToolId::Selection,
        ToolId::Step,
        ToolId::Blur,
        ToolId::Pixelate,
        ToolId::Highlighter,
        ToolId::Pen,
        ToolId::Eraser,
        ToolId::Magnify,
        ToolId::Spotlight,
        ToolId::CutOut,
        ToolId::SmartMove,
        ToolId::Simplify,
        ToolId::Crop,
        ToolId::GrabText,
    ];

    /// Pinned to the toolbar by default.
    pub const PINNED: [ToolId; 9] = [
        ToolId::Arrow,
        ToolId::Text,
        ToolId::Callout,
        ToolId::Shape,
        ToolId::Stamp,
        ToolId::Fill,
        ToolId::Move,
        ToolId::Selection,
        ToolId::GrabText,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ToolId::Move => "Move",
            ToolId::Arrow => "Arrow",
            ToolId::Text => "Text",
            ToolId::Callout => "Callout",
            ToolId::Shape => "Shape",
            ToolId::Stamp => "Stamp",
            ToolId::Selection => "Selection",
            ToolId::Step => "Step",
            ToolId::Fill => "Fill",
            ToolId::CutOut => "Cut Out",
            ToolId::Pen => "Pen",
            ToolId::Pixelate => "Pixelate",
            ToolId::Eraser => "Eraser",
            ToolId::Highlighter => "Highlight",
            ToolId::Blur => "Blur",
            ToolId::Magnify => "Magnify",
            ToolId::Spotlight => "Spotlight",
            ToolId::SmartMove => "Smart Move",
            ToolId::Simplify => "Simplify",
            ToolId::Crop => "Crop",
            ToolId::GrabText => "Grab Text",
        }
    }

    /// The engine command a finished gesture of this tool runs, for the
    /// tools wired so far.
    pub fn command(self) -> Option<&'static str> {
        match self {
            ToolId::Move => Some("annotation.move"),
            ToolId::Selection => Some("smart.moveObject"),
            ToolId::Arrow => Some("annotation.addArrow"),
            ToolId::Step => Some("annotation.addStep"),
            ToolId::Callout => Some("annotation.addCallout"),
            ToolId::Shape | ToolId::Highlighter => Some("annotation.addShape"),
            ToolId::Blur | ToolId::Pixelate => Some("annotation.addBlur"),
            ToolId::Text => Some("annotation.addText"),
            ToolId::Pen => Some("annotation.addPen"),
            ToolId::Eraser => Some("canvas.erase"),
            ToolId::Magnify => Some("annotation.addMagnifier"),
            ToolId::Spotlight => Some("annotation.addSpotlight"),
            ToolId::Crop => Some("canvas.crop"),
            ToolId::GrabText => Some("ocr.grabText"),
            ToolId::CutOut => Some("canvas.cutOut"),
            ToolId::Simplify => Some("smart.simplify"),
            _ => None,
        }
    }

    /// Tools whose Quick Styles grid the sidebar shows (Blur and
    /// Pixelate show strength cards instead).
    pub fn has_quick_styles(self) -> bool {
        matches!(
            self,
            ToolId::Arrow
                | ToolId::Text
                | ToolId::Callout
                | ToolId::Shape
                | ToolId::Step
                | ToolId::Pen
                | ToolId::Highlighter
                | ToolId::Magnify
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolbarState {
    pub active: ToolId,
    /// Shown as buttons, in order (customisable).
    pub pinned: Vec<ToolId>,
    /// Behind the "More" button.
    pub overflow: Vec<ToolId>,
    /// Tool to return to after a one-shot tool (Crop, Simplify).
    pub previous: Option<ToolId>,
}

impl Default for ToolbarState {
    fn default() -> Self {
        let pinned = ToolId::PINNED.to_vec();
        let overflow = ToolId::ALL
            .into_iter()
            .filter(|t| !pinned.contains(t))
            .collect();
        Self {
            active: ToolId::Arrow,
            pinned,
            overflow,
            previous: None,
        }
    }
}

impl ToolbarState {
    /// Every tool exactly once, pinned or behind More: drops repeats and
    /// adds tools a saved layout does not know yet (to More, in order).
    pub fn normalize(&mut self) {
        let mut seen = Vec::with_capacity(ToolId::ALL.len());
        self.pinned.retain(|t| {
            let new = !seen.contains(t);
            seen.push(*t);
            new
        });
        self.overflow.retain(|t| {
            let new = !seen.contains(t);
            seen.push(*t);
            new
        });
        for t in ToolId::ALL {
            if !seen.contains(&t) {
                self.overflow.push(t);
            }
        }
        // Keep More in menu order.
        self.overflow
            .sort_by_key(|t| ToolId::ALL.iter().position(|a| a == t));
    }

    /// Moves `tool` from More onto the toolbar (at the end).
    pub fn pin(&mut self, tool: ToolId) {
        if !self.pinned.contains(&tool) {
            self.overflow.retain(|t| *t != tool);
            self.pinned.push(tool);
        }
    }

    /// Moves `tool` from the toolbar back behind More.
    pub fn unpin(&mut self, tool: ToolId) {
        if self.pinned.contains(&tool) {
            self.pinned.retain(|t| *t != tool);
            self.overflow.push(tool);
            self.normalize();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarState {
    pub visible: bool,
    pub width_px: f32,
    /// Tool Properties or canvas Effects (the utility bar's tabs).
    pub mode: InspectorMode,
    pub quick_styles: QuickStyles,
    /// Expanded property sections below the Quick Styles grid.
    pub expanded: Vec<PropertySection>,
    /// `theme::ThemeKind` id (PhotoCraft's theme names).
    pub theme: String,
    /// Property Inspector edits not yet saved as a Quick Style; `None`
    /// shows the selected Quick Style.
    pub draft: Option<StyleDraft>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleDraft {
    pub tool: ToolId,
    pub preset: StylePreset,
}

impl Default for StyleDraft {
    fn default() -> Self {
        Self {
            tool: ToolId::Arrow,
            preset: StylePreset::default(),
        }
    }
}

impl SidebarState {
    /// The style the next `tool` annotation gets and the inspector shows.
    pub fn effective_style(&self, tool: ToolId) -> Option<StylePreset> {
        match &self.draft {
            Some(d) if d.tool == tool => Some(d.preset.clone()),
            _ => self.quick_styles.selected(tool).map(|q| q.preset.clone()),
        }
    }

    /// The draft differs from the selected Quick Style ("+ Add Quick Style").
    pub fn draft_is_new(&self, tool: ToolId) -> bool {
        match &self.draft {
            Some(d) if d.tool == tool => {
                self.quick_styles.selected(tool).map(|q| &q.preset) != Some(&d.preset)
            }
            _ => false,
        }
    }

    /// Edits the inspector's style for `tool`, starting a draft from the
    /// selected Quick Style when there is none yet.
    pub fn edit_draft(&mut self, tool: ToolId, f: impl FnOnce(&mut StylePreset)) {
        let mut preset = self.effective_style(tool).unwrap_or_default();
        f(&mut preset);
        self.draft = Some(StyleDraft { tool, preset });
    }

    /// Saves the draft as a user Quick Style and selects it.
    pub fn save_draft(&mut self, name: &str) -> bool {
        let Some(d) = self.draft.take() else {
            return false;
        };
        let tools = &mut self.quick_styles.tools;
        let i = match tools.iter().position(|t| t.tool == d.tool) {
            Some(i) => i,
            None => {
                tools.push(ToolStyles {
                    tool: d.tool,
                    styles: Vec::new(),
                    selected: None,
                });
                tools.len() - 1
            }
        };
        let Some(t) = tools.get_mut(i) else {
            return false;
        };
        let n = (0..)
            .find(|n| {
                let id = format!("user.{}.{n}", d.tool.label().to_lowercase());
                !t.styles.iter().any(|s| s.id == id)
            })
            .unwrap_or(0);
        t.styles.push(QuickStyle {
            id: format!("user.{}.{n}", d.tool.label().to_lowercase()),
            name: name.into(),
            builtin: false,
            hidden: false,
            favorite: false,
            preset: d.preset,
        });
        t.selected = Some(t.styles.len() - 1);
        true
    }
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            visible: true,
            width_px: 280.0,
            mode: InspectorMode::Properties,
            quick_styles: QuickStyles::default(),
            expanded: vec![PropertySection::Style, PropertySection::Shadow],
            theme: "studio".into(),
            draft: None,
        }
    }
}

/// What the right drawer shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectorMode {
    #[default]
    Properties,
    Effects,
    Layers,
    History,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertySection {
    /// Colour, fill, opacity.
    Style,
    Shadow,
    /// Thickness, dash, caps.
    Line,
    Arrowheads,
    Text,
    /// Corner radius, tail shape.
    Shape,
    /// Blur amount, pixel size, zoom.
    Effect,
}

/// Quick Styles grid: built-in and user presets per tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuickStyles {
    pub tools: Vec<ToolStyles>,
}

impl Default for QuickStyles {
    fn default() -> Self {
        let palette = [
            ("Red", Color::rgb(0xE5, 0x3E, 0x30)),
            ("Orange", Color::rgb(0xF2, 0x8C, 0x28)),
            ("Blue", Color::rgb(0x2D, 0x7F, 0xF9)),
            ("Black", Color::rgb(0x20, 0x20, 0x20)),
        ];
        // Highlighter pens rather than ink colours.
        let marker = [
            ("Yellow", Color::rgb(0xFF, 0xE6, 0x00)),
            ("Green", Color::rgb(0x5C, 0xE6, 0x5C)),
            ("Pink", Color::rgb(0xFF, 0x6E, 0xC7)),
            ("Blue", Color::rgb(0x4D, 0xC3, 0xFF)),
        ];
        let tools = ToolId::ALL
            .into_iter()
            .filter(|t| t.has_quick_styles())
            .map(|tool| ToolStyles {
                tool,
                styles: if tool == ToolId::Highlighter {
                    &marker
                } else {
                    &palette
                }
                .iter()
                .map(|(name, c)| QuickStyle::builtin(tool, name, *c))
                .collect(),
                selected: Some(0),
            })
            .collect();
        Self { tools }
    }
}

impl QuickStyles {
    pub fn for_tool(&self, tool: ToolId) -> Option<&ToolStyles> {
        self.tools.iter().find(|t| t.tool == tool)
    }

    /// Every favourite style as (tool, index in that tool's styles).
    pub fn favorites(&self) -> Vec<(ToolId, usize)> {
        self.tools
            .iter()
            .flat_map(|t| {
                t.styles
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.favorite && !s.hidden)
                    .map(|(i, _)| (t.tool, i))
            })
            .collect()
    }

    pub fn style(&self, tool: ToolId, index: usize) -> Option<&QuickStyle> {
        self.for_tool(tool)?.styles.get(index)
    }

    pub fn toggle_favorite(&mut self, tool: ToolId, index: usize) {
        if let Some(s) = self
            .tools
            .iter_mut()
            .find(|t| t.tool == tool)
            .and_then(|t| t.styles.get_mut(index))
        {
            s.favorite = !s.favorite;
        }
    }

    /// Makes style `index` the one the next `tool` annotation gets.
    pub fn select(&mut self, tool: ToolId, index: usize) {
        if let Some(t) = self.tools.iter_mut().find(|t| t.tool == tool)
            && index < t.styles.len()
        {
            t.selected = Some(index);
        }
    }

    /// The preset the next annotation of `tool` gets.
    pub fn selected(&self, tool: ToolId) -> Option<&QuickStyle> {
        let t = self.for_tool(tool)?;
        t.styles.get(t.selected?)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolStyles {
    pub tool: ToolId,
    pub styles: Vec<QuickStyle>,
    pub selected: Option<usize>,
}

impl Default for ToolStyles {
    fn default() -> Self {
        Self {
            tool: ToolId::Arrow,
            styles: Vec::new(),
            selected: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuickStyle {
    pub id: String,
    pub name: String,
    /// Built-ins cannot be deleted, only hidden.
    pub builtin: bool,
    pub hidden: bool,
    /// Listed under the toolbar's Favorites.
    pub favorite: bool,
    pub preset: StylePreset,
}

impl QuickStyle {
    fn builtin(tool: ToolId, name: &str, color: Color) -> Self {
        let stroke = Stroke {
            color,
            ..Stroke::default()
        };
        let preset = match tool {
            ToolId::Callout => StylePreset {
                stroke: Some(stroke),
                fill: Fill::Solid {
                    color: Color::WHITE,
                },
                shadow: Some(DropShadow::default()),
                text_color: Some(color),
                corner_radius: Some(12.0),
                ..StylePreset::default()
            },
            ToolId::Step => StylePreset {
                fill: Fill::Solid { color },
                text_color: Some(Color::WHITE),
                shadow: Some(DropShadow::default()),
                ..StylePreset::default()
            },
            ToolId::Text => StylePreset {
                text_color: Some(color),
                font: Some(FontSpec::default()),
                ..StylePreset::default()
            },
            ToolId::Arrow => StylePreset {
                stroke: Some(Stroke {
                    width: 6.0,
                    ..stroke
                }),
                shadow: Some(DropShadow::default()),
                arrow_end: Some(HeadStyle::Triangle),
                ..StylePreset::default()
            },
            ToolId::Highlighter => StylePreset {
                fill: Fill::Solid {
                    color: Color::rgba(color.r, color.g, color.b, 0x66),
                },
                ..StylePreset::default()
            },
            _ => StylePreset {
                stroke: Some(stroke),
                ..StylePreset::default()
            },
        };
        Self {
            id: format!(
                "builtin.{}.{}",
                tool.label().to_lowercase(),
                name.to_lowercase()
            ),
            name: name.into(),
            builtin: true,
            hidden: false,
            favorite: false,
            preset,
        }
    }
}

/// The style part of an annotation, as a Quick Style stores it. Fields a
/// tool does not use are `None`/ignored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StylePreset {
    pub stroke: Option<Stroke>,
    pub fill: Fill,
    pub shadow: Option<DropShadow>,
    pub opacity: f32,
    pub font: Option<FontSpec>,
    pub text_color: Option<Color>,
    pub arrow_start: Option<HeadStyle>,
    pub arrow_end: Option<HeadStyle>,
    pub corner_radius: Option<f32>,
}

impl StylePreset {
    /// The preset as `annotation.setStyle` fields: `None` fields are left
    /// out (keep), so applying a preset never clears what it does not set.
    pub fn style_params(&self) -> Map<String, Value> {
        fn val<T: Serialize>(v: &T) -> Option<Value> {
            serde_json::to_value(v).ok()
        }
        let fields = [
            ("stroke", self.stroke.as_ref().and_then(val)),
            (
                "fill",
                (!matches!(self.fill, Fill::None))
                    .then(|| val(&self.fill))
                    .flatten(),
            ),
            ("shadow", self.shadow.as_ref().and_then(val)),
            ("opacity", val(&self.opacity)),
            ("font", self.font.as_ref().and_then(val)),
            ("text_color", self.text_color.as_ref().and_then(val)),
            ("arrow_start", self.arrow_start.as_ref().and_then(val)),
            ("arrow_end", self.arrow_end.as_ref().and_then(val)),
            ("corner_radius", self.corner_radius.as_ref().and_then(val)),
        ];
        fields
            .into_iter()
            .filter_map(|(k, v)| Some((k.to_string(), v?)))
            .collect()
    }

    /// Solid fill colour, if any.
    pub fn fill_color(&self) -> Option<Color> {
        match self.fill {
            Fill::Solid { color } => Some(color),
            _ => None,
        }
    }
}

impl Default for StylePreset {
    fn default() -> Self {
        Self {
            stroke: None,
            fill: Fill::None,
            shadow: None,
            opacity: 1.0,
            font: None,
            text_color: None,
            arrow_start: None,
            arrow_end: None,
            corner_radius: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasViewState {
    pub zoom: Zoom,
    /// Scroll offset of the canvas inside the view, logical pixels.
    pub scroll: [f32; 2],
    /// Checkerboard behind transparent pixels.
    pub checkerboard: bool,
    pub selection: Vec<NodeId>,
    #[serde(skip)]
    pub hovered: Option<NodeId>,
    /// The Selection tool's marquee, canvas pixels.
    #[serde(skip)]
    pub marquee: Option<RectF>,
    pub resize: CanvasResize,
    /// What fills the new margin when the canvas grows.
    pub expand_backdrop: ExpandBackdrop,
    /// Pointer gesture in progress; never persisted.
    #[serde(skip)]
    pub gesture: Option<CanvasGesture>,
    /// The next canvas click samples a colour into the inspector.
    #[serde(skip)]
    pub eyedropper: Option<ColorTarget>,
    /// Copied annotations (`manifest.json` nodes) for Paste.
    #[serde(skip)]
    pub clipboard: Vec<serde_json::Value>,
    /// Pastes since the last copy: each lands a little further away.
    #[serde(skip)]
    pub pastes: u32,
    /// Text being typed on the canvas (Text tool, double-click a text).
    #[serde(skip)]
    pub text_edit: Option<TextEditState>,
    /// The Shape tool's polygon corners placed so far.
    #[serde(skip)]
    pub polygon: Vec<Point>,
    /// The Crop tool's pending frame, canvas pixels.
    #[serde(skip)]
    pub crop: Option<RectF>,
}

/// Inline text editing: a new text box (`node: None`) or an existing one.
#[derive(Clone, Debug, PartialEq)]
pub struct TextEditState {
    pub node: Option<NodeId>,
    /// Canvas pixels.
    pub rect: RectF,
    pub text: String,
    /// The text when editing began (unchanged text commits nothing).
    pub original: String,
}

/// What a Crop drag grabbed: a handle (`hx`, `hy` each -1, 0 or 1; 0 is
/// the middle), the inside (moves the frame) or the outside (draws a new
/// frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropHit {
    Handle(i8, i8),
    Inside,
    Outside,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpandBackdrop {
    #[default]
    Transparent,
    White,
    /// The capture's own edge colour.
    Auto,
}

impl ExpandBackdrop {
    pub const ALL: [ExpandBackdrop; 3] = [
        ExpandBackdrop::Transparent,
        ExpandBackdrop::White,
        ExpandBackdrop::Auto,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ExpandBackdrop::Transparent => "Transparent",
            ExpandBackdrop::White => "White",
            ExpandBackdrop::Auto => "Match edge",
        }
    }

    /// The `background` parameter of `canvas.expandBounds`.
    pub fn param(self) -> Value {
        match self {
            ExpandBackdrop::Transparent => json!({ "kind": "transparent" }),
            ExpandBackdrop::White => json!({ "kind": "solid", "color": Color::WHITE }),
            ExpandBackdrop::Auto => json!({ "kind": "auto" }),
        }
    }
}

/// Which inspector colour the eyedropper fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorTarget {
    Fill,
    Stroke,
    Text,
}

/// A drag on the canvas, in canvas pixels. The document changes once, by
/// one command, when the drag ends; until then the canvas draws a preview.
#[derive(Clone, Debug, PartialEq)]
pub enum CanvasGesture {
    Pan,
    /// A freehand drag of the Pen or Eraser: the pointer trace.
    Stroke {
        points: Vec<Point>,
    },
    /// Changing the Crop frame: `hit` says which part was grabbed.
    Crop {
        hit: CropHit,
        start: RectF,
        from: Point,
        to: Point,
    },
    /// Drawing a new annotation with the active tool.
    Draw {
        from: Point,
        to: Point,
    },
    MoveSelection {
        from: Point,
        to: Point,
    },
    /// Dragging out the Selection tool's marquee.
    Marquee {
        from: Point,
        to: Point,
    },
    /// Dragging the marquee's pixels (`smart.moveObject` on release).
    MoveMarquee {
        from: Point,
        to: Point,
    },
    CalloutTail {
        id: NodeId,
        index: usize,
        to: Point,
    },
    /// Dragging the "+" handle out of a callout adds a tail.
    NewTail {
        id: NodeId,
        to: Point,
    },
    ArrowBend {
        id: NodeId,
        to: Point,
    },
    /// Cut Out strip between `from` and `to` along `ToolOptions::cut_out.axis`.
    CutOut {
        from: f64,
        to: f64,
    },
    /// A gizmo resize handle; `local` is the pointer in the node's
    /// unrotated frame.
    Resize {
        id: NodeId,
        handle: ResizeHandle,
        local: Point,
    },
    /// The gizmo's rotation handle, canvas space.
    Rotate {
        id: NodeId,
        from: Point,
        to: Point,
    },
    /// The yellow corner-radius diamond.
    CornerRadius {
        id: NodeId,
        radius: f32,
    },
    /// A rounded shadow frame's margin handle on `side`; `delta` in canvas
    /// pixels since the drag began at margin `start`.
    FrameMargin {
        side: ResizeHandle,
        start: f32,
        delta: [f32; 2],
    },
    /// The rounded shadow frame's corner-radius diamond.
    FrameRadius {
        radius: f32,
    },
    /// A callout tail's neck diamond (its base width).
    CalloutNeck {
        id: NodeId,
        index: usize,
        width: f32,
    },
    /// A magnifier's source ring, moved apart from the lens.
    MagnifierSource {
        id: NodeId,
        to: Point,
    },
}

impl Default for CanvasViewState {
    fn default() -> Self {
        Self {
            zoom: Zoom::Fit,
            scroll: [0.0, 0.0],
            checkerboard: true,
            selection: Vec::new(),
            hovered: None,
            marquee: None,
            resize: CanvasResize::default(),
            expand_backdrop: ExpandBackdrop::default(),
            gesture: None,
            eyedropper: None,
            clipboard: Vec::new(),
            pastes: 0,
            text_edit: None,
            polygon: Vec::new(),
            crop: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Zoom {
    #[default]
    Fit,
    /// 100 = one image pixel per physical pixel.
    Percent { value: f32 },
}

/// The handles outside the canvas edge that grow or shrink the canvas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasResize {
    pub handles_visible: bool,
    /// Snap the new edge to the capture edge within this many pixels.
    pub snap_px: f32,
    /// In progress; never persisted.
    #[serde(skip)]
    pub drag: Option<ResizeDrag>,
}

impl Default for CanvasResize {
    fn default() -> Self {
        Self {
            handles_visible: true,
            snap_px: 6.0,
            drag: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeDrag {
    pub handle: ResizeHandle,
    /// Canvas size and base origin when the drag began.
    pub start_size: Size,
    pub start_origin: Offset,
    /// Pointer movement so far, image pixels.
    pub delta: [f32; 2],
}

impl ResizeDrag {
    /// Canvas size and base origin the drag would commit (`canvas.resize`).
    /// Dragging the left/top edge outward grows the canvas and moves the
    /// base right/down; the canvas never shrinks below 1 × 1.
    pub fn preview(&self) -> (Size, Offset) {
        let (dx, dy) = (round_px(self.delta[0]), round_px(self.delta[1]));
        let (left, right, top, bottom) = self.handle.edges();
        let mut w = i64::from(self.start_size.width);
        let mut h = i64::from(self.start_size.height);
        let mut o = self.start_origin;
        if right {
            w += i64::from(dx);
        }
        if bottom {
            h += i64::from(dy);
        }
        if left {
            let d = i64::from(dx).min(w - 1);
            w -= d;
            o.x = o.x.saturating_sub(d as i32);
        }
        if top {
            let d = i64::from(dy).min(h - 1);
            h -= d;
            o.y = o.y.saturating_sub(d as i32);
        }
        let clamp = |v: i64| v.clamp(1, i64::from(capturecraft_format::MAX_SIDE)) as u32;
        (Size::new(clamp(w), clamp(h)), o)
    }
}

fn round_px(v: f32) -> i32 {
    if v.is_finite() {
        v.round().clamp(-1e7, 1e7) as i32
    } else {
        0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeHandle {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

impl ResizeHandle {
    /// Which edges the handle moves: (left, right, top, bottom).
    pub fn edges(self) -> (bool, bool, bool, bool) {
        use ResizeHandle::*;
        (
            matches!(self, W | NW | SW),
            matches!(self, E | NE | SE),
            matches!(self, N | NE | NW),
            matches!(self, S | SE | SW),
        )
    }
}

/// The filmstrip of recent captures. Switching is instant: every item is
/// an open engine document, saved or not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecentTray {
    pub visible: bool,
    pub height_px: f32,
    pub items: Vec<TrayItem>,
    /// `TrayItem::doc_id` of the document on the canvas.
    pub active: Option<String>,
    pub scroll_px: f32,
    /// Items kept before the oldest saved ones drop off.
    pub max_items: usize,
    /// Ctrl/Shift multi-selection, for Combine Images.
    pub selected: Vec<String>,
    /// Where a Shift+Click range starts.
    #[serde(skip)]
    pub anchor: Option<String>,
}

impl Default for RecentTray {
    fn default() -> Self {
        Self {
            visible: true,
            height_px: 96.0,
            items: Vec::new(),
            active: None,
            scroll_px: 0.0,
            max_items: 50,
            selected: Vec::new(),
            anchor: None,
        }
    }
}

impl RecentTray {
    /// Unsaved items are never dropped, so the cap can be exceeded.
    pub fn push(&mut self, item: TrayItem) {
        self.active = Some(item.doc_id.clone());
        self.items.insert(0, item);
        while self.items.len() > self.max_items {
            match self.items.iter().rposition(|i| !i.unsaved) {
                Some(i) => {
                    let gone = self.items.remove(i);
                    self.selected.retain(|d| *d != gone.doc_id);
                }
                None => break,
            }
        }
    }

    /// Plain click: just this item. Ctrl/Cmd: toggle it. Shift: the range
    /// from the anchor.
    pub fn click(&mut self, doc_id: &str, toggle: bool, range: bool) {
        let pos = |id: &str| self.items.iter().position(|i| i.doc_id == id);
        if range && let (Some(a), Some(b)) = (self.anchor.as_deref().and_then(pos), pos(doc_id)) {
            let (lo, hi) = (a.min(b), a.max(b));
            self.selected = self
                .items
                .get(lo..=hi)
                .unwrap_or_default()
                .iter()
                .map(|i| i.doc_id.clone())
                .collect();
            return;
        }
        if toggle {
            if let Some(i) = self.selected.iter().position(|d| d == doc_id) {
                self.selected.remove(i);
            } else {
                self.selected.push(doc_id.into());
            }
        } else {
            self.selected = vec![doc_id.into()];
        }
        self.anchor = Some(doc_id.into());
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrayItem {
    /// `Manifest::id`.
    pub doc_id: String,
    pub title: String,
    pub kind: MediaKind,
    pub thumbnail_path: Option<String>,
    /// `.capturecraft` file, once saved.
    pub file_path: Option<String>,
    pub captured_at: String,
    pub unsaved: bool,
    /// `LibraryMeta::tags`, for the Library's filters.
    pub tags: Vec<String>,
    /// `SourceInfo::app_name` of the capture.
    pub app_name: String,
    pub favorite: bool,
    /// Smart Simplify replaced its text and detail.
    pub simplified: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    #[default]
    Image,
    /// Scrolling capture.
    Panorama,
    Video,
    Gif,
}

impl MediaKind {
    pub fn label(self) -> &'static str {
        match self {
            MediaKind::Image => "Image",
            MediaKind::Panorama => "Panorama",
            MediaKind::Video => "Video",
            MediaKind::Gif => "GIF",
        }
    }
}

/// Searchable grid of past captures (from each document's capture metadata).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryState {
    pub visible: bool,
    pub query: String,
    pub filters: LibraryFilters,
    pub sort: LibrarySort,
    pub view: LibraryView,
    pub thumb_size_px: f32,
    pub selected: Vec<String>,
}

impl Default for LibraryState {
    fn default() -> Self {
        Self {
            visible: false,
            query: String::new(),
            filters: LibraryFilters::default(),
            sort: LibrarySort::default(),
            view: LibraryView::Grid,
            thumb_size_px: 160.0,
            selected: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryFilters {
    pub date: DateFilter,
    /// `SourceInfo::app_name`.
    pub app: Option<String>,
    /// Host of `SourceInfo::url`.
    pub website: Option<String>,
    /// All of these tags.
    pub tags: Vec<String>,
    pub kind: Option<MediaKind>,
    pub favorites_only: bool,
    /// Only Smart-Simplified captures.
    pub simplified_only: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DateFilter {
    #[default]
    Any,
    Today,
    Last7Days,
    Last30Days,
    /// Inclusive `YYYY-MM-DD` bounds.
    Range {
        from: String,
        to: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibrarySort {
    pub field: SortField,
    pub descending: bool,
}

impl Default for LibrarySort {
    fn default() -> Self {
        Self {
            field: SortField::Date,
            descending: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortField {
    #[default]
    Date,
    Title,
    App,
    Website,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryView {
    #[default]
    Grid,
    List,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_old_files() {
        let mut s = AppState::default();
        s.hud.tab = CaptureTab::Video;
        s.hud.video.capture.selection = SelectionType::Scrolling {
            axis: Axis::Horizontal,
            mode: ScrollMode::Manual,
        };
        s.editor.library.filters.date = DateFilter::Range {
            from: "2026-10-01".into(),
            to: "2026-10-19".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<AppState>(&json).unwrap(), s);
        let old: AppState = serde_json::from_str(r#"{"hud":{"tab":"image"},"future":1}"#).unwrap();
        assert_eq!(old.hud.tab, CaptureTab::Image);
        assert_eq!(old.editor, EditorState::default());
    }

    #[test]
    fn toolbar_holds_every_tool_once() {
        let t = ToolbarState::default();
        let mut all: Vec<_> = t.pinned.iter().chain(&t.overflow).copied().collect();
        assert_eq!(all.len(), ToolId::ALL.len());
        all.dedup();
        assert_eq!(all.len(), ToolId::ALL.len());
    }

    #[test]
    fn tool_commands_exist_in_the_engine() {
        let ids: Vec<_> = capturecraft_engine::registry()
            .iter()
            .map(|c| c.id)
            .collect();
        for t in ToolId::ALL {
            if let Some(cmd) = t.command() {
                assert!(ids.contains(&cmd), "{cmd}");
            }
        }
    }

    #[test]
    fn quick_styles_cover_their_tools() {
        let q = QuickStyles::default();
        for t in ToolId::ALL.into_iter().filter(|t| t.has_quick_styles()) {
            assert!(q.selected(t).is_some(), "{t:?}");
        }
        assert!(q.selected(ToolId::Crop).is_none());
    }

    #[test]
    fn resize_preview_grows_outward_and_never_inverts() {
        let d = ResizeDrag {
            handle: ResizeHandle::NW,
            start_size: Size::new(100, 80),
            start_origin: Offset::default(),
            delta: [-20.0, -10.0],
        };
        assert_eq!(d.preview(), (Size::new(120, 90), Offset { x: 20, y: 10 }));
        let d = ResizeDrag {
            handle: ResizeHandle::E,
            delta: [-500.0, f32::NAN],
            ..d
        };
        assert_eq!(d.preview().0, Size::new(1, 80));
    }

    #[test]
    fn overlay_drag_rect_includes_both_corners() {
        let r = OverlayPhase::drag_rect([10, 20], [4, 5]);
        assert_eq!(r, Rect::new(4, 5, 11, 21));
        let p = OverlayPhase::Dragging {
            from: [0, 0],
            to: [0, 0],
        };
        assert_eq!(p.selection().map(|r| (r.width(), r.height())), Some((1, 1)));
        assert_eq!(OverlayPhase::Hovering.selection(), None);
    }

    #[test]
    fn style_params_leave_out_unset_fields() {
        let p = StylePreset {
            stroke: Some(Stroke::default()),
            ..StylePreset::default()
        }
        .style_params();
        assert!(p.contains_key("stroke") && p.contains_key("opacity"));
        assert!(!p.contains_key("fill") && !p.contains_key("shadow"));
        let p = CutOutOptions {
            torn: true,
            ..CutOutOptions::default()
        }
        .params(50.0, 20.0);
        assert_eq!(
            (p["start"].as_f64(), p["end"].as_f64()),
            (Some(20.0), Some(50.0))
        );
        assert_eq!(p["edge"], "torn");
    }

    #[test]
    fn drafts_become_quick_styles() {
        let mut s = SidebarState::default();
        assert!(!s.draft_is_new(ToolId::Arrow));
        let mut preset = s.effective_style(ToolId::Arrow).unwrap();
        preset.opacity = 0.5;
        s.draft = Some(StyleDraft {
            tool: ToolId::Arrow,
            preset: preset.clone(),
        });
        assert!(s.draft_is_new(ToolId::Arrow));
        assert!(!s.draft_is_new(ToolId::Step));
        assert!(s.save_draft("Faded"));
        assert_eq!(
            s.quick_styles.selected(ToolId::Arrow).unwrap().preset,
            preset
        );
        assert_eq!(
            s.quick_styles.selected(ToolId::Arrow).unwrap().id,
            "user.arrow.0"
        );
        assert!(s.draft.is_none() && !s.save_draft("again"));
    }

    #[test]
    fn tray_selection_by_click_toggle_and_range() {
        let mut t = RecentTray::default();
        for id in ["a", "b", "c", "d"] {
            t.push(TrayItem {
                doc_id: id.into(),
                ..TrayItem::default()
            });
        }
        // Newest first: d, c, b, a.
        t.click("c", false, false);
        t.click("a", false, true);
        assert_eq!(t.selected, ["c", "b", "a"]);
        t.click("b", true, false);
        assert_eq!(t.selected, ["c", "a"]);
        t.click("d", false, false);
        assert_eq!(t.selected, ["d"]);
    }

    #[test]
    fn tray_keeps_unsaved_items() {
        let mut t = RecentTray {
            max_items: 2,
            ..RecentTray::default()
        };
        for (id, unsaved) in [("a", true), ("b", false), ("c", true)] {
            t.push(TrayItem {
                doc_id: id.into(),
                unsaved,
                ..TrayItem::default()
            });
        }
        let ids: Vec<_> = t.items.iter().map(|i| i.doc_id.as_str()).collect();
        assert_eq!(ids, ["c", "a"]);
        assert_eq!(t.active.as_deref(), Some("c"));
    }
}
