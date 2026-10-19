//! What the crosshair overlay inspects: a desktop frozen at hotkey time.
//!
//! [`FrozenDesktop`] holds every display's frame, the window list and the
//! cursor from one instant, so moving the crosshair never changes what is
//! under it (menus and tooltips stay open in the snapshot even after the
//! real ones close). [`WindowIndex`] answers "what is under this point"
//! in O(windows in one grid cell): windows are bucketed into a uniform grid
//! over the desktop, each bucket keeps its windows frontmost first, and the
//! first window containing the point wins; inside it, the smallest
//! accessibility element containing the point is the tightest target.

use std::time::SystemTime;

use capturecraft_format::CaptureMode;
use photocraft_geom::Rect;
use rayon::prelude::*;

use crate::{
    CaptureError, ColorInfo, DetachedCursor, DisplayInfo, HitKind, HitTarget, PointI32,
    RawCapturePayload, Result, RgbaFrame, UiRegion, WindowMetadata, WindowRef,
};

/// One display and its pixels.
#[derive(Clone, Debug)]
pub struct DisplayFrame {
    pub info: DisplayInfo,
    pub frame: RgbaFrame,
}

impl DisplayFrame {
    /// Frame pixel under global `(x, y)` (floor of the scaled position).
    fn native(&self, x: f64, y: f64) -> Option<(i64, i64)> {
        let r = self.info.rect;
        if r.is_empty() {
            return None;
        }
        let sx = f64::from(self.frame.width()) / f64::from(r.width());
        let sy = f64::from(self.frame.height()) / f64::from(r.height());
        let nx = ((x - f64::from(r.x0)) * sx).floor();
        let ny = ((y - f64::from(r.y0)) * sy).floor();
        (nx.is_finite() && ny.is_finite()).then_some((nx as i64, ny as i64))
    }

    /// Frame pixels per global unit (the larger axis, for rotated panels).
    fn scale(&self) -> f64 {
        let r = self.info.rect;
        if r.is_empty() {
            return 1.0;
        }
        (f64::from(self.frame.width()) / f64::from(r.width()))
            .max(f64::from(self.frame.height()) / f64::from(r.height()))
    }
}

#[derive(Clone, Debug)]
pub struct HitTestOptions {
    /// Our own overlay/HUD windows.
    pub exclude_pids: Vec<u32>,
    /// Windows smaller than this (either side) are ignored.
    pub min_size: u32,
    /// Descend into accessibility elements.
    pub elements: bool,
}

impl Default for HitTestOptions {
    fn default() -> Self {
        Self {
            exclude_pids: vec![std::process::id()],
            min_size: 8,
            elements: true,
        }
    }
}

/// Upper bound on grid buckets, whatever the desktop size.
const MAX_CELLS: i64 = 1 << 16;

/// Z-ordered spatial index of windows over the desktop.
#[derive(Clone, Debug)]
pub struct WindowIndex {
    windows: Vec<WindowMetadata>,
    displays: Vec<(u32, Rect)>,
    desktop: Rect,
    cell: i32,
    cols: i32,
    rows: i32,
    /// Per cell: indices into `windows`, frontmost first.
    grid: Vec<Vec<u32>>,
    elements: bool,
}

impl WindowIndex {
    pub fn build(
        displays: &[DisplayInfo],
        windows: Vec<WindowMetadata>,
        opts: &HitTestOptions,
    ) -> Self {
        let desktop = displays
            .iter()
            .map(|d| d.rect)
            .reduce(|a, b| a.union(&b))
            .unwrap_or_default();
        let mut windows: Vec<WindowMetadata> = windows
            .into_iter()
            .filter(|w| !w.minimized)
            .filter(|w| !w.pid.is_some_and(|p| opts.exclude_pids.contains(&p)))
            .filter_map(|mut w| {
                w.frame = w.frame.intersect(&desktop);
                let ok = w.frame.width() >= opts.min_size && w.frame.height() >= opts.min_size;
                ok.then_some(w)
            })
            .collect();
        // Stable: ties keep the OS enumeration order.
        windows.sort_by_key(|w| w.z_order);

        let area = i64::from(desktop.width()) * i64::from(desktop.height());
        let mut cell = 256i64;
        while area / (cell * cell) > MAX_CELLS {
            cell *= 2;
        }
        let cell = cell as i32;
        let cols = (i64::from(desktop.width()).div_euclid(i64::from(cell)) + 1) as i32;
        let rows = (i64::from(desktop.height()).div_euclid(i64::from(cell)) + 1) as i32;
        let mut grid = vec![Vec::new(); (cols * rows) as usize];
        for (i, w) in windows.iter().enumerate() {
            let (c0, r0) = cell_of(desktop, cell, w.frame.x0, w.frame.y0);
            let (c1, r1) = cell_of(desktop, cell, w.frame.x1 - 1, w.frame.y1 - 1);
            for r in r0.max(0)..=r1.min(rows - 1) {
                for c in c0.max(0)..=c1.min(cols - 1) {
                    if let Some(b) = grid.get_mut((r * cols + c) as usize) {
                        b.push(i as u32);
                    }
                }
            }
        }
        Self {
            windows,
            displays: displays.iter().map(|d| (d.id, d.rect)).collect(),
            desktop,
            cell,
            cols,
            rows,
            grid,
            elements: opts.elements,
        }
    }

