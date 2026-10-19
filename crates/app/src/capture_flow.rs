//! From the Capture button to an open document.
//!
//! 1. [`CaptureFlow::start`]: the HUD and the editor window hide.
//! 2. [`HIDE_DELAY`] later (the window manager has to take them off screen
//!    before the grab), the app freezes the desktop through the platform
//!    backend and opens the crosshair overlay over the frozen pixels.
//! 3. The overlay's action is turned into a document ([`open_capture`]) or
//!    dropped, and the windows come back.
//!
//! Everything here is plain logic so it runs in tests without a display;
//! the viewports are driven by `app.rs`.

use std::time::{Duration, Instant};

use capturecraft_capture::{FrozenDesktop, RawCapturePayload};
use capturecraft_engine::{Pixels, Session};

/// How long the app waits after hiding its windows before grabbing the
/// screen. Compositors (DWM, Mutter, KWin) remove a hidden window within a
/// frame or two, and eframe repaints a hidden window only every 100 ms, so
/// this is measured in time, not frames.
pub const HIDE_DELAY: Duration = Duration::from_millis(200);

/// Where a capture is.
#[derive(Default)]
pub enum CaptureFlow {
    #[default]
    Idle,
    /// Our windows are hiding since `since`.
    Hiding { since: Instant },
    /// The crosshair overlay is up over this desktop.
    Overlay(Box<FrozenDesktop>),
}

impl CaptureFlow {
    /// Starts a capture unless one is already running.
    pub fn start(&mut self, now: Instant) -> bool {
        if matches!(self, CaptureFlow::Idle) {
            *self = CaptureFlow::Hiding { since: now };
            true
        } else {
            false
        }
    }

    /// `true` when it is time to freeze the desktop.
    pub fn ready_to_freeze(&self, now: Instant) -> bool {
        matches!(self, CaptureFlow::Hiding { since } if now.duration_since(*since) >= HIDE_DELAY)
    }

    /// Our windows stay hidden while a capture runs.
    pub fn hides_windows(&self) -> bool {
        !matches!(self, CaptureFlow::Idle)
    }

    pub fn desktop(&self) -> Option<&FrozenDesktop> {
        match self {
            CaptureFlow::Overlay(d) => Some(d),
            _ => None,
        }
    }
}

/// Opens a finished capture as the active document; returns its index.
pub fn open_capture(session: &mut Session, payload: RawCapturePayload) -> Result<usize, String> {
    let mut doc = payload.into_document().map_err(|e| e.to_string())?;
    // The tray and Library key documents by id.
    if doc.manifest.id.is_empty() {
        doc.manifest.id = crate::documents::unique_doc_id(session);
    }
    let size = doc.manifest.base.size;
    let pixels = Pixels::from_surface(size, doc.base)
        .ok_or_else(|| "the capture is not RGBA8".to_string())?;
    session
        .open(doc.manifest, Some(pixels))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use capturecraft_capture::{ColorInfo, RgbaFrame};
    use capturecraft_format::CaptureMode;
    use photocraft_geom::Rect;
    use std::time::SystemTime;

    #[test]
    fn waits_for_the_windows_to_hide_before_freezing() {
        let t0 = Instant::now();
        let mut f = CaptureFlow::default();
        assert!(!f.hides_windows());
        assert!(!f.ready_to_freeze(t0 + HIDE_DELAY));
        assert!(f.start(t0));
        assert!(!f.start(t0), "a second click must not restart it");
        assert!(f.hides_windows());
        assert!(!f.ready_to_freeze(t0 + HIDE_DELAY / 2));
        assert!(f.ready_to_freeze(t0 + HIDE_DELAY));
    }

    #[test]
    fn a_capture_opens_as_the_active_document() {
        let (w, h) = (300u32, 200u32);
        let mut data = vec![0u8; (w * h * 4) as usize];
        for (i, px) in data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            px.copy_from_slice(&[(i % 256) as u8, 40, 200, 255]);
        }
        let payload = RawCapturePayload {
            frame: RgbaFrame::new(w, h, data).unwrap(),
            rect: Rect::new(100, 50, 100 + w as i32, 50 + h as i32),
            scale: 1.0,
            mode: CaptureMode::Region,
            displays: Vec::new(),
            source_window: None,
            cursor: None,
            color: ColorInfo::default(),
            captured_at: SystemTime::now(),
        };
        let mut session = Session::new();
        let index = open_capture(&mut session, payload).unwrap();
        let doc = &session.documents()[index];
        assert_eq!(doc.manifest.canvas.size.width, w);
        assert_eq!(doc.manifest.canvas.size.height, h);
        assert_eq!(doc.manifest.capture.mode, CaptureMode::Region);
        assert!(session.active().is_some());
    }
}
