//! Frozen-screen overlay for drag-to-select (like Snagit), multi-monitor.
//!
//! All monitors are captured first, then each is shown dimmed in its own
//! borderless fullscreen window. Selections can span monitors.
//!
//! In Region mode the overlay follows the cursor: while it moves, the window
//! underneath is highlighted and a click captures it; after a second without
//! movement it switches to pixel picking (magnifier) and a drag selects a
//! free region. Window mode always snaps.
//!
//! - Left drag: select a region (across monitors too) — Region mode only
//! - Left click (no drag): capture the highlighted window, or the monitor
//!   under the cursor if there is none
//! - Arrow keys: move the cursor 1 px (Shift: 10 px), also while dragging
//! - Enter (while dragging): finish
//! - Esc / right click: cancel

use crate::config::{Overlay, SelectMode};
use crate::draw::{Canvas, GLYPH_H, darken, rgb, text_width};
use crate::{Region, RgbaImage, crop};
use anyhow::{Context as _, Result, anyhow, bail};
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{CursorIcon, Fullscreen, Window, WindowId, WindowLevel};
use xcap::image::imageops;

const WHITE: u32 = 0x00FF_FFFF;
const LABEL_BG: u32 = 0x0020_2020;

/// Rectangle in global (virtual desktop) physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn from_points(a: (i32, i32), b: (i32, i32)) -> Self {
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        Self {
            x: x0,
            y: y0,
            w: x1.abs_diff(x0),
            h: y1.abs_diff(y0),
        }
    }

    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x) as i64;
        let y0 = self.y.max(other.y) as i64;
        let x1 = (self.x as i64 + self.w as i64).min(other.x as i64 + other.w as i64);
        let y1 = (self.y as i64 + self.h as i64).min(other.y as i64 + other.h as i64);
        (x1 > x0 && y1 > y0).then(|| Rect {
            x: x0 as i32,
            y: y0 as i32,
            w: (x1 - x0) as u32,
            h: (y1 - y0) as u32,
        })
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x0 = self.x.min(other.x) as i64;
        let y0 = self.y.min(other.y) as i64;
        let x1 = (self.x as i64 + self.w as i64).max(other.x as i64 + other.w as i64);
        let y1 = (self.y as i64 + self.h as i64).max(other.y as i64 + other.h as i64);
        Rect {
            x: x0 as i32,
            y: y0 as i32,
            w: (x1 - x0) as u32,
            h: (y1 - y0) as u32,
        }
    }

    pub fn contains(&self, p: (i32, i32)) -> bool {
        let (x, y) = (p.0 as i64, p.1 as i64);
        x >= self.x as i64
            && y >= self.y as i64
            && x < self.x as i64 + self.w as i64
            && y < self.y as i64 + self.h as i64
    }
}

/// Result of an overlay session.
pub enum Outcome {
    Cancelled,
    Selected(RgbaImage),
}

/// Runs the overlay once in its own event loop.
/// Returns the selected image, or `None` if cancelled.
pub fn select_interactive(mode: SelectMode, style: Overlay) -> Result<Option<RgbaImage>> {
    let event_loop = EventLoop::new()?;
    let mut app = OneShot {
        mode,
        style,
        ..Default::default()
    };
    event_loop.run_app(&mut app)?;
    if let Some(e) = app.error {
        return Err(e);
    }
    Ok(match app.outcome {
        Some(Outcome::Selected(img)) => Some(img),
        _ => None,
    })
}

#[derive(Default)]
struct OneShot {
    mode: SelectMode,
    style: Overlay,
    started: bool,
    session: Option<Session>,
    outcome: Option<Outcome>,
    error: Option<anyhow::Error>,
}

