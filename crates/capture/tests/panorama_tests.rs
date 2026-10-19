//! Synthetic scrolling captures: a tall virtual page with a sticky header
//! and a fixed right scrollbar (whose thumb moves), sliced into overlapping
//! viewports with varying scroll steps, stitched and compared pixel by
//! pixel with the page.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use capturecraft_capture::panorama::{
    AutoScrollEnd, AutoScrollOptions, Method, ScrollTarget, auto_scroll, estimate_shift,
};
use capturecraft_capture::{
    Capabilities, CaptureBackend, CaptureError, CaptureSession, DetachedCursor, DisplayInfo,
    FrameOutcome, PanoramaStitcher, PointI32, Result, RgbaFrame, ScrollAxis, ScrollRequest,
    SessionOptions, SessionTarget, StitchConfig, WindowMetadata,
};
use photocraft_geom::Rect;
use proptest::prelude::*;

const W: u32 = 320;
const H: u32 = 400;
const DOC_H: u32 = 3000;
const HEADER: u32 = 48;
const BAR: u32 = 14;
const INNER: u32 = W - BAR;

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Page content: text-like rows with blank bands every 240 rows.
fn content(x: u32, y: u32) -> [u8; 4] {
    if (200..215).contains(&(y % 240)) {
        return [250, 250, 250, 255];
    }
    let h = mix(u64::from(x / 3) << 32 | u64::from(y));
    if h.is_multiple_of(5) {
        [20, 20, (h >> 8) as u8 % 60, 255]
    } else {
        [255 - (y % 7) as u8 * 3, 250, 245 - (x % 5) as u8, 255]
    }
}

fn header(x: u32, y: u32) -> [u8; 4] {
    [30, 60 + (x % 50) as u8, 90 + y as u8, 255]
}

/// A non-uniform floating "chat" button near the bottom right.
const WIDGET: (u32, u32, u32, u32) = (INNER - 80, H - 70, INNER - 16, H - 14);

fn widget(x: u32, y: u32) -> [u8; 4] {
    [(x * 13 % 256) as u8, 40, (y * 7 % 256) as u8, 255]
}

#[derive(Clone, Copy, Default)]
struct Page {
    widget: bool,
    /// Per-frame noise amplitude on the content (smooth-scroll blending).
    noise: u8,
}

impl Page {
    fn viewport_px(&self, s: u32, x: u32, y: u32, seed: u64) -> [u8; 4] {
        let thumb = H * H / DOC_H;
        let top = s * (H - thumb) / (DOC_H - H);
        if x >= INNER {
            return if (top..top + thumb).contains(&y) {
                [120, 120, 120, 255]
            } else {
                [220, 220, 220, 255]
            };
        }
        if y < HEADER {
            return header(x, y);
        }
        let (x0, y0, x1, y1) = WIDGET;
        if self.widget && (x0..x1).contains(&x) && (y0..y1).contains(&y) {
            return widget(x, y);
        }
        let mut p = content(x, s + y);
        if self.noise > 0 {
            let n = mix(seed ^ u64::from(x) << 20 ^ u64::from(y));
            for (i, c) in p.iter_mut().take(3).enumerate() {
                let d = ((n >> (i * 8)) % (2 * u64::from(self.noise) + 1)) as i16
                    - i16::from(self.noise);
                *c = (i16::from(*c) + d).clamp(0, 255) as u8;
            }
        }
        p
    }

    /// The viewport scrolled to `s`.
    fn frame(&self, s: u32, seed: u64) -> RgbaFrame {
        let mut data = Vec::with_capacity((W * H * 4) as usize);
        for y in 0..H {
            for x in 0..W {
                data.extend_from_slice(&self.viewport_px(s, x, y, seed));
            }
        }
        RgbaFrame::new(W, H, data).unwrap()
    }
}

/// What the stitched page must look like for scroll positions `s_min..=s_max`
/// (`last` = the bottom-most frame's position, for the widget).
fn expected(page: &Page, s_min: u32, s_max: u32) -> Vec<[u8; 4]> {
    let len = s_max - s_min + H;
    let mut out = Vec::with_capacity((INNER * len) as usize);
    for r in 0..len {
        for x in 0..INNER {
            let p = if r < HEADER {
                header(x, r)
            } else {
                content(x, s_min + r)
            };
            out.push(p);
        }
    }
    if page.widget {
        let (x0, y0, x1, y1) = WIDGET;
        let base = s_max - s_min;
        for y in y0..y1 {
            for x in x0..x1 {
                out[((base + y) * INNER + x) as usize] = widget(x, y);
            }
        }
    }
    out
}

