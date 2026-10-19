//! CaptureCraft screen acquisition (L6).
//!
//! - [`CaptureBackend`]: displays, windows, the cursor as its own sprite,
//!   still grabs and frame sessions, implemented per platform in
//!   [`backends`].
//! - [`hit_test`]: the frozen desktop the crosshair overlay inspects
//!   (window/element hit testing, pixel loupe, region crops).
//! - [`panorama`]: the scrolling/panoramic stitcher writing straight into
//!   `photocraft-raster` 256 × 256 copy-on-write tiles.
//!
//! Coordinates: **global** points are the OS desktop coordinate space
//! (physical pixels on Windows and X11, points on macOS); each display maps
//! its global rect onto its frame's pixels, so mixed-DPI desktops work
//! without per-OS special cases. Pixels are straight-alpha RGBA8 in the
//! display's colour space ([`ColorInfo`]).
//!
//! Nothing here panics on OS or input errors: permission denials, portal
//! cancellation and lost display access are [`CaptureError`] variants.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod backends;
pub mod hit_test;
pub mod panorama;
mod tiles;

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use capturecraft_format::{
    CaptureMetadata, CaptureMode, ColorProfile, CursorKind, CursorLayer, Manifest, MonitorInfo,
    SourceInfo, TileData, TileEntry, tile_path,
};
use photocraft_color::PixelFormat;
use photocraft_geom::{Point, Rect, Size};
use photocraft_raster::{AllocationError, Surface};

