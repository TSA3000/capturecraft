//! Scrolling / panoramic capture: an incremental stitcher that writes newly
//! revealed strips straight into a sparse `photocraft-raster` [`Surface`]
//! (256 × 256 copy-on-write tiles), so a 30 000 px page never needs one
//! monolithic bitmap.
//!
//! Frames are worked on in an *along/across* orientation: along is the
//! scroll axis (rows for vertical scrolling), across the other one. A
//! horizontal capture is the same algorithm on transposed frames.
//!
//! Per frame:
//! 1. **Sticky parts** (from the first pair that moved): header/footer rows
//!    identical at the same position in both frames, and side columns (fixed
//!    scrollbars, side panels) that do not follow the scroll. They are kept
//!    out of alignment and seams; the final image gets the header from the
//!    top-most frame and the footer from the bottom-most one, and the side
//!    columns are cropped.
//! 2. **Alignment** `(Δacross, Δalong)` with `cur[y][x] = prev[y+Δalong][x+Δacross]`:
//!    exact row-hash matching (screenshots of scrolled pages are pixel
//!    identical), then normalised cross-correlation of 1-D edge projections
//!    (tolerates noise and smooth-scroll blending), then Harris/BRIEF
//!    feature matching with a RANSAC translation model from
//!    `photocraft-algo` for cross-axis drift. Candidates are scored on
//!    `rayon` threads and every estimate is verified on the pixels.
//! 3. **Floating overlays** (chat bubbles, "back to top" buttons): 8 × 8
//!    patches that stay put while the content moves. Rows they cover are not
//!    committed from that frame; the content commits once it has scrolled
//!    out from under them.
//! 4. **Commit**: only rows extending the stitched range, contiguously.
//!
//! The stitched rows always form one contiguous range; at [`finish`]
//! the top-most frame fills everything above it (header) and the bottom-most
//! everything below (footer, overlays at their final place), so the result
//! has no gaps.
//!
//! [`finish`]: PanoramaStitcher::finish

use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use capturecraft_format::{Axis, CaptureMode, MAX_SIDE, ScrollInfo};
use photocraft_algo::features::match_descriptors;
use photocraft_algo::panorama::{Motion, detect, ransac_motion};
use photocraft_color::PixelFormat;
use photocraft_geom::{Rect, Size};
use photocraft_raster::Surface;
use rayon::prelude::*;