fn pixels(p: &capturecraft_capture::Panorama) -> Vec<[u8; 4]> {
    let r = Rect::from_xywh(0, 0, p.size.width, p.size.height);
    p.surface.to_interleaved(r).as_chunks::<4>().0.to_vec()
}

fn transpose(v: &[[u8; 4]], w: u32, h: u32) -> Vec<[u8; 4]> {
    let mut out = vec![[0; 4]; v.len()];
    for y in 0..h {
        for x in 0..w {
            out[(x * h + y) as usize] = v[(y * w + x) as usize];
        }
    }
    out
}

fn first_diff(a: &[[u8; 4]], b: &[[u8; 4]], w: u32) -> Option<(u32, u32, [u8; 4], [u8; 4])> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .map(|i| (i as u32 % w, i as u32 / w, a[i], b[i]))
}

/// Feeds scroll positions and returns the stitched result.
fn stitch(page: &Page, positions: &[u32], cfg: StitchConfig) -> capturecraft_capture::Panorama {
    let mut st = PanoramaStitcher::new(cfg.clone());
    for (i, &s) in positions.iter().enumerate() {
        let mut f = page.frame(s, i as u64);
        if cfg.axis == ScrollAxis::Horizontal {
            let t = transpose(f.data().as_chunks::<4>().0, W, H);
            f = RgbaFrame::new(H, W, t.into_iter().flatten().collect()).unwrap();
        }
        let out = st.push_frame(&f).unwrap();
        if i > 0 {
            assert!(
                matches!(out, FrameOutcome::Stitched { .. } | FrameOutcome::Duplicate),
                "frame {i} at {s}: {out:?}"
            );
        }
    }
    st.finish().unwrap()
}

fn positions(deltas: &[u32]) -> Vec<u32> {
    let mut s = 0;
    let mut out = vec![0];
    for d in deltas {
        s = (s + d).min(DOC_H - H);
        out.push(s);
        if s == DOC_H - H {
            break;
        }
    }
    out
}

const DELTAS: [u32; 17] = [
    37, 120, 5, 260, 91, 300, 1, 77, 199, 3, 288, 150, 64, 300, 300, 300, 300,
];

#[test]
fn sticky_header_and_scrollbar_reconstruct_exactly() {
    let page = Page::default();
    let mut pos = positions(&DELTAS);
    let last = *pos.last().unwrap();
    pos.push(last); // end of page: an unchanged frame
    let p = stitch(&page, &pos, StitchConfig::default());
    assert_eq!(
        (
            p.mask.header,
            p.mask.trailing,
            p.mask.footer,
            p.mask.leading
        ),
        (HEADER, BAR, 0, 0)
    );
    assert_eq!((p.size.width, p.size.height), (INNER, last + H));
    assert_eq!(last, DOC_H - H);
    let got = pixels(&p);
    let want = expected(&page, 0, last);
    assert_eq!(first_diff(&got, &want, INNER), None);
    assert!(
        p.shifts
            .iter()
            .all(|s| s.method == Method::RowHash && s.across == 0)
    );
}

#[test]
fn scrolling_back_up_extends_the_top() {
    let page = Page::default();
    let pos = [1200, 1100, 950, 1000, 1300, 1600, 1700, 1650];
    let p = stitch(&page, &pos, StitchConfig::default());
    assert_eq!(p.size.height, 1700 - 950 + H);
    let got = pixels(&p);
    assert_eq!(first_diff(&got, &expected(&page, 950, 1700), INNER), None);
}

#[test]
fn horizontal_scrolling_is_the_transpose() {
    let page = Page::default();
    let pos = positions(&DELTAS[..6]);
    let last = *pos.last().unwrap();
    let cfg = StitchConfig {
        axis: ScrollAxis::Horizontal,
        ..StitchConfig::default()
    };
    let p = stitch(&page, &pos, cfg);
    let len = last + H;
    assert_eq!((p.size.width, p.size.height), (len, INNER));
    let want = transpose(&expected(&page, 0, last), INNER, len);
    assert_eq!(first_diff(&pixels(&p), &want, len), None);
}

