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

use capturecraft_format::{
    Axis, Color, DropShadow, Fill, FontSpec, HeadStyle, NodeId, Offset, Stroke,
};
use photocraft_geom::Size;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    pub hud: CaptureHud,
    pub crosshair: CrosshairPrefs,
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
    /// Integer zoom of the loupe (nearest neighbour, pixel grid).
    pub loupe_zoom: u8,
    pub loupe_size_px: f32,
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
            loupe_zoom: 8,
            loupe_size_px: 120.0,
            color_code: ColorCode::Hex,
            show_dimensions: true,
            snap_to_windows: true,
            crosshair_lines: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorCode {
    #[default]
    Hex,
    Rgb,
}

// ---------------------------------------------------------------------------
// Editor

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
}

/// Editor tools, in toolbar order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolId {
    #[default]
    Move,
    Arrow,
    Text,
    Callout,
    Shape,
    Stamp,
    Step,
    Fill,
    CutOut,
    Pen,
    Highlighter,
    Blur,
    Magnify,
    SmartMove,
    Simplify,
    Crop,
}

impl ToolId {
    pub const ALL: [ToolId; 16] = [
        ToolId::Move,
        ToolId::Arrow,
        ToolId::Text,
        ToolId::Callout,
        ToolId::Shape,
        ToolId::Stamp,
        ToolId::Step,
        ToolId::Fill,
        ToolId::CutOut,
        ToolId::Pen,
        ToolId::Highlighter,
        ToolId::Blur,
        ToolId::Magnify,
        ToolId::SmartMove,
        ToolId::Simplify,
        ToolId::Crop,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ToolId::Move => "Move",
            ToolId::Arrow => "Arrow",
            ToolId::Text => "Text",
            ToolId::Callout => "Callout",
            ToolId::Shape => "Shape",
            ToolId::Stamp => "Stamp",
            ToolId::Step => "Step",
            ToolId::Fill => "Fill",
            ToolId::CutOut => "Cut Out",
            ToolId::Pen => "Pen",
            ToolId::Highlighter => "Highlighter",
            ToolId::Blur => "Blur",
            ToolId::Magnify => "Magnify",
            ToolId::SmartMove => "Smart Move",
            ToolId::Simplify => "Simplify",
            ToolId::Crop => "Crop",
        }
    }

    /// The engine command a finished gesture of this tool runs, for the
    /// tools wired so far.
    pub fn command(self) -> Option<&'static str> {
        match self {
            ToolId::Step => Some("annotation.addStep"),
            ToolId::Callout => Some("annotation.addCallout"),
            ToolId::CutOut => Some("canvas.cutOut"),
            ToolId::Simplify => Some("smart.simplify"),
            _ => None,
        }
    }

    /// Tools whose Quick Styles grid the sidebar shows.
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
                | ToolId::Blur
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
        let pinned = vec![
            ToolId::Move,
            ToolId::Arrow,
            ToolId::Text,
            ToolId::Callout,
            ToolId::Shape,
            ToolId::Step,
            ToolId::Blur,
            ToolId::Magnify,
            ToolId::CutOut,
            ToolId::Crop,
        ];
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarState {
    pub visible: bool,
    pub width_px: f32,
    pub quick_styles: QuickStyles,
    /// Expanded property sections below the Quick Styles grid.
    pub expanded: Vec<PropertySection>,
    /// `theme::ThemeKind` name (PhotoCraft themes).
    pub theme: String,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            visible: true,
            width_px: 280.0,
            quick_styles: QuickStyles::default(),
            expanded: vec![PropertySection::Style, PropertySection::Shadow],
            theme: "dark".into(),
        }
    }
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
        let tools = ToolId::ALL
            .into_iter()
            .filter(|t| t.has_quick_styles())
            .map(|tool| ToolStyles {
                tool,
                styles: palette
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
    pub resize: CanvasResize,
}

impl Default for CanvasViewState {
    fn default() -> Self {
        Self {
            zoom: Zoom::Fit,
            scroll: [0.0, 0.0],
            checkerboard: true,
            selection: Vec::new(),
            hovered: None,
            resize: CanvasResize::default(),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
                    self.items.remove(i);
                }
                None => break,
            }
        }
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
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    #[default]
    Image,
    Video,
    Gif,
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