use crate::{
    CaptureBackend, CaptureError, CaptureSession, CapturedDocument, ColorInfo, PointI32, Result,
    RgbaFrame, ScrollRequest, WindowMetadata, document_from_surface, rfc3339_utc, tiles,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollAxis {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StitchConfig {
    pub axis: ScrollAxis,
    /// Least overlap (along) between consecutive frames.
    pub min_overlap_px: u32,
    /// Per-channel difference still counted as equal.
    pub tolerance: u8,
    /// Largest across drift the feature fallback accepts.
    pub max_drift_px: u32,
    pub feature_fallback: bool,
    pub detect_sticky: bool,
    pub detect_overlays: bool,
    /// Longest result along the axis.
    pub max_length_px: u32,
    pub preview_width: u32,
    pub preview_max_rows: usize,
}

impl Default for StitchConfig {
    fn default() -> Self {
        Self {
            axis: ScrollAxis::Vertical,
            min_overlap_px: 24,
            tolerance: 6,
            max_drift_px: 48,
            feature_fallback: true,
            detect_sticky: true,
            detect_overlays: true,
            max_length_px: MAX_SIDE,
            preview_width: 160,
            preview_max_rows: 2048,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    RowHash,
    EdgeProjection,
    Features,
}

/// `cur[y][x] = prev[y + along][x + across]`: positive `along` = scrolled down/right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shift {
    pub along: i32,
    pub across: i32,
    pub method: Method,
    pub confidence: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FrameOutcome {
    First,
    /// Nothing moved (end of page, or the app had not repainted yet).
    Duplicate,
    Stitched {
        shift: Shift,
        /// Rows/columns committed from this frame.
        new_px: u32,
        /// Current result length along the axis.
        length: u32,
    },
    /// Could not be aligned with the last accepted frame; skipped.
    Rejected {
        reason: &'static str,
    },
    /// Accepting the frame would exceed `max_length_px`.
    LimitReached,
}

/// Sticky parts of the viewport, in pixels (along: header/footer; across:
/// leading = left/top, trailing = right/bottom).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StickyMask {
    pub header: u32,
    pub footer: u32,
    pub leading: u32,
    pub trailing: u32,
}

/// A frame in along/across orientation: row `y` runs along `across`.
#[derive(Clone, Debug)]
struct Plane {
    w: usize,
    h: usize,
    data: Arc<[u8]>,
}

impl Plane {
    fn from_frame(f: &RgbaFrame, axis: ScrollAxis) -> Result<Plane> {
        let (w, h) = (f.width() as usize, f.height() as usize);
        match axis {
            ScrollAxis::Vertical => Ok(Plane {
                w,
                h,
                data: f.data.clone(),
            }),
            ScrollAxis::Horizontal => {
                let mut out = Vec::new();
                out.try_reserve_exact(w * h * 4)
                    .map_err(|e| CaptureError::Allocation(e.to_string()))?;
                let src = f.data();
                for x in 0..w {
                    for y in 0..h {
                        let i = (y * w + x) * 4;
                        out.extend_from_slice(src.get(i..i + 4).unwrap_or(&[0; 4]));
                    }
                }
                Ok(Plane {
                    w: h,
                    h: w,
                    data: out.into(),
                })
            }
        }
    }

    fn px(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.w + x) * 4;
        match self.data.get(i..i + 4) {
            Some(p) => [p[0], p[1], p[2], p[3]],
            None => [0; 4],
        }
    }

    fn band(&self, y: usize, cols: &Range<usize>) -> &[u8] {
        let row = y * self.w;
        self.data
            .get((row + cols.start) * 4..(row + cols.end) * 4)
            .unwrap_or_default()
    }
}

fn close(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tol)
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn luma(p: [u8; 4]) -> f32 {
    (54.0 * f32::from(p[0]) + 183.0 * f32::from(p[1]) + 19.0 * f32::from(p[2])) / (256.0 * 255.0)
}

fn uniform(row: &[u8]) -> bool {
    let first = row.get(..4).unwrap_or_default();
    row.as_chunks::<4>().0.iter().all(|p| p == first)
}

/// A frame placed in panorama coordinates.
#[derive(Clone, Debug)]
struct Placed {
    plane: Plane,
    /// Across and along offsets of the frame's (0, 0).
    ox: i64,
    oy: i64,
}

/// State once the first moving pair has fixed the sticky mask.
#[derive(Clone, Debug)]
struct Track {
    mask: StickyMask,
    /// Inner (stitched) columns and content rows of the viewport.
    cols: Range<usize>,
    rows: Range<usize>,
    /// Committed along range `[wmin, wmax)`.
    wmin: i64,
    wmax: i64,
    xmin: i64,
    xmax: i64,
    top: Placed,
    bottom: Placed,
    last: Placed,
    /// Floating overlays, viewport coordinates (x across, y along).
    overlays: Vec<Rect>,
}

impl Track {
    fn x_of(&self, ox: i64, c: usize) -> i64 {
        ox + c as i64 - self.cols.start as i64
    }

    fn committable(&self, y: usize) -> bool {
        self.rows.contains(&y)
            && !self
                .overlays
                .iter()
                .any(|r| (y as i32) >= r.y0 && (y as i32) < r.y1)
    }
}

/// Live low-resolution preview of the stitched content for the HUD.
#[derive(Clone, Debug, Default)]
struct Preview {
    inner_w: usize,
    scale: f64,
    first: i64,
    rows: VecDeque<Vec<u8>>,
    max_rows: usize,
}

impl Preview {
    fn width(&self) -> usize {
        ((self.inner_w as f64 * self.scale).round() as usize).max(1)
    }

    /// Adds preview rows for panorama rows `[a, b)` taken from `f`.
    fn add(&mut self, f: &Placed, t: &Track, a: i64, b: i64) {
        if self.scale <= 0.0 || a >= b {
            return;
        }
        let s = self.scale;
        let j0 = (a as f64 * s).ceil() as i64;
        let j1 = (b as f64 * s).ceil() as i64;
        let pw = self.width();
        for j in j0..j1 {
            let p = (j as f64 / s).floor() as i64;
            let y = p - f.oy;
            if y < 0 || y as usize >= f.plane.h {
                continue;
            }
            let mut row = Vec::with_capacity(pw * 4);
            for i in 0..pw {
                let c =
                    t.cols.start + ((i as f64 / s) as usize).min(self.inner_w.saturating_sub(1));
                row.extend_from_slice(&f.plane.px(c, y as usize));
            }
            self.put(j, row);
        }
        while self.rows.len() > self.max_rows.max(16) {
            self.halve();
        }
    }

    fn put(&mut self, j: i64, row: Vec<u8>) {
        if self.rows.is_empty() {
            self.first = j;
        }
        while j < self.first {
            self.rows.push_front(Vec::new());
            self.first -= 1;
        }
        let i = (j - self.first) as usize;
        while self.rows.len() <= i {
            self.rows.push_back(Vec::new());
        }
        if let Some(slot) = self.rows.get_mut(i) {
            *slot = row;
        }
    }

    fn halve(&mut self) {
        self.scale /= 2.0;
        let first = self.first.div_euclid(2);
        let mut rows = VecDeque::with_capacity(self.rows.len() / 2 + 1);
        let mut j = first;
        loop {
            let old = j * 2 - self.first;
            if old as usize >= self.rows.len() {
                break;
            }
            let row = if old >= 0 {
                self.rows
                    .get(old as usize)
                    .map(|r| {
                        r.as_chunks::<4>()
                            .0
                            .iter()
                            .step_by(2)
                            .flatten()
                            .copied()
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            rows.push_back(row);
            j += 1;
        }
        self.first = first;
        self.rows = rows;
    }

    fn frame(&self, axis: ScrollAxis) -> Option<RgbaFrame> {
        let pw = self.width();
        let ph = self.rows.len();
        if ph == 0 {
            return None;
        }
        let mut data = vec![0u8; pw * ph * 4];
        for (y, row) in self.rows.iter().enumerate() {
            for x in 0..pw.min(row.len() / 4) {
                let (ox, oy, w) = match axis {
                    ScrollAxis::Vertical => (x, y, pw),
                    ScrollAxis::Horizontal => (y, x, ph),
                };
                let (src, dst) = ((x) * 4, (oy * w + ox) * 4);
                if let (Some(s), Some(d)) = (row.get(src..src + 4), data.get_mut(dst..dst + 4)) {
                    d.copy_from_slice(s);
                }
            }
        }
        let (w, h) = match axis {
            ScrollAxis::Vertical => (pw, ph),
            ScrollAxis::Horizontal => (ph, pw),
        };
        RgbaFrame::new(w as u32, h as u32, data).ok()
    }
}

/// The incremental scrolling stitcher.
pub struct PanoramaStitcher {
    cfg: StitchConfig,
    view: Option<(usize, usize)>,
    first: Option<Plane>,
    track: Option<Track>,
    surface: Surface,
    hint: i32,
    frames: u32,
    rejected: u32,
    shifts: Vec<Shift>,
    preview: Preview,
}

/// The finished capture.
#[derive(Clone, Debug)]
pub struct Panorama {
    /// RGBA8, content at `(0, 0) .. size`.
    pub surface: Surface,
    pub size: Size,
    pub axis: ScrollAxis,
    pub mask: StickyMask,
    /// Frames that contributed (the first included).
    pub frames: u32,
    pub rejected: u32,
    pub shifts: Vec<Shift>,
}

impl PanoramaStitcher {
    pub fn new(cfg: StitchConfig) -> Self {
        let preview = Preview {
            max_rows: cfg.preview_max_rows,
            ..Preview::default()
        };
        Self {
            cfg,
            view: None,
            first: None,
            track: None,
            surface: Surface::new(PixelFormat::RGBA8),
            hint: 0,
            frames: 0,
            rejected: 0,
            shifts: Vec::new(),
            preview,
        }
    }

    pub fn config(&self) -> &StitchConfig {
        &self.cfg
    }

    /// Sticky parts found so far (`None` before the first moving pair).
    pub fn mask(&self) -> Option<StickyMask> {
        self.track.as_ref().map(|t| t.mask)
    }

    /// The result length along the axis so far.
    pub fn length(&self) -> u32 {
        match (&self.track, self.view) {
            (Some(t), Some((_, h))) => {
                (t.bottom.oy + h as i64 - t.top.oy).clamp(0, i64::from(u32::MAX)) as u32
            }
            (None, Some((_, h))) => h as u32,
            _ => 0,
        }
    }

    /// Low-resolution preview of the stitched content (HUD strip).
    pub fn preview(&self) -> Option<RgbaFrame> {
        self.preview.frame(self.cfg.axis)
    }

    /// Feeds one viewport frame (all frames must have the same size).
    pub fn push_frame(&mut self, frame: &RgbaFrame) -> Result<FrameOutcome> {
        self.push_frame_with_hint(frame, None)
    }

    /// As [`push_frame`](Self::push_frame), with the expected along shift
    /// (auto-scroll knows roughly how far it scrolled); it breaks ties on
    /// repetitive content.
    pub fn push_frame_with_hint(
        &mut self,
        frame: &RgbaFrame,
        expected: Option<i32>,
    ) -> Result<FrameOutcome> {
        let cur = Plane::from_frame(frame, self.cfg.axis)?;
        let Some((w, h)) = self.view else {
            let min_h = self.cfg.min_overlap_px as usize * 2 + 8;
            if cur.w < 16 || cur.h < min_h {
                return Err(CaptureError::InvalidFrame(format!(
                    "a scrolling viewport needs at least 16 × {min_h} px"
                )));
            }
            self.view = Some((cur.w, cur.h));
            self.first = Some(cur);
            self.frames = 1;
            return Ok(FrameOutcome::First);
        };
        if (cur.w, cur.h) != (w, h) {
            return Err(CaptureError::InvalidFrame(
                "the viewport size changed".into(),
            ));
        }
        let prev = match (&self.track, &self.first) {
            (Some(t), _) => t.last.clone(),
            (None, Some(f)) => Placed {
                plane: f.clone(),
                ox: 0,
                oy: 0,
            },
            (None, None) => return Err(CaptureError::InvalidFrame("no first frame".into())),
        };
        if prev.plane.data == cur.data {
            return Ok(FrameOutcome::Duplicate);
        }
        if let Some(e) = expected {
            self.hint = e;
        }
        let tol = self.cfg.tolerance;

        // Rows/columns to align on.
        let (rows, cols, sticky) = match &self.track {
            Some(t) => (t.rows.clone(), t.cols.clone(), None),
            None => {
                let band = w / 10..w - w / 10;
                let (hh, fh) = if self.cfg.detect_sticky {
                    sticky_rows(&prev.plane, &cur, &band, h)
                } else {
                    (0, 0)
                };
                if hh + fh >= h {
                    return Ok(FrameOutcome::Duplicate);
                }
                (hh..h - fh, band, Some((hh, fh)))
            }
        };
        let Some(shift) = estimate(&prev.plane, &cur, &rows, &cols, self.hint, &self.cfg) else {
            self.rejected += 1;
            return Ok(FrameOutcome::Rejected {
                reason: "no consistent shift against the last accepted frame",
            });
        };
        if shift.along == 0 && shift.across == 0 {
            return Ok(FrameOutcome::Duplicate);
        }
        // Sticky parts are found from how the viewport changed; a pixel or
        // two of movement (a scrollbar thumb that barely moved) is too
        // little, so keep comparing against the first frame until it is more.
        if self.track.is_none() && shift.along.unsigned_abs() < MIN_FIRST_SHIFT {
            return Ok(FrameOutcome::Rejected {
                reason: "too little movement yet to find the sticky parts",
            });
        }

        let mut track = match self.track.take() {
            Some(t) => t,
            None => {
                let (hh, fh) = sticky.unwrap_or((0, 0));
                let (lead, trail) = if self.cfg.detect_sticky {
                    side_columns(&prev.plane, &cur, &rows, shift, tol)
                } else {
                    (0, 0)
                };
                let cols = lead..w - trail;
                // Re-check the header/footer on the final columns.
                let (hh2, fh2) = sticky_rows(&prev.plane, &cur, &cols, h);
                let (hh, fh) = (hh.min(hh2), fh.min(fh2));
                let first = prev.clone();
                let mut t = Track {
                    mask: StickyMask {
                        header: hh as u32,
                        footer: fh as u32,
                        leading: lead as u32,
                        trailing: trail as u32,
                    },
                    rows: hh..h - fh,
                    cols,
                    wmin: 0,
                    wmax: 0,
                    xmin: 0,
                    xmax: 0,
                    top: first.clone(),
                    bottom: first.clone(),
                    last: first.clone(),
                    overlays: Vec::new(),
                };
                if self.cfg.detect_overlays {
                    learn_overlays(&mut t, &prev.plane, &cur, shift, tol);
                }
                let s = self.cfg.preview_width as f64 / t.cols.len().max(1) as f64;
                self.preview.inner_w = t.cols.len();
                self.preview.scale = s.min(1.0);
                commit_first(&mut self.surface, &mut self.preview, &mut t, self.cfg.axis)?;
                t
            }
        };

        let placed = Placed {
            plane: cur,
            ox: track.last.ox + i64::from(shift.across),
            oy: track.last.oy + i64::from(shift.along),
        };
        let hh = track.rows.start as i64;
        let fh_end = track.rows.end as i64;
        let top = track.top.oy.min(placed.oy);
        let bottom = track.bottom.oy.max(placed.oy) + h as i64;
        if bottom - top > i64::from(self.cfg.max_length_px) {
            self.track = Some(track);
            return Ok(FrameOutcome::LimitReached);
        }
        // Rows held back under an overlay that this frame no longer shows
        // (its header hides them): take them from the previous frame now, or
        // they would be lost. Consecutive frames overlap, so it has them.
        let fill = (track.wmax, placed.oy + hh);
        if fill.0 < fill.1 {
            place_rows(
                &mut self.surface,
                &mut track,
                &prev,
                fill.0,
                fill.1,
                0..w,
                self.cfg.axis,
            )?;
            self.preview.add(&prev, &track, fill.0, fill.1);
            track.wmax = fill.1.min(prev.oy + h as i64);
        }
        let fill = (placed.oy + fh_end, track.wmin);
        if fill.0 < fill.1 {
            place_rows(
                &mut self.surface,
                &mut track,
                &prev,
                fill.0,
                fill.1,
                0..w,
                self.cfg.axis,
            )?;
            self.preview.add(&prev, &track, fill.0, fill.1);
            track.wmin = fill.0.max(prev.oy);
        }
        let touches = placed.oy + hh <= track.wmax && placed.oy + fh_end >= track.wmin;
        let result = if !touches {
            self.rejected += 1;
            Ok(FrameOutcome::Rejected {
                reason: "the frame does not overlap the stitched area",
            })
        } else {
            if self.cfg.detect_overlays {
                learn_overlays(&mut track, &prev.plane, &placed.plane, shift, tol);
            }
            let new_px = commit(
                &mut self.surface,
                &mut self.preview,
                &mut track,
                &placed,
                self.cfg.axis,
            )?;
            if placed.oy < track.top.oy {
                track.top = placed.clone();
            }
            if placed.oy >= track.bottom.oy {
                track.bottom = placed.clone();
            }
            track.last = placed;
            self.hint = shift.along;
            self.frames += 1;
            self.shifts.push(shift);
            Ok(FrameOutcome::Stitched {
                shift,
                new_px,
                length: (bottom - top).clamp(0, i64::from(u32::MAX)) as u32,
            })
        };
        self.track = Some(track);
        result
    }

    /// Fills header/footer from the extreme frames and returns the result.
    pub fn finish(mut self) -> Result<Panorama> {
        let axis = self.cfg.axis;
        let Some(mut t) = self.track.take() else {
            // One frame (or nothing ever moved): the frame itself.
            let first = self
                .first
                .take()
                .ok_or_else(|| CaptureError::InvalidFrame("no frames".into()))?;
            let mut surface = Surface::new(PixelFormat::RGBA8);
            let rect = Rect::from_xywh(0, 0, first.w as u32, first.h as u32);
            write_block(&mut surface, axis, rect, |x, y| {
                first.px(x as usize, y as usize)
            })?;
            let size = oriented(axis, first.w as u32, first.h as u32);
            return Ok(Panorama {
                surface,
                size,
                axis,
                mask: StickyMask::default(),
                frames: self.frames,
                rejected: self.rejected,
                shifts: self.shifts,
            });
        };
        let h = t.top.plane.h as i64;
        // Above the stitched range: the top-most frame (header included).
        let top = t.top.clone();
        let (a, b) = (top.oy, t.wmin);
        place_rows(&mut self.surface, &mut t, &top, a, b, 0..top.plane.w, axis)?;
        // Below it: the bottom-most frame (footer, overlays at their final place).
        let bottom = t.bottom.clone();
        let (a, b) = (t.wmax, bottom.oy + h);
        place_rows(
            &mut self.surface,
            &mut t,
            &bottom,
            a,
            b,
            0..bottom.plane.w,
            axis,
        )?;

        let length = (bottom.oy + h - top.oy).clamp(1, i64::from(MAX_SIDE));
        let width = (t.xmax - t.xmin).clamp(1, i64::from(MAX_SIDE));
        let (dx, dy) = match axis {
            ScrollAxis::Vertical => (-t.xmin, -top.oy),
            ScrollAxis::Horizontal => (-top.oy, -t.xmin),
        };
        let size = oriented(axis, width as u32, length as u32);
        let content = Rect::from_xywh(-dx as i32, -dy as i32, size.width, size.height);
        let surface = if dx == 0 && dy == 0 {
            self.surface
        } else {
            self.surface.translated(dx as i32, dy as i32, content)
        };
        Ok(Panorama {
            surface,
            size,
            axis,
            mask: t.mask,
            frames: self.frames,
            rejected: self.rejected,
            shifts: self.shifts,
        })
    }
}

fn oriented(axis: ScrollAxis, across: u32, along: u32) -> Size {
    match axis {
        ScrollAxis::Vertical => Size::new(across, along),
        ScrollAxis::Horizontal => Size::new(along, across),
    }
}

impl Panorama {
    /// The scrolling capture as a `.capturecraft` document.
    pub fn into_document(
        self,
        source: Option<&WindowMetadata>,
        color: &ColorInfo,
        scale: f32,
        captured_at: SystemTime,
        automatic: bool,
    ) -> Result<CapturedDocument> {
        let mut doc = document_from_surface(self.surface, self.size, color, scale)?;
        let m = &mut doc.manifest;
        m.capture.mode = CaptureMode::Scrolling;
        m.capture.captured_at = rfc3339_utc(captured_at);
        m.capture.os = std::env::consts::OS.into();
        m.capture.source = source.map(WindowMetadata::source_info);
        m.capture.scroll = Some(ScrollInfo {
            axis: match self.axis {
                ScrollAxis::Vertical => Axis::Vertical,
                ScrollAxis::Horizontal => Axis::Horizontal,
            },
            frames: self.frames,
            automatic,
        });
        if let Some(w) = source {
            m.title = w.title.clone();
        }
        m.validate()
            .map_err(|e| CaptureError::InvalidFrame(e.to_string()))?;
        Ok(doc)
    }
}

// ---------------------------------------------------------------------------
// Sticky detection

/// Header and footer: rows identical at the same position in both frames
/// (within `cols`). More than a third of the viewport is not plausible as
/// sticky chrome and counts as content.
fn sticky_rows(prev: &Plane, cur: &Plane, cols: &Range<usize>, h: usize) -> (usize, usize) {
    let same = |y: usize| prev.band(y, cols) == cur.band(y, cols);
    let hh = (0..h).take_while(|&y| same(y)).count();
    let fh = (0..h).rev().take_while(|&y| same(y)).count();
    let cap = h / 3;
    let hh = if hh > cap { 0 } else { hh };
    let fh = if fh > cap || hh + fh >= h { 0 } else { fh };
    (hh, fh)
}

/// Columns at either edge that do not follow the scroll.
fn side_columns(
    prev: &Plane,
    cur: &Plane,
    rows: &Range<usize>,
    s: Shift,
    tol: u8,
) -> (usize, usize) {
    let w = cur.w;
    let overlap = overlap_rows(rows, s.along);
    let n = overlap.len();
    if n == 0 {
        return (0, 0);
    }
    let moving = |c: usize| {
        let pc = c as i64 + i64::from(s.across);
        let misses = overlap
            .clone()
            .filter(|&y| {
                let py = (y as i64 + i64::from(s.along)) as usize;
                pc < 0 || pc as usize >= w || !close(cur.px(c, y), prev.px(pc as usize, py), tol)
            })
            .count();
        misses < 2 && misses * 200 <= n
    };
    let cap = w / 4;
    let trail = (0..w).rev().take(cap).take_while(|&c| !moving(c)).count();
    let lead = (0..w).take(cap).take_while(|&c| !moving(c)).count();
    (lead, trail)
}

fn overlap_rows(rows: &Range<usize>, along: i32) -> Range<usize> {
    let (r0, r1, d) = (rows.start as i64, rows.end as i64, i64::from(along));
    let a = r0.max(r0 - d);
    let b = r1.min(r1 - d);
    if a >= b { 0..0 } else { a as usize..b as usize }
}

const PATCH: usize = 8;

/// Least along movement of the pair that fixes the sticky mask.
const MIN_FIRST_SHIFT: u32 = 8;

/// Learns floating overlays: non-uniform patches that stay at the same
/// viewport position while the content moved.
fn learn_overlays(t: &mut Track, prev: &Plane, cur: &Plane, s: Shift, tol: u8) {
    let (rows, cols) = (t.rows.clone(), t.cols.clone());
    let found: Vec<Rect> = rows
        .clone()
        .step_by(PATCH)
        .collect::<Vec<_>>()
        .par_iter()
        .flat_map_iter(|&y0| {
            let cols = cols.clone();
            let rows = rows.clone();
            cols.clone().step_by(PATCH).filter_map(move |x0| {
                let ys = y0..(y0 + PATCH).min(rows.end);
                let xs = x0..(x0 + PATCH).min(cols.end);
                let p0 = cur.px(x0, y0);
                let mut nonuniform = false;
                let mut shifted_match = true;
                for y in ys.clone() {
                    for x in xs.clone() {
                        let c = cur.px(x, y);
                        if !close(c, prev.px(x, y), tol) {
                            return None;
                        }
                        nonuniform |= !close(c, p0, tol);
                        let (px, py) = (
                            x as i64 + i64::from(s.across),
                            y as i64 + i64::from(s.along),
                        );
                        let inside =
                            px >= 0 && py >= 0 && (px as usize) < cur.w && (py as usize) < cur.h;
                        shifted_match &= inside && close(c, prev.px(px as usize, py as usize), tol);
                    }
                }
                (nonuniform && !shifted_match).then(|| {
                    Rect::new(
                        xs.start as i32,
                        ys.start as i32,
                        xs.end as i32,
                        ys.end as i32,
                    )
                })
            })
        })
        .collect();
    if found.is_empty() {
        return;
    }
    let mut all = std::mem::take(&mut t.overlays);
    all.extend(found.into_iter().map(|r| r.inflate(PATCH as i32)));
    let merged = merge_rects(all);
    // Overlays over most of the content are not overlays (repetitive
    // content fooled the detector): ignore them rather than stall commits.
    let covered: usize = t
        .rows
        .clone()
        .filter(|&y| {
            merged
                .iter()
                .any(|r| (y as i32) >= r.y0 && (y as i32) < r.y1)
        })
        .count();
    t.overlays = if covered * 2 > t.rows.len() {
        Vec::new()
    } else {
        merged
    };
}

fn merge_rects(mut rects: Vec<Rect>) -> Vec<Rect> {
    let mut merged = true;
    while merged {
        merged = false;
        let mut out: Vec<Rect> = Vec::with_capacity(rects.len());
        for r in rects {
            if let Some(o) = out
                .iter_mut()
                .find(|o| !o.inflate(1).intersect(&r).is_empty())
            {
                *o = o.union(&r);
                merged = true;
            } else {
                out.push(r);
            }
        }
        rects = out;
    }
    rects
}

// ---------------------------------------------------------------------------
// Alignment

/// Estimates the shift of `cur` against `prev` over content `rows` × `cols`.
fn estimate(
    prev: &Plane,
    cur: &Plane,
    rows: &Range<usize>,
    cols: &Range<usize>,
    hint: i32,
    cfg: &StitchConfig,
) -> Option<Shift> {
    let max_d = rows.len() as i32 - cfg.min_overlap_px as i32;
    if max_d < 1 || cols.is_empty() {
        return None;
    }
    let pick = |scores: Vec<(i32, f32)>| {
        scores
            .into_iter()
            .filter(|(_, s)| s.is_finite())
            .max_by(|a, b| {
                a.1.total_cmp(&b.1)
                    .then_with(|| (b.0 - hint).abs().cmp(&(a.0 - hint).abs()))
            })
    };
    let verified =
        |along: i32, across: i32| verify(prev, cur, rows, cols, along, across, cfg.tolerance);

    // 1. Exact row hashes.
    let hp: Vec<u64> = rows.clone().map(|y| fnv(prev.band(y, cols))).collect();
    let hc: Vec<u64> = rows.clone().map(|y| fnv(cur.band(y, cols))).collect();
    let info: Vec<bool> = rows.clone().map(|y| !uniform(cur.band(y, cols))).collect();
    let n = rows.len() as i32;
    let scores: Vec<(i32, f32)> = (-max_d..=max_d)
        .into_par_iter()
        .map(|d| {
            let (a, b) = (0.max(-d), n.min(n - d));
            let (mut tot, mut hit, mut informative) = (0.0f32, 0.0f32, 0u32);
            for y in a..b {
                let (i, j) = (y as usize, (y + d) as usize);
                let w = if info[i] { 1.0 } else { 0.05 };
                tot += w;
                if hc[i] == hp[j] {
                    hit += w;
                    informative += u32::from(info[i]);
                }
            }
            let ok = b - a >= cfg.min_overlap_px as i32 && informative >= 3;
            (d, if ok && tot > 0.0 { hit / tot } else { 0.0 })
        })
        .collect();
    if let Some((d, s)) = pick(scores)
        && s >= 0.97
        && (d == 0 || verified(d, 0))
    {
        return Some(Shift {
            along: d,
            across: 0,
            method: Method::RowHash,
            confidence: s,
        });
    }

    // 2. Normalised cross-correlation of horizontal edge energy per row.
    let profile = |p: &Plane| -> Vec<f32> {
        rows.clone()
            .map(|y| {
                (cols.start + 1..cols.end)
                    .map(|x| (luma(p.px(x, y)) - luma(p.px(x - 1, y))).abs())
                    .sum::<f32>()
            })
            .collect()
    };
    let (pp, pc) = (profile(prev), profile(cur));
    let scores: Vec<(i32, f32)> = (-max_d..=max_d)
        .into_par_iter()
        .filter(|&d| d != 0)
        .map(|d| {
            let (a, b) = (0.max(-d), n.min(n - d));
            let xs = (a..b).map(|y| pc[y as usize]);
            let ys = (a..b).map(|y| pp[(y + d) as usize]);
            (d, ncc(xs, ys))
        })
        .collect();
    if let Some((d, s)) = pick(scores)
        && s >= 0.9
        && verified(d, 0)
    {
        return Some(Shift {
            along: d,
            across: 0,
            method: Method::EdgeProjection,
            confidence: s,
        });
    }

    // 3. Features + RANSAC translation (cross-axis drift).
    if !cfg.feature_fallback {
        return None;
    }
    let lum = |p: &Plane| -> Vec<f32> {
        rows.clone()
            .flat_map(|y| cols.clone().map(move |x| (x, y)))
            .map(|(x, y)| luma(p.px(x, y)))
            .collect()
    };
    let (w, h) = (cols.len(), rows.len());
    let fa = detect(w, h, &lum(prev), None, 600);
    let fb = detect(w, h, &lum(cur), None, 600);
    let m = match_descriptors(&fb.desc, &fa.desc, 0.8);
    if m.len() < 12 {
        return None;
    }
    let src: Vec<[f64; 2]> = m
        .iter()
        .filter_map(|&(i, _)| fb.points.get(i).copied())
        .collect();
    let dst: Vec<[f64; 2]> = m
        .iter()
        .filter_map(|&(_, j)| fa.points.get(j).copied())
        .collect();
    if src.len() != dst.len() {
        return None;
    }
    let (hm, inliers) = ransac_motion(Motion::Translation, &src, &dst, 400, 1.5, 0x005C_4011)?;
    if inliers.len() < 12 {
        return None;
    }
    let (tx, ty) = (hm.0[2] / hm.0[8], hm.0[5] / hm.0[8]);
    if !(tx.is_finite() && ty.is_finite()) {
        return None;
    }
    let (across, along) = (tx.round() as i32, ty.round() as i32);
    let ok = across.unsigned_abs() <= cfg.max_drift_px
        && along.abs() <= max_d
        && (along, across) != (0, 0);
    (ok && verified(along, across)).then(|| Shift {
        along,
        across,
        method: Method::Features,
        confidence: inliers.len() as f32 / src.len() as f32,
    })
}

fn ncc(xs: impl Iterator<Item = f32> + Clone, ys: impl Iterator<Item = f32> + Clone) -> f32 {
    let n = xs.clone().count() as f32;
    if n < 2.0 {
        return f32::NAN;
    }
    let mx = xs.clone().sum::<f32>() / n;
    let my = ys.clone().sum::<f32>() / n;
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (x, y) in xs.zip(ys) {
        let (a, b) = (x - mx, y - my);
        sxy += a * b;
        sxx += a * a;
        syy += b * b;
    }
    let d = (sxx * syy).sqrt();
    if d > 1e-6 { sxy / d } else { f32::NAN }
}

/// Checks a shift on the pixels: at most 10 % of sampled overlap pixels and
/// 20 % of its edge pixels may differ (overlays, noise). Edges matter on
/// sparse content, where a wrong across shift only shows at edges.
fn verify(
    prev: &Plane,
    cur: &Plane,
    rows: &Range<usize>,
    cols: &Range<usize>,
    along: i32,
    across: i32,
    tol: u8,
) -> bool {
    let overlap = overlap_rows(rows, along);
    let (mut seen, mut bad, mut edges, mut edge_bad) = (0u32, 0u32, 0u32, 0u32);
    for y in overlap.step_by(2) {
        let py = (y as i64 + i64::from(along)) as usize;
        for x in cols.clone().step_by(2) {
            let px = x as i64 + i64::from(across);
            if px < cols.start as i64 || px >= cols.end as i64 {
                continue;
            }
            let c = cur.px(x, y);
            let miss = !close(c, prev.px(px as usize, py), tol);
            seen += 1;
            bad += u32::from(miss);
            let edge = (x > cols.start && !close(c, cur.px(x - 1, y), tol))
                || (y > rows.start && !close(c, cur.px(x, y - 1), tol));
            if edge {
                edges += 1;
                edge_bad += u32::from(miss);
            }
        }
    }
    seen >= 64 && bad * 10 <= seen && (edges < 32 || edge_bad * 5 <= edges)
}

/// Public entry for one pair of frames (diagnostics and tests): the shift
/// of `cur` against `prev` over the whole viewport.
pub fn estimate_shift(
    prev: &RgbaFrame,
    cur: &RgbaFrame,
    cfg: &StitchConfig,
) -> Result<Option<Shift>> {
    let (a, b) = (
        Plane::from_frame(prev, cfg.axis)?,
        Plane::from_frame(cur, cfg.axis)?,
    );
    if (a.w, a.h) != (b.w, b.h) {
        return Err(CaptureError::InvalidFrame("frames differ in size".into()));
    }
    Ok(estimate(&a, &b, &(0..a.h), &(0..a.w), 0, cfg))
}

// ---------------------------------------------------------------------------
// Committing pixels

/// Writes an along/across block into the surface in output orientation.
fn write_block(
    surface: &mut Surface,
    axis: ScrollAxis,
    block: Rect,
    mut px: impl FnMut(i32, i32) -> [u8; 4],
) -> Result<()> {
    match axis {
        ScrollAxis::Vertical => tiles::blit(surface, block, px),
        ScrollAxis::Horizontal => {
            let t = Rect::new(block.y0, block.x0, block.y1, block.x1);
            tiles::blit(surface, t, |x, y| px(y, x))
        }
    }
}

/// Writes panorama rows `[a, b)` (along) from `f`, columns `cols`.
fn place_rows(
    surface: &mut Surface,
    t: &mut Track,
    f: &Placed,
    a: i64,
    b: i64,
    cols: Range<usize>,
    axis: ScrollAxis,
) -> Result<()> {
    let a = a.max(f.oy);
    let b = b.min(f.oy + f.plane.h as i64);
    let cols = cols.start.max(t.cols.start)..cols.end.min(t.cols.end);
    if a >= b || cols.is_empty() {
        return Ok(());
    }
    let x0 = t.x_of(f.ox, cols.start);
    let x1 = t.x_of(f.ox, cols.end);
    let (Ok(x0i), Ok(x1i), Ok(ai), Ok(bi)) = (
        i32::try_from(x0),
        i32::try_from(x1),
        i32::try_from(a),
        i32::try_from(b),
    ) else {
        return Err(CaptureError::InvalidRegion(
            "panorama coordinates overflow".into(),
        ));
    };
    write_block(surface, axis, Rect::new(x0i, ai, x1i, bi), |x, p| {
        let c = i64::from(x) - f.ox + t.cols.start as i64;
        let y = i64::from(p) - f.oy;
        f.plane.px(c as usize, y as usize)
    })?;
    t.xmin = t.xmin.min(x0);
    t.xmax = t.xmax.max(x1);
    Ok(())
}

/// The first frame: its longest committable run of content rows.
fn commit_first(
    surface: &mut Surface,
    preview: &mut Preview,
    t: &mut Track,
    axis: ScrollAxis,
) -> Result<()> {
    let f = t.top.clone();
    let rows = t.rows.clone();
    let mut best = (rows.start, rows.start);
    let mut y = rows.start;
    while y < rows.end {
        let s = y;
        while y < rows.end && t.committable(y) {
            y += 1;
        }
        if y - s > best.1 - best.0 {
            best = (s, y);
        }
        y += 1;
    }
    if best.0 == best.1 {
        // Overlays everywhere: commit the content rows anyway.
        best = (rows.start, rows.end);
    }
    let (a, b) = (best.0 as i64, best.1 as i64);
    t.xmin = 0;
    t.xmax = t.cols.len() as i64;
    t.wmin = a;
    t.wmax = b;
    place_rows(surface, t, &f, a, b, t.cols.clone(), axis)?;
    preview.add(&f, t, a, b);
    Ok(())
}

/// Extends the committed range with rows of `f` (both directions).
fn commit(
    surface: &mut Surface,
    preview: &mut Preview,
    t: &mut Track,
    f: &Placed,
    axis: ScrollAxis,
) -> Result<u32> {
    let mut new = 0i64;
    // Downward: rows wmax.. while committable.
    let mut y = t.wmax - f.oy;
    let start = y;
    while y >= 0 && t.committable(y as usize) {
        y += 1;
    }
    if y > start {
        let (a, b) = (t.wmax, f.oy + y);
        place_rows(surface, t, f, a, b, t.cols.clone(), axis)?;
        preview.add(f, t, a, b);
        t.wmax = b;
        new += b - a;
    }
    // Upward: rows ..wmin while committable.
    let mut y = t.wmin - f.oy - 1;
    let start = y;
    while y >= 0 && t.committable(y as usize) {
        y -= 1;
    }
    if y < start {
        let (a, b) = (f.oy + y + 1, t.wmin);
        place_rows(surface, t, f, a, b, t.cols.clone(), axis)?;
        preview.add(f, t, a, b);
        t.wmin = a;
        new += b - a;
    }
    Ok(new.clamp(0, i64::from(u32::MAX)) as u32)
}

// ---------------------------------------------------------------------------
// Auto-scroll driver

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoScrollOptions {
    pub notches_per_step: i32,
    /// Pause after each scroll for the app to repaint.
    pub settle: Duration,
    pub frame_timeout: Duration,
    pub max_frames: u32,
    /// Consecutive unchanged frames that mean "end of page".
    pub stop_after_duplicates: u32,
}

impl Default for AutoScrollOptions {
    fn default() -> Self {
        Self {
            notches_per_step: 3,
            settle: Duration::from_millis(120),
            frame_timeout: Duration::from_secs(2),
            max_frames: 400,
            stop_after_duplicates: 2,
        }
    }
}

/// Where auto-scroll sends its wheel events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollTarget {
    /// A point inside the scrolled area, global coordinates.
    pub at: PointI32,
    pub window: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoScrollEnd {
    ReachedEnd,
    Cancelled,
    LimitReached,
    MaxFrames,
}

/// Scrolls the target step by step and stitches until the page stops
/// moving, the user cancels, or a limit is hit.
pub fn auto_scroll(
    backend: &dyn CaptureBackend,
    session: &mut dyn CaptureSession,
    stitcher: &mut PanoramaStitcher,
    target: ScrollTarget,
    opts: &AutoScrollOptions,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&FrameOutcome),
) -> Result<AutoScrollEnd> {
    let mut duplicates = 0;
    for _ in 0..opts.max_frames {
        if cancel.load(Ordering::Relaxed) {
            return Ok(AutoScrollEnd::Cancelled);
        }
        let frame = session
            .next_frame(opts.frame_timeout)?
            .ok_or(CaptureError::Timeout)?;
        let outcome = stitcher.push_frame(&frame)?;
        progress(&outcome);
        match outcome {
            FrameOutcome::Duplicate => {
                duplicates += 1;
                if duplicates >= opts.stop_after_duplicates {
                    return Ok(AutoScrollEnd::ReachedEnd);
                }
            }
            FrameOutcome::LimitReached => return Ok(AutoScrollEnd::LimitReached),
            _ => duplicates = 0,
        }
        backend.scroll(ScrollRequest {
            axis: stitcher.config().axis,
            notches: opts.notches_per_step,
            at: target.at,
            window: target.window,
        })?;
        std::thread::sleep(opts.settle);
    }
    Ok(AutoScrollEnd::MaxFrames)
}