#[test]
fn floating_widget_appears_once() {
    let page = Page {
        widget: true,
        ..Page::default()
    };
    let pos = positions(&[40, 90, 120, 60, 200, 150, 110]);
    let last = *pos.last().unwrap();
    let p = stitch(&page, &pos, StitchConfig::default());
    let got = pixels(&p);
    assert_eq!(first_diff(&got, &expected(&page, 0, last), INNER), None);
}

#[test]
fn noisy_frames_align_on_edge_projections() {
    let page = Page {
        noise: 2,
        ..Page::default()
    };
    let deltas = [40, 130, 75, 210, 33, 180];
    let pos = positions(&deltas);
    let last = *pos.last().unwrap();
    let p = stitch(&page, &pos, StitchConfig::default());
    let along: Vec<i32> = p.shifts.iter().map(|s| s.along).collect();
    assert_eq!(along, deltas.map(|d| d as i32));
    assert!(p.shifts.iter().all(|s| s.method == Method::EdgeProjection));
    let got = pixels(&p);
    let want = expected(&page, 0, last);
    assert_eq!(got.len(), want.len());
    let worst = got
        .iter()
        .zip(&want)
        .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)))
        .max();
    assert!(worst <= Some(2), "{worst:?}");
}

/// A page of scattered rectangles: corners for the feature matcher.
fn blocks(x: i64, y: i64) -> [u8; 4] {
    let (cx, cy) = (x.div_euclid(24), y.div_euclid(24));
    let h = mix((cx as u64) << 32 ^ cy as u64);
    let (bx, by) = (x.rem_euclid(24), y.rem_euclid(24));
    if h.is_multiple_of(3)
        && (4..4 + (h >> 8) as i64 % 14 + 4).contains(&bx)
        && (3..3 + (h >> 16) as i64 % 15 + 4).contains(&by)
    {
        [(h >> 24) as u8, (h >> 32) as u8, 60, 255]
    } else {
        [240, 240, 236, 255]
    }
}

fn block_frame(ox: i64, oy: i64) -> RgbaFrame {
    let (w, h) = (300u32, 300u32);
    let data = (0..i64::from(h))
        .flat_map(|y| (0..i64::from(w)).map(move |x| (x, y)))
        .flat_map(|(x, y)| blocks(x + ox, y + oy))
        .collect();
    RgbaFrame::new(w, h, data).unwrap()
}

#[test]
fn cross_axis_drift_falls_back_to_features() {
    let prev = block_frame(500, 1000);
    let cur = block_frame(503, 1040); // drifted 3 px right while scrolling 40
    let s = estimate_shift(&prev, &cur, &StitchConfig::default())
        .unwrap()
        .unwrap();
    assert_eq!((s.along, s.across, s.method), (40, 3, Method::Features));
}

#[test]
fn bad_frames_are_errors_not_panics() {
    let mut st = PanoramaStitcher::new(StitchConfig::default());
    let tiny = RgbaFrame::new(8, 8, vec![0; 256]).unwrap();
    assert!(matches!(
        st.push_frame(&tiny),
        Err(CaptureError::InvalidFrame(_))
    ));
    let page = Page::default();
    st.push_frame(&page.frame(0, 0)).unwrap();
    let other = RgbaFrame::new(W, H - 1, vec![0; (W * (H - 1) * 4) as usize]).unwrap();
    assert!(st.push_frame(&other).is_err());
    // Unrelated content is rejected, not stitched.
    let noise: Vec<u8> = (0..W * H * 4).map(|i| mix(u64::from(i)) as u8).collect();
    let out = st
        .push_frame(&RgbaFrame::new(W, H, noise).unwrap())
        .unwrap();
    assert!(matches!(out, FrameOutcome::Rejected { .. }), "{out:?}");
    let p = st.finish().unwrap();
    assert_eq!((p.size.width, p.size.height), (W, H));
}

#[test]
fn preview_grows_with_the_page() {
    let page = Page::default();
    let mut st = PanoramaStitcher::new(StitchConfig::default());
    assert!(st.preview().is_none());
    for s in positions(&[100, 200, 300]) {
        st.push_frame(&page.frame(s, 0)).unwrap();
    }
    let pv = st.preview().unwrap();
    assert_eq!(pv.width(), 160);
    // Content rows (header excluded) at 160/306 scale.
    let content = 600 + H - HEADER;
    assert!(
        (pv.height() as i64 - i64::from(content) * 160 / 306).abs() <= 2,
        "{}",
        pv.height()
    );
}

