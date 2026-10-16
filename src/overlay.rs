//! Fullscreen "frozen screen" overlay for drag-to-select (like Snagit).
//!
//! The screen is captured first, then shown dimmed in a borderless fullscreen
//! window. Dragging reveals the selection at full brightness with a border.
//!
//! - Left drag: select a region
//! - Left click (no drag): select the whole screen
//! - Esc / right click / close: cancel

use crate::{Region, RgbaImage};
use anyhow::{Result, anyhow};
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Fullscreen, Window, WindowId, WindowLevel};

/// Brightness (out of 256) of the area outside the selection.
const DIM: u32 = 140;
/// Selection border colour, 0x00RRGGBB.
const BORDER: u32 = 0x003B_82F6;
const BORDER_PX: usize = 2;
/// Drags smaller than this (window px) count as a click.
const MIN_DRAG: u32 = 3;

/// Shows `frozen` fullscreen and lets the user select a rectangle.
/// Returns the selection in `frozen`'s pixel coordinates, or `None` if cancelled.
pub fn select_region(event_loop: EventLoop<()>, frozen: &RgbaImage) -> Result<Option<Region>> {
    let mut app = Overlay::new(frozen);
    event_loop.run_app(&mut app)?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(app.result),
    }
}

type WinSurface = Surface<Rc<Window>, Rc<Window>>;

struct Overlay<'a> {
    frozen: &'a RgbaImage,
    window: Option<Rc<Window>>,
    surface: Option<WinSurface>,
    /// Window size in physical pixels.
    size: (u32, u32),
    /// `frozen` scaled to window size, 0x00RRGGBB.
    bright: Vec<u32>,
    /// Dimmed copy of `bright`.
    dim: Vec<u32>,
    cursor: PhysicalPosition<f64>,
    drag_start: Option<PhysicalPosition<f64>>,
    result: Option<Region>,
    error: Option<anyhow::Error>,
}

impl<'a> Overlay<'a> {
    fn new(frozen: &'a RgbaImage) -> Self {
        Self {
            frozen,
            window: None,
            surface: None,
            size: (0, 0),
            bright: Vec::new(),
            dim: Vec::new(),
            cursor: PhysicalPosition::new(0.0, 0.0),
            drag_start: None,
            result: None,
            error: None,
        }
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let attrs = Window::default_attributes()
            .with_title("CaptureCraft")
            .with_decorations(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_fullscreen(Some(Fullscreen::Borderless(event_loop.primary_monitor())));
        let window = Rc::new(event_loop.create_window(attrs)?);
        window.set_cursor(CursorIcon::Crosshair);

        let context =
            Context::new(window.clone()).map_err(|e| anyhow!("softbuffer context: {e}"))?;
        let surface = Surface::new(&context, window.clone())
            .map_err(|e| anyhow!("softbuffer surface: {e}"))?;

        let size = window.inner_size();
        self.surface = Some(surface);
        self.window = Some(window);
        self.resize(size.width, size.height);
        self.request_redraw();
        Ok(())
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// Resizes the surface and rebuilds the scaled bright/dim buffers.
    fn resize(&mut self, w: u32, h: u32) {
        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else {
            return;
        };
        if let Some(surface) = &mut self.surface
            && let Err(e) = surface.resize(nw, nh)
        {
            eprintln!("overlay: resize failed: {e}");
            return;
        }
        self.size = (w, h);

        // Nearest-neighbour scale of the frozen image to the window size.
        let (iw, ih) = self.frozen.dimensions();
        let raw = self.frozen.as_raw();
        let mut bright = Vec::with_capacity(w as usize * h as usize);
        for y in 0..h {
            let sy = (y as u64 * ih as u64 / h as u64) as usize;
            for x in 0..w {
                let sx = (x as u64 * iw as u64 / w as u64) as usize;
                let i = (sy * iw as usize + sx) * 4;
                bright.push(rgb(raw[i], raw[i + 1], raw[i + 2]));
            }
        }
        self.dim = bright.iter().map(|&c| dim(c)).collect();
        self.bright = bright;
    }

    /// Current selection in window pixels.
    fn selection(&self) -> Option<Region> {
        let start = self.drag_start?;
        Region::from_drag(to_i32(start), to_i32(self.cursor)).clamp_to(self.size.0, self.size.1)
    }

    fn redraw(&mut self) {
        let selection = self.selection();
        let stride = self.size.0 as usize;
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let mut buf = match surface.buffer_mut() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("overlay: buffer failed: {e}");
                return;
            }
        };
        if buf.len() != self.dim.len() {
            return;
        }

        buf.copy_from_slice(&self.dim);
        if let Some(r) = selection {
            let (x0, x1) = (r.x as usize, (r.x + r.width) as usize);
            for y in r.y as usize..(r.y + r.height) as usize {
                let row = y * stride;
                buf[row + x0..row + x1].copy_from_slice(&self.bright[row + x0..row + x1]);
            }
            draw_border(&mut buf, stride, r);
        }
        if let Err(e) = buf.present() {
            eprintln!("overlay: present failed: {e}");
        }
    }

    /// Finishes the selection and converts it to image pixels.
    fn finish(&mut self, event_loop: &ActiveEventLoop) {
        let image_size = self.frozen.dimensions();
        let region = match self.selection() {
            Some(r) if r.width >= MIN_DRAG && r.height >= MIN_DRAG => {
                r.scale(self.size, image_size)
            }
            // Plain click: whole screen
            _ => Region::new(0, 0, image_size.0, image_size.1),
        };
        self.result = Some(region);
        event_loop.exit();
    }
}

impl ApplicationHandler for Overlay<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if let Err(e) = self.init(event_loop) {
            self.error = Some(e);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.resize(size.width, size.height);
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                if self.drag_start.is_some() {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.drag_start = Some(self.cursor),
                ElementState::Released if self.drag_start.is_some() => self.finish(event_loop),
                ElementState::Released => {}
            },
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            }
            | WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),
            _ => {}
        }
    }
}

fn to_i32(p: PhysicalPosition<f64>) -> (i32, i32) {
    (p.x.round() as i32, p.y.round() as i32)
}

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

fn dim(c: u32) -> u32 {
    let channel = |shift: u32| (((c >> shift) & 0xFF) * DIM / 256) << shift;
    channel(16) | channel(8) | channel(0)
}

fn draw_border(buf: &mut [u32], stride: usize, r: Region) {
    let (x0, y0) = (r.x as usize, r.y as usize);
    let (x1, y1) = (x0 + r.width as usize, y0 + r.height as usize);
    let t = BORDER_PX.min(r.width as usize).min(r.height as usize);
    for y in y0..y1 {
        let row = y * stride;
        if y < y0 + t || y >= y1 - t {
            buf[row + x0..row + x1].fill(BORDER);
        } else {
            buf[row + x0..row + x0 + t].fill(BORDER);
            buf[row + x1 - t..row + x1].fill(BORDER);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_halves_roughly() {
        assert_eq!(dim(0x00FF_FFFF), rgb(139, 139, 139));
        assert_eq!(dim(0), 0);
    }

    #[test]
    fn border_draws_outline_only() {
        let (w, h) = (10usize, 10usize);
        let mut buf = vec![0u32; w * h];
        draw_border(&mut buf, w, Region::new(2, 2, 6, 6));
        assert_eq!(buf[2 * w + 2], BORDER); // top-left corner
        assert_eq!(buf[7 * w + 7], BORDER); // bottom-right corner
        assert_eq!(buf[5 * w + 5], 0); // inside stays untouched
        assert_eq!(buf[0], 0); // outside stays untouched
    }
}