pub use hit_test::{DisplayFrame, FrozenDesktop, HitTestOptions, Loupe, WindowIndex};
pub use panorama::{FrameOutcome, Panorama, PanoramaStitcher, ScrollAxis, StitchConfig};

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// macOS Screen Recording (TCC) or another OS-level refusal.
    #[error("screen capture permission denied: {hint}")]
    PermissionDenied { hint: String },
    /// The user dismissed the Wayland portal / OS picker.
    #[error("capture cancelled")]
    Cancelled,
    /// DXGI `ACCESS_LOST` (UAC prompt, lock screen, mode change); retry.
    #[error("display access lost ({0}); retry")]
    AccessLost(String),
    #[error("{backend} cannot {what}")]
    Unsupported { backend: &'static str, what: String },
    #[error("no display with id {0}")]
    NoSuchDisplay(u32),
    #[error("no window with id {0}")]
    NoSuchWindow(u64),
    #[error("invalid region: {0}")]
    InvalidRegion(String),
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    #[error("timed out waiting for a frame")]
    Timeout,
    #[error("out of memory: {0}")]
    Allocation(String),
    #[error("{backend}: {msg}")]
    Backend { backend: &'static str, msg: String },
}

impl CaptureError {
    /// Worth retrying after a short pause.
    pub fn is_retryable(&self) -> bool {
        matches!(self, CaptureError::AccessLost(_) | CaptureError::Timeout)
    }
}

impl From<AllocationError> for CaptureError {
    fn from(e: AllocationError) -> Self {
        CaptureError::Allocation(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, CaptureError>;

// ---------------------------------------------------------------------------
// Pixels

/// A tightly packed straight-alpha RGBA8 image; cheap to clone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaFrame {
    width: u32,
    height: u32,
    data: Arc<[u8]>,
}

impl RgbaFrame {
    /// Fails unless `data` is exactly `width × height × 4` bytes within the
    /// format's size limits.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        let len = frame_len(width, height)?;
        if data.len() != len {
            return Err(CaptureError::InvalidFrame(format!(
                "{width}×{height} needs {len} bytes, got {}",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            data: data.into(),
        })
    }

    /// From BGRA rows with a row stride (DXGI, ScreenCaptureKit, PipeWire).
    pub fn from_bgra(width: u32, height: u32, stride: usize, bgra: &[u8]) -> Result<Self> {
        let len = frame_len(width, height)?;
        let row = width as usize * 4;
        if stride < row {
            return Err(CaptureError::InvalidFrame(
                "stride shorter than a row".into(),
            ));
        }
        let mut out = Vec::new();
        out.try_reserve_exact(len)
            .map_err(|e| CaptureError::Allocation(e.to_string()))?;
        for y in 0..height as usize {
            let src = bgra.get(y * stride..y * stride + row).ok_or_else(|| {
                CaptureError::InvalidFrame("buffer shorter than height × stride".into())
            })?;
            for [b, g, r, a] in src.as_chunks::<4>().0 {
                out.extend_from_slice(&[*r, *g, *b, *a]);
            }
        }
        Self::new(width, height, out)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn pixel(&self, x: i64, y: i64) -> Option<[u8; 4]> {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = self.data.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }

    pub fn row(&self, y: u32) -> Option<&[u8]> {
        let w = self.width as usize * 4;
        let y = y as usize;
        self.data.get(y * w..(y + 1) * w)
    }

    /// The pixels of `r` (frame coordinates), clipped to the frame.
    pub fn crop(&self, r: Rect) -> Result<RgbaFrame> {
        let r = r.intersect(&Rect::from_xywh(0, 0, self.width, self.height));
        if r.is_empty() {
            return Err(CaptureError::InvalidRegion(
                "crop is outside the frame".into(),
            ));
        }
        let mut out = Vec::new();
        out.try_reserve_exact(frame_len(r.width(), r.height())?)
            .map_err(|e| CaptureError::Allocation(e.to_string()))?;
        for y in r.y0..r.y1 {
            let row = self.row(y as u32).unwrap_or_default();
            let s = row
                .get(r.x0 as usize * 4..r.x1 as usize * 4)
                .ok_or_else(|| CaptureError::InvalidRegion("crop is outside the frame".into()))?;
            out.extend_from_slice(s);
        }
        RgbaFrame::new(r.width(), r.height(), out)
    }
}

pub(crate) fn frame_len(width: u32, height: u32) -> Result<usize> {
    use capturecraft_format::{MAX_PIXELS, MAX_SIDE};
    let s = Size::new(width, height);
    if s.is_empty() || width > MAX_SIDE || height > MAX_SIDE || s.area() > MAX_PIXELS {
        return Err(CaptureError::InvalidFrame(format!(
            "{width}×{height} is empty or larger than {MAX_SIDE} px / {MAX_PIXELS} px²"
        )));
    }
    usize::try_from(s.area() * 4).map_err(|_| CaptureError::Allocation("frame too large".into()))
}

// ---------------------------------------------------------------------------
// Displays, windows, cursor

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PointI32 {
    pub x: i32,
    pub y: i32,
}

impl PointI32 {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub fn in_rect(self, r: &Rect) -> bool {
        r.contains(self.x, self.y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorSpace {
    #[default]
    Srgb,
    DisplayP3,
    /// BT.2020 primaries (HDR); frames arrive tone-mapped to 8 bit.
    Rec2020,
    Unknown,
}

/// Colour metadata of a display, preserved into the document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColorInfo {
    pub space: ColorSpace,
    /// The monitor's ICC profile, stored in the archive when present.
    pub icc_profile: Option<Arc<[u8]>>,
    pub hdr: bool,
    /// SDR reference white on an HDR display (Windows "SDR content brightness").
    pub sdr_white_nits: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DisplayInfo {
    pub id: u32,
    pub name: String,
    /// Global coordinates.
    pub rect: Rect,
    /// Fractional DPI scale (1.25, 1.5, 2.0 …).
    pub scale_factor: f32,
    pub primary: bool,
    pub rotation_deg: f32,
    pub refresh_hz: f32,
    pub color: ColorInfo,
}

/// Accessibility role of a sub-window region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiRole {
    #[default]
    Pane,
    Toolbar,
    Menu,
    MenuItem,
    Button,
    TextField,
    List,
    Tab,
    Other,
}

/// A control or pane inside a window (UI Automation, AX, AT-SPI).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiRegion {
    /// Global coordinates.
    pub rect: Rect,
    pub role: UiRole,
    pub name: String,
    /// Nesting depth below the window (1 = direct child).
    pub depth: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowMetadata {
    pub id: u64,
    pub pid: Option<u32>,
    pub app_name: String,
    pub process_name: String,
    pub title: String,
    /// Page URL when a browser exposes it.
    pub url: Option<String>,
    /// Visible frame in global coordinates, without the invisible resize
    /// border/shadow (`DWMWA_EXTENDED_FRAME_BOUNDS` on Windows).
    pub frame: Rect,
    /// 0 = frontmost.
    pub z_order: u32,
    pub display_id: Option<u32>,
    pub minimized: bool,
    pub focused: bool,
    pub elements: Vec<UiRegion>,
}

impl WindowMetadata {
    pub fn source_info(&self) -> SourceInfo {
        SourceInfo {
            app_name: self.app_name.clone(),
            process_name: self.process_name.clone(),
            window_title: self.title.clone(),
            url: self.url.clone(),
            window_rect: self.frame,
        }
    }
}

/// The OS cursor at capture time, never baked into the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct DetachedCursor {
    pub image: RgbaFrame,
    /// Hotspot inside `image`, in sprite pixels.
    pub hotspot: (i32, i32),
    /// Hotspot position, global coordinates.
    pub position: PointI32,
    pub kind: CursorKind,
    /// Sprite pixels per global unit (2 for a Retina cursor).
    pub scale_factor: f32,
    pub visible: bool,
}

impl DetachedCursor {
    /// The cursor layer for a capture of `rect` at `scale` frame pixels per
    /// global unit; `None` when the cursor is hidden or outside the capture.
    pub fn to_layer(&self, rect: Rect, scale: f32) -> Option<CursorLayer> {
        if !self.visible || !self.position.in_rect(&rect) || scale.is_nan() || scale <= 0.0 {
            return None;
        }
        let s = f64::from(scale);
        let sprite_scale = if self.scale_factor > 0.0 {
            scale / self.scale_factor
        } else {
            scale
        };
        Some(CursorLayer {
            visible: true,
            position: Point::new(
                f64::from(self.position.x - rect.x0) * s,
                f64::from(self.position.y - rect.y0) * s,
            ),
            hotspot: Point::new(f64::from(self.hotspot.0), f64::from(self.hotspot.1)),
            sprite_size: Size::new(self.image.width(), self.image.height()),
            kind: self.kind,
            scale: sprite_scale.clamp(0.1, 16.0),
            ..CursorLayer::default()
        })
    }
}

/// What the crosshair is over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HitKind {
    /// A control or pane inside a window.
    Element,
    Window,
    Display,
    /// Outside every display.
    #[default]
    Nothing,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowRef {
    pub id: u64,
    pub z_order: u32,
    pub pid: Option<u32>,
    pub app_name: String,
    pub title: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HitTarget {
    pub kind: HitKind,
    /// Tightest box under the point, global coordinates, clipped to the desktop.
    pub rect: Rect,
    pub window: Option<WindowRef>,
    pub element: Option<UiRegion>,
    pub display_id: Option<u32>,
}

// ---------------------------------------------------------------------------
// A finished still capture

/// Everything one capture produced, before it becomes a document.
#[derive(Clone, Debug)]
pub struct RawCapturePayload {
    pub frame: RgbaFrame,
    /// Global rect the frame covers.
    pub rect: Rect,
    /// Frame pixels per global unit.
    pub scale: f32,
    pub mode: CaptureMode,
    pub displays: Vec<DisplayInfo>,
    pub source_window: Option<WindowMetadata>,
    pub cursor: Option<DetachedCursor>,
    pub color: ColorInfo,
    pub captured_at: SystemTime,
}

/// A capture turned into a `.capturecraft` document, ready for the archive
/// writer: the manifest, the base tiles and the side files it refers to.
#[derive(Clone, Debug)]
pub struct CapturedDocument {
    pub manifest: Manifest,
    pub base: Surface,
    pub cursor_sprite: Option<RgbaFrame>,
    /// Bytes for `ColorProfile::Icc { path }`.
    pub icc_profile: Option<Arc<[u8]>>,
}

pub const ICC_PATH: &str = "profiles/display.icc";

impl RawCapturePayload {
    pub fn into_document(self) -> Result<CapturedDocument> {
        let size = Size::new(self.frame.width(), self.frame.height());
        let mut base = Surface::new(PixelFormat::RGBA8);
        tiles::write_frame(&mut base, &self.frame, 0, 0)?;
        let cursor_layer = self
            .cursor
            .as_ref()
            .and_then(|c| c.to_layer(self.rect, self.scale));
        let cursor_sprite = cursor_layer
            .as_ref()
            .and(self.cursor.as_ref())
            .map(|c| c.image.clone());
        let mut doc = document_from_surface(base, size, &self.color, self.scale)?;
        let m = &mut doc.manifest;
        m.cursor = cursor_layer;
        m.capture = CaptureMetadata {
            captured_at: rfc3339_utc(self.captured_at),
            mode: self.mode,
            region: self.rect,
            source: self.source_window.as_ref().map(WindowMetadata::source_info),
            monitors: self.displays.iter().map(monitor_info).collect(),
            os: std::env::consts::OS.into(),
            ..CaptureMetadata::default()
        };
        if let Some(w) = &self.source_window {
            m.title = w.title.clone();
        }
        m.validate()
            .map_err(|e| CaptureError::InvalidFrame(e.to_string()))?;
        doc.cursor_sprite = cursor_sprite;
        Ok(doc)
    }
}

fn monitor_info(d: &DisplayInfo) -> MonitorInfo {
    MonitorInfo {
        name: d.name.clone(),
        rect: d.rect,
        scale_factor: f64::from(d.scale_factor),
        primary: d.primary,
    }
}

/// Manifest + base raster for `surface` (content at `0,0 .. size`).
pub(crate) fn document_from_surface(
    base: Surface,
    size: Size,
    color: &ColorInfo,
    scale: f32,
) -> Result<CapturedDocument> {
    let mut m = Manifest::new(size);
    m.generator = concat!("capturecraft-capture ", env!("CARGO_PKG_VERSION")).into();
    m.base.scale_factor = f64::from(scale).clamp(0.25, 16.0);
    m.base.color_profile = match (&color.icc_profile, color.space) {
        (Some(_), _) => ColorProfile::Icc {
            path: ICC_PATH.into(),
        },
        (None, ColorSpace::DisplayP3) => ColorProfile::DisplayP3,
        _ => ColorProfile::Srgb,
    };
    let (gw, gh) = m.base.grid();
    m.base.tiles = base
        .tiles()
        .map(|(c, _)| *c)
        .filter(|c| c.tx >= 0 && c.ty >= 0 && c.tx < gw && c.ty < gh)
        .map(|coord| TileEntry {
            coord,
            data: TileData::File {
                path: tile_path("base", coord),
                sha256: None,
            },
        })
        .collect();
    Ok(CapturedDocument {
        manifest: m,
        base,
        cursor_sprite: None,
        icc_profile: color.icc_profile.clone(),
    })
}

/// `2026-10-19T12:03:11Z`.
pub fn rfc3339_utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

// ---------------------------------------------------------------------------
// Backends and sessions

/// What a backend can do on this machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub window_enumeration: bool,
    pub window_capture: bool,
    /// Cursor sprite + hotspot separate from the frame.
    pub detached_cursor: bool,
    /// Controls/panes inside windows (accessibility APIs).
    pub ui_elements: bool,
    /// Can send scroll events for auto-scroll capture.
    pub scroll_injection: bool,
    /// The OS shows its own picker/consent (Wayland portal).
    pub os_picker: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionTarget {
    Display(u32),
    Window(u64),
    /// Global coordinates.
    Region(Rect),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SessionOptions {
    pub fps: f32,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self { fps: 20.0 }
    }
}

/// One scroll step for auto-scroll capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollRequest {
    pub axis: ScrollAxis,
    /// Wheel notches; positive scrolls down/right.
    pub notches: i32,
    /// Where to deliver the wheel event (inside the target), global.
    pub at: PointI32,
    pub window: Option<u64>,
}

/// A platform capture implementation.
pub trait CaptureBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    fn displays(&self) -> Result<Vec<DisplayInfo>>;
    /// Visible top-level windows, frontmost first.
    fn windows(&self) -> Result<Vec<WindowMetadata>>;
    /// `Ok(None)` when the platform cannot provide the cursor separately.
    fn cursor(&self) -> Result<Option<DetachedCursor>>;
    /// One display's pixels, cursor excluded.
    fn grab_display(&self, id: u32) -> Result<RgbaFrame>;
    fn grab_window(&self, id: u64) -> Result<RgbaFrame>;

    /// A global region, composed from the displays it touches.
    fn grab_region(&self, rect: Rect) -> Result<(RgbaFrame, f32)> {
        let frames = self
            .displays()?
            .into_iter()
            .filter(|d| !d.rect.intersect(&rect).is_empty())
            .map(|d| {
                let frame = self.grab_display(d.id)?;
                Ok(DisplayFrame { info: d, frame })
            })
            .collect::<Result<Vec<_>>>()?;
        hit_test::compose(&frames, rect)
    }

    /// Freezes the desktop when the hotkey fires: every display, the window
    /// list and the cursor at one instant, for the crosshair to inspect.
    fn freeze(&self, opts: &HitTestOptions) -> Result<FrozenDesktop> {
        let captured_at = SystemTime::now();
        let cursor = self.cursor().unwrap_or(None);
        let displays = self
            .displays()?
            .into_iter()
            .map(|d| {
                let frame = self.grab_display(d.id)?;
                Ok(DisplayFrame { info: d, frame })
            })
            .collect::<Result<Vec<_>>>()?;
        // Window bounds are best effort: the capture still works without them.
        let windows = self.windows().unwrap_or_default();
        FrozenDesktop::new(displays, windows, cursor, captured_at, opts)
    }

    /// Continuous frames of a target (video, scrolling capture).
    fn start_session(
        &self,
        target: SessionTarget,
        opts: SessionOptions,
    ) -> Result<Box<dyn CaptureSession>>;

    fn scroll(&self, request: ScrollRequest) -> Result<()> {
        let _ = request;
        Err(CaptureError::Unsupported {
            backend: self.name(),
            what: "send scroll events".into(),
        })
    }
}

/// A running stream of frames.
pub trait CaptureSession: Send {
    /// Global rect of the frames.
    fn target_rect(&self) -> Rect;
    /// The next frame, or `Ok(None)` if none arrived within `timeout`.
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<RgbaFrame>>;
    fn stop(&mut self) -> Result<()>;
}

type GrabFn = Box<dyn FnMut() -> Result<RgbaFrame> + Send>;

/// A session for backends without a native stream: grabs at a fixed rate.
pub struct PollingSession {
    rect: Rect,
    grab: GrabFn,
    interval: Duration,
    last: Option<Instant>,
    stopped: bool,
}

impl PollingSession {
    pub fn new(rect: Rect, fps: f32, grab: GrabFn) -> Self {
        let fps = if fps.is_finite() {
            fps.clamp(0.5, 60.0)
        } else {
            20.0
        };
        Self {
            rect,
            grab,
            interval: Duration::from_secs_f32(1.0 / fps),
            last: None,
            stopped: false,
        }
    }
}

impl CaptureSession for PollingSession {
    fn target_rect(&self) -> Rect {
        self.rect
    }

    fn next_frame(&mut self, timeout: Duration) -> Result<Option<RgbaFrame>> {
        if self.stopped {
            return Err(CaptureError::Cancelled);
        }
        if let Some(last) = self.last {
            let wait = self.interval.saturating_sub(last.elapsed());
            if wait > timeout {
                std::thread::sleep(timeout);
                return Ok(None);
            }
            std::thread::sleep(wait);
        }
        self.last = Some(Instant::now());
        (self.grab)().map(Some)
    }

    fn stop(&mut self) -> Result<()> {
        self.stopped = true;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn frames_reject_bad_sizes() {
        assert!(RgbaFrame::new(2, 2, vec![0; 15]).is_err());
        assert!(RgbaFrame::new(0, 2, vec![]).is_err());
        assert!(RgbaFrame::from_bgra(2, 2, 7, &[0; 16]).is_err());
        let f = RgbaFrame::from_bgra(1, 2, 8, &[1, 2, 3, 4, 0, 0, 0, 0, 5, 6, 7, 8]).unwrap();
        assert_eq!(f.pixel(0, 1), Some([7, 6, 5, 8]));
        assert_eq!(f.pixel(1, 0), None);
    }

    #[test]
    fn utc_timestamps() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_411_391);
        assert_eq!(rfc3339_utc(t), "2026-10-19T12:03:11Z");
        assert_eq!(rfc3339_utc(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn cursor_layer_is_relative_to_the_capture() {
        let c = DetachedCursor {
            image: RgbaFrame::new(32, 32, vec![0; 32 * 32 * 4]).unwrap(),
            hotspot: (4, 2),
            position: PointI32::new(150, 120),
            kind: CursorKind::Arrow,
            scale_factor: 2.0,
            visible: true,
        };
        let l = c.to_layer(Rect::new(100, 100, 300, 300), 2.0).unwrap();
        assert_eq!((l.position.x, l.position.y), (100.0, 40.0));
        assert_eq!(l.scale, 1.0);
        assert!(c.to_layer(Rect::new(0, 0, 100, 100), 1.0).is_none());
    }

    #[test]
    fn payload_becomes_a_valid_document() {
        let frame = RgbaFrame::new(300, 20, vec![9; 300 * 20 * 4]).unwrap();
        let p = RawCapturePayload {
            frame,
            rect: Rect::new(0, 0, 300, 20),
            scale: 1.0,
            mode: CaptureMode::Region,
            displays: Vec::new(),
            source_window: None,
            cursor: None,
            color: ColorInfo {
                space: ColorSpace::DisplayP3,
                ..ColorInfo::default()
            },
            captured_at: SystemTime::UNIX_EPOCH,
        };
        let d = p.into_document().unwrap();
        assert_eq!(d.manifest.base.tiles.len(), 2);
        assert_eq!(d.manifest.base.color_profile, ColorProfile::DisplayP3);
        assert_eq!(
            d.base.to_interleaved(Rect::new(299, 19, 300, 20)),
            [9, 9, 9, 9]
        );
    }
}
