//! Frozen-screen overlay for drag-to-select (like Snagit), multi-monitor.
//!
//! All monitors are captured first, then each is shown dimmed in its own
//! borderless fullscreen window. Selections can span monitors.
//!
//! - Left drag: select a region (across monitors too)
//! - Left click (no drag): capture the monitor under the cursor
//! - Arrow keys: move the cursor 1 px (Shift: 10 px), also while dragging
//! - Enter (while dragging): finish
//! - Esc / right click: cancel

use crate::draw::{Canvas, GLYPH_H, darken, rgb, text_width};
use crate::{Region, RgbaImage, crop};
use anyhow::{Context as _, Result, anyhow, bail};
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{CursorIcon, Fullscreen, Window, WindowId, WindowLevel};
use xcap::image::imageops;

/// Brightness (out of 256) of everything outside the selection.
const DIM: u32 = 140;
const BORDER: u32 = 0x003B_82F6;
const BORDER_PX: u32 = 2;
/// Drags smaller than this count as a click.
const MIN_DRAG: u32 = 3;
const ZOOM: u32 = 8;
/// Magnifier shows (2 * MAG_RADIUS + 1)^2 source pixels.
const MAG_RADIUS: i32 = 7;
const MAG_OFFSET: i32 = 24;
const LABEL_SCALE: u32 = 2;
const LABEL_BG: u32 = 0x0020_2020;
const WHITE: u32 = 0x00FF_FFFF;

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
pub fn select_interactive() -> Result<Option<RgbaImage>> {
    let event_loop = EventLoop::new()?;
    let mut app = OneShot::default();
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
        match Session::start(event_loop) {
            Ok(s) => self.session = Some(s),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
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
}

impl Session {
    /// Captures every monitor, then opens the overlay windows.
    pub fn start(event_loop: &ActiveEventLoop) -> Result<Self> {
        let monitors: Vec<MonitorHandle> = event_loop.available_monitors().collect();
        let single = monitors.len() == 1;

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

        let screens = frozen
            .into_iter()
            .map(|(monitor, img)| Screen::new(event_loop, monitor, img))
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
                self.cursor = (
                    r.x + position.x.round() as i32,
                    r.y + position.y.round() as i32,
                );
                self.request_redraw_all();
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.drag_start = Some(self.cursor),
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
            Key::Named(NamedKey::Enter) if self.drag_start.is_some() => {
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
            let local = PhysicalPosition::new(target.0 - screen.rect.x, target.1 - screen.rect.y);
            // Not supported on Wayland; the overlay still follows `self.cursor`.
            let _ = screen.window.set_cursor_position(local);
        }
        self.request_redraw_all();
        None
    }

    fn request_redraw_all(&self) {
        for s in &self.screens {
            s.window.request_redraw();
        }
    }

    fn selection(&self) -> Option<Rect> {
        self.drag_start.map(|s| Rect::from_points(s, self.cursor))
    }

    fn finish(&self) -> Outcome {
        match self.selection() {
            Some(sel) if sel.w >= MIN_DRAG && sel.h >= MIN_DRAG => match self.compose(sel) {
                Some(img) => Outcome::Selected(img),
                None => Outcome::Cancelled,
            },
            // Plain click: whole monitor under the cursor.
            _ => {
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
        let sel = self.selection().filter(|r| r.w > 0 && r.h > 0);
        let label = match sel {
            Some(r) => format!("{} × {}", r.w, r.h),
            None => format!("{}, {}", self.cursor.0, self.cursor.1),
        };
        let cursor = self.cursor;
        self.screens[idx].draw(sel, cursor, &label);
    }
}

/// One monitor: its frozen capture and overlay window.
struct Screen {
    /// Window position and size in global physical pixels.
    rect: Rect,
    frozen: RgbaImage,
    window: Rc<Window>,
    surface: Surface<Rc<Window>, Rc<Window>>,
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
        window.set_cursor(CursorIcon::Crosshair);

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
        self.dim = self.bright.iter().map(|&c| darken(c, DIM)).collect();
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

    fn draw(&mut self, sel: Option<Rect>, cursor: (i32, i32), label: &str) {
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

        buf.copy_from_slice(&self.dim);

        // Selection at full brightness
        if let Some(part) = sel.and_then(|s| s.intersect(&r)) {
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
            canvas.stroke_rect(s.x - r.x, s.y - r.y, s.w, s.h, BORDER_PX, BORDER);
        }
        if r.contains(cursor) {
            let local = (cursor.0 - r.x, cursor.1 - r.y);
            draw_magnifier(&mut canvas, &self.bright, local, label);
        }

        if let Err(e) = buf.present() {
            eprintln!("overlay: present failed: {e}");
        }
    }
}

/// Magnified view around the cursor plus a label (size or coordinates).
fn draw_magnifier(canvas: &mut Canvas<'_>, src: &[u32], cursor: (i32, i32), label: &str) {
    let (w, h) = (canvas.width as i32, canvas.height as i32);
    let side = (2 * MAG_RADIUS + 1) as u32 * ZOOM;
    let label_w = text_width(label, LABEL_SCALE) + 12;
    let label_h = GLYPH_H * LABEL_SCALE + 8;
    let box_w = (side + 4).max(label_w) as i32;
    let box_h = (side + 4 + label_h + 2) as i32;

    // Bottom-right of the cursor, flipped near screen edges.
    let mut x = cursor.0 + MAG_OFFSET;
    let mut y = cursor.1 + MAG_OFFSET;
    if x + box_w > w {
        x = cursor.0 - MAG_OFFSET - box_w;
    }
    if y + box_h > h {
        y = cursor.1 - MAG_OFFSET - box_h;
    }

    // Zoomed pixels with a white frame
    canvas.fill_rect(x, y, side + 4, side + 4, WHITE);
    for dy in -MAG_RADIUS..=MAG_RADIUS {
        for dx in -MAG_RADIUS..=MAG_RADIUS {
            let (sx, sy) = (cursor.0 + dx, cursor.1 + dy);
            let color = if (0..w).contains(&sx) && (0..h).contains(&sy) {
                src[(sy * w + sx) as usize]
            } else {
                0
            };
            canvas.fill_rect(
                x + 2 + (dx + MAG_RADIUS) * ZOOM as i32,
                y + 2 + (dy + MAG_RADIUS) * ZOOM as i32,
                ZOOM,
                ZOOM,
                color,
            );
        }
    }
    let center = MAG_RADIUS * ZOOM as i32 + 2;
    canvas.stroke_rect(x + center, y + center, ZOOM, ZOOM, 1, BORDER);

    // Label
    let ly = y + side as i32 + 6;
    canvas.fill_rect(x, ly, label_w.max(side + 4), label_h, LABEL_BG);
    canvas.text(x + 6, ly + 4, label, LABEL_SCALE, WHITE);
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
            draw_magnifier(&mut canvas, &src, cursor, "994 × 645");
            assert!(buf.contains(&WHITE), "nothing drawn for cursor {cursor:?}");
        }
    }
}