impl ApplicationHandler for OneShot {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.started {
            return;
        }
        self.started = true;
        match Session::start(event_loop, self.mode, self.style.clone()) {
            Ok(s) => self.session = Some(s),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let wake = self.session.as_mut().and_then(Session::tick);
        event_loop.set_control_flow(match wake {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let outcome = match &mut self.session {
            Some(s) => s.window_event(id, event),
            None => None,
        };
        if let Some(outcome) = outcome {
            self.session = None; // closes the windows
            self.outcome = Some(outcome);
            event_loop.exit();
        }
    }
}

/// One overlay: a frozen image + window per monitor and the shared drag state.
/// Dropping it closes all overlay windows.
pub struct Session {
    screens: Vec<Screen>,
    /// Cursor in global physical pixels.
    cursor: (i32, i32),
    drag_start: Option<(i32, i32)>,
    modifiers: Modifiers,
    mode: SelectMode,
    style: Overlay,
    /// Visible top-level windows (global physical px), topmost first.
    windows: Vec<Rect>,
    /// Window highlighted when the mouse button went down; a click (no drag)
    /// captures this, whatever the cursor has done since.
    press_target: Option<Rect>,
    /// When the cursor last moved (drives the idle switch in Region mode).
    last_move: Instant,
    /// Idle state the screens were last drawn with.
    idle_drawn: bool,
}

impl Session {
    /// Captures every monitor, then opens the overlay windows.
    pub fn start(event_loop: &ActiveEventLoop, mode: SelectMode, style: Overlay) -> Result<Self> {
        let monitors: Vec<MonitorHandle> = event_loop.available_monitors().collect();
        let single = monitors.len() == 1;
        // Window positions are taken at the same moment as the screenshots.
        let windows = list_windows(&monitors, style.min_window);

        // Capture everything before any overlay window is visible.
        let mut frozen = Vec::new();
        for monitor in monitors {
            match capture_monitor(&monitor, single) {
                Ok(img) => frozen.push((monitor, img)),
                Err(e) => eprintln!("skipping monitor {:?}: {e:#}", monitor.name()),
            }
        }
        if frozen.is_empty() {
            bail!("no monitor could be captured");
        }

        let cursor = match mode {
            SelectMode::Window => CursorIcon::Pointer,
            _ => CursorIcon::Crosshair,
        };
        let screens = frozen
            .into_iter()
            .map(|(monitor, img)| Screen::new(event_loop, monitor, img, cursor, &style))
            .collect::<Result<Vec<_>>>()?;
        for s in &screens {
            s.window.request_redraw();
        }
        screens[0].window.focus_window();

        Ok(Self {
            screens,
            cursor: (0, 0),
            drag_start: None,
            modifiers: Modifiers::default(),
            mode,
            style,
            windows,
            press_target: None,
            last_move: Instant::now(),
            idle_drawn: false,
        })
    }

    /// Handles an event for one of this session's windows.
    /// Returns `Some` when the session is finished.
    pub fn window_event(&mut self, id: WindowId, event: WindowEvent) -> Option<Outcome> {
        let idx = self.screens.iter().position(|s| s.window.id() == id)?;
        match event {
            WindowEvent::CloseRequested => return Some(Outcome::Cancelled),
            WindowEvent::Resized(size) => {
                self.screens[idx].resize(size);
                self.screens[idx].window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(idx),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m,
            WindowEvent::CursorMoved { position, .. } => {
                let r = self.screens[idx].rect;
                let moved = (
                    r.x + position.x.round() as i32,
                    r.y + position.y.round() as i32,
                );
                if moved != self.cursor {
                    self.cursor = moved;
                    self.last_move = Instant::now();
                    self.idle_drawn = false;
                }
                self.request_redraw_all();
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => {
                    self.drag_start = Some(self.cursor);
                    self.press_target = self.hovered_window();
                }
                ElementState::Released if self.drag_start.is_some() => {
                    return Some(self.finish());
                }
                ElementState::Released => {}
            },
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => return Some(Outcome::Cancelled),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => return self.key(logical_key),
            _ => {}
        }
        None
    }

    fn key(&mut self, key: Key) -> Option<Outcome> {
        let step = if self.modifiers.state().shift_key() {
            10
        } else {
            1
        };
        let (dx, dy) = match key {
            Key::Named(NamedKey::Escape) => return Some(Outcome::Cancelled),
            Key::Named(NamedKey::Enter)
                if self.drag_start.is_some() || self.mode == SelectMode::Window =>
            {
                return Some(self.finish());
            }
            Key::Named(NamedKey::ArrowLeft) => (-step, 0),
            Key::Named(NamedKey::ArrowRight) => (step, 0),
            Key::Named(NamedKey::ArrowUp) => (0, -step),
            Key::Named(NamedKey::ArrowDown) => (0, step),
            _ => return None,
        };
        let target = (self.cursor.0 + dx, self.cursor.1 + dy);
        if let Some(screen) = self.screens.iter().find(|s| s.rect.contains(target)) {
            self.cursor = target;
            self.last_move = Instant::now();
            self.idle_drawn = false;
            let local = PhysicalPosition::new(target.0 - screen.rect.x, target.1 - screen.rect.y);
            // Not supported on Wayland; the overlay still follows `self.cursor`.
            let _ = screen.window.set_cursor_position(local);
        }
        self.request_redraw_all();
        None
    }

    /// True once the cursor has been still long enough for pixel picking.
    fn is_idle(&self) -> bool {
        self.mode == SelectMode::Region && self.last_move.elapsed() >= self.style.zoom_delay()
    }

    /// Redraws when the idle state flips; returns when to wake up next.
    pub fn tick(&mut self) -> Option<Instant> {
        if self.mode != SelectMode::Region {
            return None;
        }
        let idle = self.is_idle();
        if idle != self.idle_drawn {
            self.idle_drawn = idle;
            self.request_redraw_all();
        }
        (!idle).then(|| self.last_move + self.style.zoom_delay())
    }

    fn request_redraw_all(&self) {
        for s in &self.screens {
            s.window.request_redraw();
        }
    }

    /// Dragged rectangle (Region mode only).
    fn selection(&self) -> Option<Rect> {
        if self.mode != SelectMode::Region {
            return None;
        }
        self.drag_start.map(|s| Rect::from_points(s, self.cursor))
    }

    fn is_dragging(&self) -> bool {
        let min = self.style.min_drag;
        self.selection().is_some_and(|r| r.w >= min && r.h >= min)
    }

    /// Window under the cursor, clipped to the screens.
    fn hovered_window(&self) -> Option<Rect> {
        self.windows
            .iter()
            .find(|w| w.contains(self.cursor))
            .and_then(|w| self.clip_to_screens(*w))
    }

    /// Bounding box of the parts of `r` that are on some screen.
    fn clip_to_screens(&self, r: Rect) -> Option<Rect> {
        self.screens
            .iter()
            .filter_map(|s| s.rect.intersect(&r))
            .reduce(|a, b| a.union(&b))
    }

    /// What is highlighted right now: the drag, else the hovered window.
    /// In Window mode an empty spot highlights the whole monitor, which is
    /// also what a click there captures.
    fn highlight(&self) -> Option<Rect> {
        if self.is_dragging() {
            return self.selection();
        }
        match (self.hovered_window(), self.mode) {
            (Some(w), _) => Some(w),
            (None, SelectMode::Window) => self.monitor_under_cursor(),
            (None, _) => None,
        }
    }

    fn monitor_under_cursor(&self) -> Option<Rect> {
        self.screens
            .iter()
            .find(|s| s.rect.contains(self.cursor))
            .map(|s| s.rect)
    }

    fn finish(&self) -> Outcome {
        let target = if self.is_dragging() {
            self.selection()
        } else {
            self.press_target.or_else(|| self.hovered_window())
        };
        match target {
            Some(rect) => match self.compose(rect) {
                Some(img) => Outcome::Selected(img),
                None => Outcome::Cancelled,
            },
            // No window under the cursor: whole monitor.
            None => {
                let screen = self
                    .screens
                    .iter()
                    .find(|s| s.rect.contains(self.cursor))
                    .unwrap_or(&self.screens[0]);
                Outcome::Selected(screen.frozen.clone())
            }
        }
    }

    /// Builds the output image for a global selection.
    /// A selection on a single monitor keeps that monitor's native resolution.
    fn compose(&self, sel: Rect) -> Option<RgbaImage> {
        let parts: Vec<(&Screen, Rect)> = self
            .screens
            .iter()
            .filter_map(|s| s.rect.intersect(&sel).map(|part| (s, part)))
            .collect();

        match parts.as_slice() {
            [] => None,
            [(screen, part)] => crop(&screen.frozen, screen.image_region(*part)),
            _ => {
                let mut out = RgbaImage::new(sel.w, sel.h);
                for (screen, part) in &parts {
                    let Some(mut piece) = crop(&screen.frozen, screen.image_region(*part)) else {
                        continue;
                    };
                    if piece.dimensions() != (part.w, part.h) {
                        piece = imageops::resize(
                            &piece,
                            part.w,
                            part.h,
                            imageops::FilterType::Triangle,
                        );
                    }
                    let (ox, oy) = ((part.x - sel.x) as i64, (part.y - sel.y) as i64);
                    imageops::replace(&mut out, &piece, ox, oy);
                }
                Some(out)
            }
        }
    }

    fn redraw(&mut self, idx: usize) {
        let sel = self.highlight().filter(|r| r.w > 0 && r.h > 0);
        let label = match sel {
            Some(r) => format!("{} × {}", r.w, r.h),
            None => format!("{}, {}", self.cursor.0, self.cursor.1),
        };
        // The magnifier belongs to free-hand picking: while dragging, or
        // once the cursor has been still long enough.
        let magnify =
            self.mode == SelectMode::Region && (self.is_idle() || self.drag_start.is_some());
        // With dim_on_drag, hovering leaves the screen bright and only the
        // frame marks the target; dragging dims everything outside it.
        let dim = !(self.style.dim_on_drag
            && self.mode == SelectMode::Region
            && self.drag_start.is_none());
        let cursor = self.cursor;
        self.screens[idx].draw(sel, cursor, &label, magnify, dim);
    }
}

/// One monitor: its frozen capture and overlay window.
struct Screen {
    /// Window position and size in global physical pixels.
    rect: Rect,
    frozen: RgbaImage,
    window: Rc<Window>,
    surface: Surface<Rc<Window>, Rc<Window>>,
    style: Overlay,
    /// `frozen` scaled to the window size, 0x00RRGGBB.
    bright: Vec<u32>,
    /// Dimmed copy of `bright`.
    dim: Vec<u32>,
}

impl Screen {
    fn new(
        event_loop: &ActiveEventLoop,
        monitor: MonitorHandle,
        frozen: RgbaImage,
        cursor: CursorIcon,
        style: &Overlay,
    ) -> Result<Self> {
        let pos = monitor.position();
        let size = monitor.size();
        let attrs = Window::default_attributes()
            .with_title("CaptureCraft")
            .with_decorations(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(pos)
            .with_fullscreen(Some(Fullscreen::Borderless(Some(monitor))));
        let window = Rc::new(event_loop.create_window(attrs)?);
        window.set_cursor(cursor);

        let context =
            Context::new(window.clone()).map_err(|e| anyhow!("softbuffer context: {e}"))?;
        let surface = Surface::new(&context, window.clone())
            .map_err(|e| anyhow!("softbuffer surface: {e}"))?;

        let mut screen = Self {
            rect: Rect {
                x: pos.x,
                y: pos.y,
                w: size.width,
                h: size.height,
            },
            frozen,
            window,
            surface,
            style: style.clone(),
            bright: Vec::new(),
            dim: Vec::new(),
        };
        let inner = screen.window.inner_size();
        screen.resize(inner);
        Ok(screen)
    }

    /// Resizes the surface and rebuilds the scaled bright/dim buffers.
    fn resize(&mut self, size: PhysicalSize<u32>) {
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if let Err(e) = self.surface.resize(w, h) {
            eprintln!("overlay: resize failed: {e}");
            return;
        }
        self.rect.w = size.width;
        self.rect.h = size.height;
        self.bright = scale_to_buffer(&self.frozen, size.width, size.height);
        let factor = self.style.dim_factor();
        self.dim = self.bright.iter().map(|&c| darken(c, factor)).collect();
    }

    /// Converts a global rect on this screen to frozen-image pixels.
    fn image_region(&self, part: Rect) -> Region {
        Region::new(
            (part.x - self.rect.x) as u32,
            (part.y - self.rect.y) as u32,
            part.w,
            part.h,
        )
        .scale((self.rect.w, self.rect.h), self.frozen.dimensions())
    }

    fn draw(
        &mut self,
        sel: Option<Rect>,
        cursor: (i32, i32),
        label: &str,
        magnify: bool,
        dim: bool,
    ) {
        let r = self.rect;
        let (w, h) = (r.w as usize, r.h as usize);
        let mut buf = match self.surface.buffer_mut() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("overlay: buffer failed: {e}");
                return;
            }
        };
        if buf.len() != self.dim.len() || self.dim.len() != w * h {
            return;
        }

        buf.copy_from_slice(if dim { &self.dim } else { &self.bright });

        // Selection at full brightness
        if dim && let Some(part) = sel.and_then(|s| s.intersect(&r)) {
            let x0 = (part.x - r.x) as usize;
            let x1 = x0 + part.w as usize;
            let y0 = (part.y - r.y) as usize;
            for y in y0..y0 + part.h as usize {
                let row = y * w;
                buf[row + x0..row + x1].copy_from_slice(&self.bright[row + x0..row + x1]);
            }
        }

        let mut canvas = Canvas {
            buf: &mut buf[..],
            width: w,
            height: h,
        };
        if let Some(s) = sel {
            let (x, y) = (s.x - r.x, s.y - r.y);
            canvas.stroke_rect(x, y, s.w, s.h, self.style.border_width, self.style.border());
        }
        if r.contains(cursor) {
            let local = (cursor.0 - r.x, cursor.1 - r.y);
            if magnify {
                draw_magnifier(&mut canvas, &self.bright, local, label, &self.style);
            } else if let Some(s) = sel {
                draw_label(&mut canvas, (s.x - r.x, s.y - r.y), s.h, label, &self.style);
            }
        }

        if let Err(e) = buf.present() {
            eprintln!("overlay: present failed: {e}");
        }
    }
}

/// Size badge just outside the top-left corner of a snapped rectangle
/// (inside it, if there is no room above).
fn draw_label(
    canvas: &mut Canvas<'_>,
    corner: (i32, i32),
    rect_height: u32,
    label: &str,
    style: &Overlay,
) {
    let w = text_width(label, style.label_scale) + 12;
    let h = GLYPH_H * style.label_scale + 8;
    let x = corner.0.max(0);
    let above = corner.1 - h as i32 - 4;
    let y = if above >= 0 {
        above
    } else {
        corner.1 + rect_height as i32 + 4
    };
    canvas.fill_rect(x, y, w, h, LABEL_BG);
    canvas.stroke_rect(x, y, w, h, 1, style.border());
    canvas.text(x + 6, y + 4, label, style.label_scale, WHITE);
}

/// Magnified view around the cursor plus a label (size or coordinates).
fn draw_magnifier(
    canvas: &mut Canvas<'_>,
    src: &[u32],
    cursor: (i32, i32),
    label: &str,
    style: &Overlay,
) {
    let (w, h) = (canvas.width as i32, canvas.height as i32);
    let (zoom, radius, offset) = (style.zoom, style.zoom_radius, style.zoom_offset);
    let side = (2 * radius + 1) as u32 * zoom;
    let label_w = text_width(label, style.label_scale) + 12;
    let label_h = GLYPH_H * style.label_scale + 8;
    let box_w = (side + 4).max(label_w) as i32;
    let box_h = (side + 4 + label_h + 2) as i32;

    // Bottom-right of the cursor, flipped near screen edges.
    let mut x = cursor.0 + offset;
    let mut y = cursor.1 + offset;
    if x + box_w > w {
        x = cursor.0 - offset - box_w;
    }
    if y + box_h > h {
        y = cursor.1 - offset - box_h;
    }

    // Zoomed pixels with a white frame
    canvas.fill_rect(x, y, side + 4, side + 4, WHITE);
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let (sx, sy) = (cursor.0 + dx, cursor.1 + dy);
            let color = if (0..w).contains(&sx) && (0..h).contains(&sy) {
                src[(sy * w + sx) as usize]
            } else {
                0
            };
            canvas.fill_rect(
                x + 2 + (dx + radius) * zoom as i32,
                y + 2 + (dy + radius) * zoom as i32,
                zoom,
                zoom,
                color,
            );
        }
    }
    let center = radius * zoom as i32 + 2;
    canvas.stroke_rect(x + center, y + center, zoom, zoom, 1, style.border());