// --- Auto-scroll against a fake backend -------------------------------------

struct FakeBackend {
    scroll: Arc<Mutex<u32>>,
}

struct FakeSession {
    scroll: Arc<Mutex<u32>>,
}

impl CaptureBackend for FakeBackend {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            scroll_injection: true,
            ..Capabilities::default()
        }
    }
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(Vec::new())
    }
    fn windows(&self) -> Result<Vec<WindowMetadata>> {
        Ok(Vec::new())
    }
    fn cursor(&self) -> Result<Option<DetachedCursor>> {
        Ok(None)
    }
    fn grab_display(&self, id: u32) -> Result<RgbaFrame> {
        Err(CaptureError::NoSuchDisplay(id))
    }
    fn grab_window(&self, id: u64) -> Result<RgbaFrame> {
        Err(CaptureError::NoSuchWindow(id))
    }
    fn start_session(
        &self,
        _: SessionTarget,
        _: SessionOptions,
    ) -> Result<Box<dyn CaptureSession>> {
        Ok(Box::new(FakeSession {
            scroll: self.scroll.clone(),
        }))
    }
    fn scroll(&self, r: ScrollRequest) -> Result<()> {
        let mut s = self.scroll.lock().map_err(|_| CaptureError::Timeout)?;
        // 3 notches = 129 px, clamped at the end of the page.
        *s = (*s + r.notches.max(0) as u32 * 43).min(DOC_H - H);
        Ok(())
    }
}

impl CaptureSession for FakeSession {
    fn target_rect(&self) -> Rect {
        Rect::from_xywh(0, 0, W, H)
    }
    fn next_frame(&mut self, _: Duration) -> Result<Option<RgbaFrame>> {
        let s = *self.scroll.lock().map_err(|_| CaptureError::Timeout)?;
        Ok(Some(Page::default().frame(s, 0)))
    }
    fn stop(&mut self) -> Result<()> {
        Ok(())
    }
}

#[test]
fn auto_scroll_stops_at_the_end_of_the_page() {
    let backend = FakeBackend {
        scroll: Arc::new(Mutex::new(0)),
    };
    let mut session = backend
        .start_session(SessionTarget::Window(1), SessionOptions::default())
        .unwrap();
    let mut st = PanoramaStitcher::new(StitchConfig::default());
    let opts = AutoScrollOptions {
        settle: Duration::ZERO,
        ..AutoScrollOptions::default()
    };
    let mut seen = 0;
    let end = auto_scroll(
        &backend,
        session.as_mut(),
        &mut st,
        ScrollTarget {
            at: PointI32::new(10, 10),
            window: Some(1),
        },
        &opts,
        &AtomicBool::new(false),
        &mut |_| seen += 1,
    )
    .unwrap();
    assert_eq!(end, AutoScrollEnd::ReachedEnd);
    let p = st.finish().unwrap();
    assert_eq!(p.size.height, DOC_H);
    assert_eq!(
        first_diff(
            &pixels(&p),
            &expected(&Page::default(), 0, DOC_H - H),
            INNER
        ),
        None
    );
    assert!(seen > 20);
}

// --- Properties ---------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    #[test]
    fn any_scroll_sequence_reconstructs_exactly(
        deltas in prop::collection::vec(1u32..=300, 1..30),
    ) {
        let page = Page::default();
        let pos = positions(&deltas);
        let last = *pos.last().unwrap();
        // The first step must move enough to reveal the sticky parts.
        prop_assume!(pos.get(1).is_some_and(|&s| s >= 8));
        let p = stitch(&page, &pos, StitchConfig::default());
        prop_assert_eq!(p.size.height, last + H);
        prop_assert_eq!(first_diff(&pixels(&p), &expected(&page, 0, last), INNER), None);
    }

    #[test]
    fn garbage_frames_never_panic(
        w in 1u32..64, h in 60u32..120,
        seeds in prop::collection::vec(any::<u64>(), 1..6),
    ) {
        let mut st = PanoramaStitcher::new(StitchConfig::default());
        for s in seeds {
            let data = (0..w * h * 4).map(|i| mix(s ^ u64::from(i)) as u8 & 0xF0).collect();
            let _ = st.push_frame(&RgbaFrame::new(w, h, data).unwrap());
        }
        let _ = st.finish();
    }
}