    pub fn windows(&self) -> &[WindowMetadata] {
        &self.windows
    }

    pub fn desktop(&self) -> Rect {
        self.desktop
    }

    fn display_at(&self, p: PointI32) -> Option<(u32, Rect)> {
        self.displays.iter().copied().find(|(_, r)| p.in_rect(r))
    }

    /// Frontmost window containing `p`.
    pub fn window_at(&self, p: PointI32) -> Option<&WindowMetadata> {
        if !p.in_rect(&self.desktop) {
            return None;
        }
        let (c, r) = cell_of(self.desktop, self.cell, p.x, p.y);
        if c < 0 || r < 0 || c >= self.cols || r >= self.rows {
            return None;
        }
        self.grid
            .get((r * self.cols + c) as usize)?
            .iter()
            .filter_map(|&i| self.windows.get(i as usize))
            .find(|w| p.in_rect(&w.frame))
    }

    /// The tightest target under `p`: element, else window, else display.
    pub fn hit_test(&self, p: PointI32) -> HitTarget {
        self.hit_stack(p).into_iter().next().unwrap_or_default()
    }

    /// Every target under `p`, tightest first (element, window, display),
    /// so the overlay can widen the selection step by step.
    pub fn hit_stack(&self, p: PointI32) -> Vec<HitTarget> {
        let Some((display_id, drect)) = self.display_at(p) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(3);
        if let Some(w) = self.window_at(p) {
            let wref = WindowRef {
                id: w.id,
                z_order: w.z_order,
                pid: w.pid,
                app_name: w.app_name.clone(),
                title: w.title.clone(),
            };
            if self.elements
                && let Some(e) = tightest_element(w, p)
            {
                out.push(HitTarget {
                    kind: HitKind::Element,
                    rect: e.rect.intersect(&w.frame),
                    window: Some(wref.clone()),
                    element: Some(e.clone()),
                    display_id: Some(display_id),
                });
            }
            out.push(HitTarget {
                kind: HitKind::Window,
                rect: w.frame,
                window: Some(wref),
                element: None,
                display_id: Some(display_id),
            });
        }
        out.push(HitTarget {
            kind: HitKind::Display,
            rect: drect,
            window: None,
            element: None,
            display_id: Some(display_id),
        });
        out
    }
}

fn cell_of(desktop: Rect, cell: i32, x: i32, y: i32) -> (i32, i32) {
    (
        (x.saturating_sub(desktop.x0)).div_euclid(cell),
        (y.saturating_sub(desktop.y0)).div_euclid(cell),
    )
}

/// Smallest element containing `p` (deeper wins ties), clipped to the window.
fn tightest_element(w: &WindowMetadata, p: PointI32) -> Option<&UiRegion> {
    w.elements
        .iter()
        .filter(|e| p.in_rect(&e.rect.intersect(&w.frame)))
        .min_by_key(|e| (e.rect.size().area(), std::cmp::Reverse(e.depth)))
}

/// The pixel loupe beside the crosshair, in native display pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Loupe {
    /// Edge length in pixels (`2 × radius + 1`); the centre is the hovered pixel.
    pub size: u32,
    /// Row-major; transparent outside the display.
    pub pixels: Vec<[u8; 4]>,
    pub center: Option<[u8; 4]>,
    pub display_id: Option<u32>,
}

impl Loupe {
    /// `#RRGGBB` of the centre pixel.
    pub fn hex(&self) -> Option<String> {
        self.center
            .map(|[r, g, b, _]| format!("#{r:02X}{g:02X}{b:02X}"))
    }
}