    // Label
    let ly = y + side as i32 + 6;
    canvas.fill_rect(x, ly, label_w.max(side + 4), label_h, LABEL_BG);
    canvas.text(x + 6, ly + 4, label, style.label_scale, WHITE);
}

/// Nearest-neighbour scale of an RGBA image into a 0x00RRGGBB buffer.
fn scale_to_buffer(img: &RgbaImage, w: u32, h: u32) -> Vec<u32> {
    let (iw, ih) = img.dimensions();
    let raw = img.as_raw();
    let mut out = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h {
        let sy = (y as u64 * ih as u64 / h as u64) as usize;
        for x in 0..w {
            let sx = (x as u64 * iw as u64 / w as u64) as usize;
            let i = (sy * iw as usize + sx) * 4;
            out.push(rgb(raw[i], raw[i + 1], raw[i + 2]));
        }
    }
    out
}

/// Visible, non-minimized top-level windows in global physical pixels,
/// topmost first. Tiny windows (tooltips etc.) are skipped.
fn list_windows(monitors: &[MonitorHandle], min_size: u32) -> Vec<Rect> {
    // xcap reports points (logical) on macOS; elsewhere physical pixels.
    let scales: Vec<(Rect, f64)> = if cfg!(target_os = "macos") {
        monitors
            .iter()
            .map(|m| {
                let s = m.scale_factor();
                let (p, z) = (m.position(), m.size());
                let logical = Rect {
                    x: (p.x as f64 / s).round() as i32,
                    y: (p.y as f64 / s).round() as i32,
                    w: (z.width as f64 / s).round() as u32,
                    h: (z.height as f64 / s).round() as u32,
                };
                (logical, s)
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut windows: Vec<(i32, Rect)> = xcap::Window::all()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|w| {
            if w.is_minimized().unwrap_or(true) || is_own_window(&w) {
                return None;
            }
            let rect = Rect {
                x: w.x().ok()?,
                y: w.y().ok()?,
                w: w.width().ok()?,
                h: w.height().ok()?,
            };
            if rect.w < min_size || rect.h < min_size {
                return None;
            }
            Some((w.z().unwrap_or(0), to_physical(rect, &scales)))
        })
        .collect();
    windows.sort_by(|a, b| b.0.cmp(&a.0));
    windows.into_iter().map(|(_, r)| r).collect()
}

/// True for CaptureCraft's own windows: they must never be snap targets.
/// Matched by process, never by title — a user window may well be *called*
/// "capturecraft" (a terminal, an editor, a folder) and must still snap.
fn is_own_window(w: &xcap::Window) -> bool {
    if w.pid().is_ok_and(|pid| pid == std::process::id()) {
        return true;
    }
    let app = w.app_name().unwrap_or_default().to_ascii_lowercase();
    let app = app.strip_suffix(".exe").unwrap_or(&app);
    matches!(
        app,
        "capturecraft" | "capturecraft-app" | "capturecraft-tray"
    )
}

/// Prints the snap targets as the overlay sees them (for `--list-windows`).
pub fn debug_list_windows() -> Result<()> {
    let event_loop = EventLoop::new()?;
    let mut app = DebugList;
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct DebugList;

impl ApplicationHandler for DebugList {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(e) = print_windows(event_loop) {
            eprintln!("{e:#}");
        }
        event_loop.exit();
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

fn print_windows(event_loop: &ActiveEventLoop) -> Result<()> {
    let monitors: Vec<MonitorHandle> = event_loop.available_monitors().collect();
    for m in &monitors {
        let (p, s) = (m.position(), m.size());
        println!(
            "monitor {:?}: {},{} {}x{} scale {}",
            m.name().unwrap_or_default(),
            p.x,
            p.y,
            s.width,
            s.height,
            m.scale_factor()
        );
    }
    println!("--- snap targets (topmost first) ---");
    for (i, r) in list_windows(&monitors, Overlay::default().min_window)
        .iter()
        .enumerate()
    {
        println!("{i:>2}: {},{} {}x{}", r.x, r.y, r.w, r.h);
    }
    println!("--- raw xcap windows ---");
    for w in xcap::Window::all()?.iter() {
        println!(
            "z={:<4} {},{} {}x{} min={} own={} {:?} / {:?}",
            w.z().unwrap_or(-1),
            w.x().unwrap_or(0),
            w.y().unwrap_or(0),
            w.width().unwrap_or(0),
            w.height().unwrap_or(0),
            w.is_minimized().unwrap_or(false),
            is_own_window(w),
            w.app_name().unwrap_or_default(),
            w.title().unwrap_or_default()
        );
    }
    Ok(())
}

/// Converts a logical rect to physical using the scale of the monitor that
/// contains its top-left corner. Unchanged if no monitor matches.
fn to_physical(r: Rect, scales: &[(Rect, f64)]) -> Rect {
    match scales.iter().find(|(m, _)| m.contains((r.x, r.y))) {
        Some((_, s)) => Rect {
            x: (r.x as f64 * s).round() as i32,
            y: (r.y as f64 * s).round() as i32,
            w: (r.w as f64 * s).round() as u32,
            h: (r.h as f64 * s).round() as u32,
        },
        None => r,
    }
}

/// Captures the monitor behind a winit `MonitorHandle` with xcap.
fn capture_monitor(handle: &MonitorHandle, single: bool) -> Result<RgbaImage> {
    let pos = handle.position();
    let size = handle.size();
    let cx = pos.x + (size.width / 2) as i32;
    let cy = pos.y + (size.height / 2) as i32;
    // xcap uses points (logical) on macOS, winit reports physical pixels.
    #[cfg(target_os = "macos")]
    let (cx, cy) = {
        let s = handle.scale_factor();
        (
            (cx as f64 / s).round() as i32,
            (cy as f64 / s).round() as i32,
        )
    };
    match xcap::Monitor::from_point(cx, cy) {
        Ok(m) => m.capture_image().context("capture failed"),
        // e.g. Wayland, where positions are unreliable
        Err(_) if single => crate::capture_primary(),
        Err(e) => Err(e).with_context(|| format!("no capture monitor at {cx},{cy}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcap::image::Rgba;

    #[test]
    fn rect_from_points_any_direction() {
        let a = Rect::from_points((10, 20), (110, 220));
        assert_eq!(a, Rect::from_points((110, 220), (10, 20)));
        assert_eq!(
            a,
            Rect {
                x: 10,
                y: 20,
                w: 100,
                h: 200
            }
        );
    }

    #[test]
    fn rect_negative_coordinates() {
        // Monitor left of the primary has negative x.
        let r = Rect::from_points((-1920, 0), (100, 50));
        assert_eq!(
            r,
            Rect {
                x: -1920,
                y: 0,
                w: 2020,
                h: 50
            }
        );
    }

    #[test]
    fn rect_intersect() {
        let left = Rect {
            x: -1920,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let right = Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1200,
        };
        let sel = Rect {
            x: -100,
            y: 10,
            w: 300,
            h: 50,
        };
        assert_eq!(
            left.intersect(&sel),
            Some(Rect {
                x: -100,
                y: 10,
                w: 100,
                h: 50
            })
        );
        assert_eq!(
            right.intersect(&sel),
            Some(Rect {
                x: 0,
                y: 10,
                w: 200,
                h: 50
            })
        );
        assert_eq!(
            left.intersect(&Rect {
                x: 0,
                y: 0,
                w: 5,
                h: 5
            }),
            None
        );
    }

    #[test]
    fn rect_union() {
        let a = Rect {
            x: -10,
            y: 0,
            w: 10,
            h: 10,
        };
        let b = Rect {
            x: 5,
            y: 5,
            w: 10,
            h: 10,
        };
        assert_eq!(
            a.union(&b),
            Rect {
                x: -10,
                y: 0,
                w: 25,
                h: 15
            }
        );
    }

    #[test]
    fn to_physical_scales_on_matching_monitor() {
        let scales = [(
            Rect {
                x: 0,
                y: 0,
                w: 1440,
                h: 900,
            },
            2.0,
        )];
        let r = Rect {
            x: 10,
            y: 20,
            w: 100,
            h: 50,
        };
        assert_eq!(
            to_physical(r, &scales),
            Rect {
                x: 20,
                y: 40,
                w: 200,
                h: 100
            }
        );
        assert_eq!(to_physical(r, &[]), r);
    }

    #[test]
    fn rect_contains() {
        let r = Rect {
            x: -10,
            y: -10,
            w: 20,
            h: 20,
        };
        assert!(r.contains((-10, -10)));
        assert!(r.contains((9, 9)));
        assert!(!r.contains((10, 0)));
    }

    #[test]
    fn scale_to_buffer_identity_and_2x() {
        let img = RgbaImage::from_fn(2, 1, |x, _| Rgba([x as u8 * 255, 0, 0, 255]));
        assert_eq!(scale_to_buffer(&img, 2, 1), vec![0, 0x00FF_0000]);
        assert_eq!(
            scale_to_buffer(&img, 4, 1),
            vec![0, 0, 0x00FF_0000, 0x00FF_0000]
        );
    }

    #[test]
    fn magnifier_stays_on_canvas_near_edges() {
        let (w, h) = (400usize, 300usize);
        let src = vec![0x0012_3456u32; w * h];
        for cursor in [(0, 0), (399, 299), (399, 0), (0, 299), (200, 150)] {
            let mut buf = vec![0u32; w * h];
            let mut canvas = Canvas {
                buf: &mut buf,
                width: w,
                height: h,
            };
            draw_magnifier(&mut canvas, &src, cursor, "994 × 645", &Overlay::default());
            assert!(buf.contains(&WHITE), "nothing drawn for cursor {cursor:?}");
        }
    }
}