/// The desktop at one instant.
#[derive(Clone, Debug)]
pub struct FrozenDesktop {
    displays: Vec<DisplayFrame>,
    index: WindowIndex,
    cursor: Option<DetachedCursor>,
    captured_at: SystemTime,
}

impl FrozenDesktop {
    pub fn new(
        displays: Vec<DisplayFrame>,
        windows: Vec<WindowMetadata>,
        cursor: Option<DetachedCursor>,
        captured_at: SystemTime,
        opts: &HitTestOptions,
    ) -> Result<Self> {
        if displays.is_empty() {
            return Err(CaptureError::InvalidFrame("no displays".into()));
        }
        if let Some(d) = displays.iter().find(|d| d.info.rect.is_empty()) {
            return Err(CaptureError::InvalidFrame(format!(
                "display {} has an empty rect",
                d.info.id
            )));
        }
        let infos: Vec<DisplayInfo> = displays.iter().map(|d| d.info.clone()).collect();
        let index = WindowIndex::build(&infos, windows, opts);
        Ok(Self {
            displays,
            index,
            cursor,
            captured_at,
        })
    }

    pub fn bounds(&self) -> Rect {
        self.index.desktop()
    }

    pub fn displays(&self) -> &[DisplayFrame] {
        &self.displays
    }

    pub fn index(&self) -> &WindowIndex {
        &self.index
    }

    pub fn cursor(&self) -> Option<&DetachedCursor> {
        self.cursor.as_ref()
    }

    pub fn hit_test(&self, p: PointI32) -> HitTarget {
        self.index.hit_test(p)
    }

    fn display_at(&self, x: f64, y: f64) -> Option<&DisplayFrame> {
        self.displays.iter().find(|d| {
            let r = d.info.rect;
            x >= f64::from(r.x0)
                && x < f64::from(r.x1)
                && y >= f64::from(r.y0)
                && y < f64::from(r.y1)
        })
    }

    /// The native pixel at global `p`.
    pub fn pixel_at(&self, p: PointI32) -> Option<[u8; 4]> {
        let (x, y) = (f64::from(p.x), f64::from(p.y));
        let d = self.display_at(x, y)?;
        let (nx, ny) = d.native(x, y)?;
        d.frame.pixel(nx, ny)
    }

    /// `(2r+1)²` native pixels around global `p` (`r` capped at 64).
    pub fn loupe(&self, p: PointI32, radius: u32) -> Loupe {
        let r = i64::from(radius.min(64));
        let size = (2 * r + 1) as u32;
        let (x, y) = (f64::from(p.x), f64::from(p.y));
        let Some((d, (cx, cy))) = self
            .display_at(x, y)
            .and_then(|d| d.native(x, y).map(|n| (d, n)))
        else {
            return Loupe {
                size,
                pixels: vec![[0; 4]; (size * size) as usize],
                center: None,
                display_id: None,
            };
        };
        let pixels = (-r..=r)
            .flat_map(|dy| (-r..=r).map(move |dx| (dx, dy)))
            .map(|(dx, dy)| d.frame.pixel(cx + dx, cy + dy).unwrap_or_default())
            .collect();
        Loupe {
            size,
            pixels,
            center: d.frame.pixel(cx, cy),
            display_id: Some(d.info.id),
        }
    }

    /// The still capture of a global `rect` (region, window or display).
    pub fn crop(
        &self,
        rect: Rect,
        mode: CaptureMode,
        source: Option<&WindowMetadata>,
    ) -> Result<RawCapturePayload> {
        let (frame, scale) = compose(&self.displays, rect)?;
        let center = (
            f64::from(rect.x0) + f64::from(rect.width()) / 2.0,
            f64::from(rect.y0) + f64::from(rect.height()) / 2.0,
        );
        let color = self
            .display_at(center.0, center.1)
            .or_else(|| self.displays.first())
            .map_or_else(ColorInfo::default, |d| d.info.color.clone());
        Ok(RawCapturePayload {
            frame,
            rect,
            scale,
            mode,
            displays: self.displays.iter().map(|d| d.info.clone()).collect(),
            source_window: source.cloned(),
            cursor: self.cursor.clone(),
            color,
            captured_at: self.captured_at,
        })
    }

    /// Capture of the target under `p` (click in All-in-One mode).
    pub fn crop_target(&self, target: &HitTarget) -> Result<RawCapturePayload> {
        let window = target
            .window
            .as_ref()
            .and_then(|w| self.index.windows().iter().find(|m| m.id == w.id));
        let mode = match target.kind {
            HitKind::Element | HitKind::Window => CaptureMode::Window,
            HitKind::Display => CaptureMode::Fullscreen,
            HitKind::Nothing => {
                return Err(CaptureError::InvalidRegion(
                    "nothing under the cursor".into(),
                ));
            }
        };
        self.crop(target.rect, mode, window)
    }
}

/// Composes a global `rect` from display frames at the highest pixel
/// density among the displays it touches (nearest-neighbour from the
/// others); areas outside every display are transparent.
pub(crate) fn compose(displays: &[DisplayFrame], rect: Rect) -> Result<(RgbaFrame, f32)> {
    if rect.is_empty() {
        return Err(CaptureError::InvalidRegion("empty rect".into()));
    }
    let touching: Vec<&DisplayFrame> = displays
        .iter()
        .filter(|d| !d.info.rect.intersect(&rect).is_empty())
        .collect();
    if touching.is_empty() {
        return Err(CaptureError::InvalidRegion(
            "the region is not on any display".into(),
        ));
    }
    let scale = touching
        .iter()
        .map(|d| d.scale())
        .fold(1.0f64, f64::max)
        .clamp(0.25, 8.0);
    let w = (f64::from(rect.width()) * scale).round().max(1.0) as u32;
    let h = (f64::from(rect.height()) * scale).round().max(1.0) as u32;
    let len = crate::frame_len(w, h)?;
    let mut data = Vec::new();
    data.try_reserve_exact(len)
        .map_err(|e| CaptureError::Allocation(e.to_string()))?;
    data.resize(len, 0);
    data.par_chunks_mut(w as usize * 4)
        .enumerate()
        .for_each(|(oy, row)| {
            let gy = f64::from(rect.y0) + (oy as f64 + 0.5) / scale;
            let mut last: Option<&DisplayFrame> = None;
            for (ox, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let gx = f64::from(rect.x0) + (ox as f64 + 0.5) / scale;
                let inside = |d: &DisplayFrame| {
                    let r = d.info.rect;
                    gx >= f64::from(r.x0)
                        && gx < f64::from(r.x1)
                        && gy >= f64::from(r.y0)
                        && gy < f64::from(r.y1)
                };
                let d = match last {
                    Some(d) if inside(d) => Some(d),
                    _ => touching.iter().copied().find(|d| inside(d)),
                };
                last = d;
                if let Some(p) = d
                    .and_then(|d| d.native(gx, gy))
                    .zip(d)
                    .and_then(|((nx, ny), d)| d.frame.pixel(nx, ny))
                {
                    px.copy_from_slice(&p);
                }
            }
        });
    Ok((RgbaFrame::new(w, h, data)?, scale as f32))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{ColorInfo, UiRole};

    fn display(id: u32, rect: Rect, scale: u32, color: u8) -> DisplayFrame {
        let (w, h) = (rect.width() * scale, rect.height() * scale);
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&[color, (x % 256) as u8, (y % 256) as u8, 255]);
            }
        }
        DisplayFrame {
            info: DisplayInfo {
                id,
                name: format!("D{id}"),
                rect,
                scale_factor: scale as f32,
                primary: id == 1,
                rotation_deg: 0.0,
                refresh_hz: 60.0,
                color: ColorInfo::default(),
            },
            frame: RgbaFrame::new(w, h, data).unwrap(),
        }
    }

    fn window(id: u64, z: u32, frame: Rect) -> WindowMetadata {
        WindowMetadata {
            id,
            pid: Some(1000 + id as u32),
            app_name: format!("app{id}"),
            title: format!("Window {id}"),
            frame,
            z_order: z,
            ..WindowMetadata::default()
        }
    }

    fn desktop() -> FrozenDesktop {
        // A 1x display at the left and a 2x (Retina-style) one to its right.
        let displays = vec![
            display(1, Rect::new(0, 0, 800, 600), 1, 10),
            display(2, Rect::new(800, 0, 1200, 300), 2, 20),
        ];
        let mut editor = window(1, 1, Rect::new(100, 100, 700, 500));
        editor.elements = vec![
            UiRegion {
                rect: Rect::new(100, 100, 700, 140),
                role: UiRole::Toolbar,
                name: "Toolbar".into(),
                depth: 1,
            },
            UiRegion {
                rect: Rect::new(110, 105, 140, 135),
                role: UiRole::Button,
                name: "Save".into(),
                depth: 2,
            },
        ];
        let windows = vec![
            editor,
            window(2, 0, Rect::new(500, 50, 900, 250)), // frontmost, spans both displays
            window(3, 2, Rect::new(0, 0, 800, 600)),
            WindowMetadata {
                minimized: true,
                ..window(4, 0, Rect::new(0, 0, 50, 50))
            },
        ];
        let opts = HitTestOptions {
            exclude_pids: vec![],
            ..HitTestOptions::default()
        };
        FrozenDesktop::new(displays, windows, None, SystemTime::UNIX_EPOCH, &opts).unwrap()
    }

    #[test]
    fn frontmost_window_wins() {
        let d = desktop();
        let h = d.hit_test(PointI32::new(600, 200));
        assert_eq!((h.kind, h.window.map(|w| w.id)), (HitKind::Window, Some(2)));
        let h = d.hit_test(PointI32::new(300, 300));
        assert_eq!(h.window.map(|w| w.id), Some(1));
        let h = d.hit_test(PointI32::new(20, 20));
        assert_eq!(h.window.map(|w| w.id), Some(3)); // minimized window 4 skipped
    }

    #[test]
    fn tightest_element_then_wider() {
        let d = desktop();
        let stack = d.index().hit_stack(PointI32::new(120, 120));
        let kinds: Vec<_> = stack.iter().map(|h| h.kind).collect();
        assert_eq!(kinds, [HitKind::Element, HitKind::Window, HitKind::Display]);
        assert_eq!(
            stack[0].element.as_ref().map(|e| e.name.as_str()),
            Some("Save")
        );
        assert_eq!(stack[0].rect, Rect::new(110, 105, 140, 135));
    }

    #[test]
    fn windows_are_clipped_to_the_desktop() {
        let d = desktop();
        let h = d.hit_test(PointI32::new(850, 100));
        assert_eq!(h.rect, Rect::new(500, 50, 900, 250));
        assert_eq!(h.display_id, Some(2));
        assert_eq!(d.hit_test(PointI32::new(1000, 500)).kind, HitKind::Nothing);
    }

    #[test]
    fn loupe_reads_native_pixels() {
        let d = desktop();
        // Global (900, 10) on the 2x display is native (200, 20).
        let l = d.loupe(PointI32::new(900, 10), 2);
        assert_eq!(l.size, 5);
        assert_eq!(l.center, Some([20, 200, 20, 255]));
        assert_eq!(l.pixels[0], [20, 198, 18, 255]);
        assert_eq!(l.hex().as_deref(), Some("#14C814"));
        assert_eq!(d.loupe(PointI32::new(-5, -5), 1).center, None);
    }

    #[test]
    fn crop_across_mixed_dpi_uses_the_denser_display() {
        let d = desktop();
        let p = d
            .crop(Rect::new(790, 10, 810, 20), CaptureMode::Region, None)
            .unwrap();
        assert_eq!((p.frame.width(), p.frame.height(), p.scale), (40, 20, 2.0));
        // Left half from display 1 (red channel 10), right from display 2 (20).
        assert_eq!(p.frame.pixel(0, 0).map(|p| p[0]), Some(10));
        assert_eq!(p.frame.pixel(39, 0).map(|p| p[0]), Some(20));
        // Below display 2's bottom edge: transparent.
        let p = d
            .crop(Rect::new(1100, 290, 1110, 310), CaptureMode::Region, None)
            .unwrap();
        assert_eq!(p.frame.pixel(0, 39), Some([0, 0, 0, 0]));
        assert!(
            d.crop(Rect::new(2000, 2000, 2010, 2010), CaptureMode::Region, None)
                .is_err()
        );
    }

    #[test]
    fn huge_desktops_cap_the_grid() {
        let infos = [display(1, Rect::new(0, 0, 4, 4), 1, 0).info];
        let mut big = infos[0].clone();
        big.rect = Rect::new(-1_000_000, -1_000_000, 1_000_000, 1_000_000);
        let idx = WindowIndex::build(
            &[big],
            vec![window(1, 0, Rect::new(0, 0, 100, 100))],
            &HitTestOptions::default(),
        );
        assert!(i64::from(idx.cols) * i64::from(idx.rows) <= MAX_CELLS);
        assert_eq!(idx.window_at(PointI32::new(50, 50)).map(|w| w.id), Some(1));
    }
}
