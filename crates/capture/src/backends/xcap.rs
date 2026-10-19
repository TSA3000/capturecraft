//! Portable backend on the `xcap` crate.

use ::xcap::{Monitor, Window, XCapError};
use photocraft_geom::Rect;

use crate::{
    Capabilities, CaptureBackend, CaptureError, CaptureSession, ColorInfo, DetachedCursor,
    DisplayInfo, PollingSession, Result, RgbaFrame, SessionOptions, SessionTarget, WindowMetadata,
};

const NAME: &str = "xcap";

#[derive(Clone, Copy, Debug, Default)]
pub struct XcapBackend;

/// Maps xcap's string errors onto typed ones where the OS message says
/// what happened (TCC denial, portal cancellation).
fn map_err(e: XCapError) -> CaptureError {
    let msg = e.to_string();
    let lower = msg.to_lowercase();
    if matches!(e, XCapError::NotSupported) {
        CaptureError::Unsupported {
            backend: NAME,
            what: "do this on this platform".into(),
        }
    } else if lower.contains("permission")
        || lower.contains("denied")
        || lower.contains("not authorized")
    {
        CaptureError::PermissionDenied {
            hint: format!("allow screen recording for CaptureCraft ({msg})"),
        }
    } else if lower.contains("cancel") {
        CaptureError::Cancelled
    } else {
        CaptureError::Backend { backend: NAME, msg }
    }
}

fn image(img: ::xcap::image::RgbaImage) -> Result<RgbaFrame> {
    let (w, h) = (img.width(), img.height());
    RgbaFrame::new(w, h, img.into_raw())
}

fn display(m: &Monitor) -> Result<DisplayInfo> {
    let rect = Rect::from_xywh(
        m.x().map_err(map_err)?,
        m.y().map_err(map_err)?,
        m.width().map_err(map_err)?,
        m.height().map_err(map_err)?,
    );
    Ok(DisplayInfo {
        id: m.id().map_err(map_err)?,
        name: m.friendly_name().or_else(|_| m.name()).unwrap_or_default(),
        rect,
        scale_factor: m.scale_factor().unwrap_or(1.0),
        primary: m.is_primary().unwrap_or(false),
        rotation_deg: m.rotation().unwrap_or(0.0),
        refresh_hz: m.frequency().unwrap_or(0.0),
        // xcap does not report colour spaces; native backends fill this.
        color: ColorInfo::default(),
    })
}

fn find_monitor(id: u32) -> Result<Monitor> {
    Monitor::all()
        .map_err(map_err)?
        .into_iter()
        .find(|m| m.id().ok() == Some(id))
        .ok_or(CaptureError::NoSuchDisplay(id))
}

fn find_window(id: u64) -> Result<Window> {
    Window::all()
        .map_err(map_err)?
        .into_iter()
        .find(|w| w.id().ok().map(u64::from) == Some(id))
        .ok_or(CaptureError::NoSuchWindow(id))
}

impl CaptureBackend for XcapBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            window_enumeration: true,
            window_capture: true,
            detached_cursor: false,
            ui_elements: false,
            scroll_injection: false,
            os_picker: cfg!(target_os = "linux"),
        }
    }

    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Monitor::all()
            .map_err(map_err)?
            .iter()
            .map(display)
            .collect()
    }

    fn windows(&self) -> Result<Vec<WindowMetadata>> {
        // `Window::all` enumerates frontmost first on every platform
        // (EnumWindows, CGWindowList, _NET_CLIENT_LIST_STACKING reversed),
        // so the index is the z-order; `Window::z` would re-enumerate per window.
        let windows = Window::all().map_err(map_err)?;
        Ok(windows
            .iter()
            .enumerate()
            .filter_map(|(z, w)| {
                let frame =
                    Rect::from_xywh(w.x().ok()?, w.y().ok()?, w.width().ok()?, w.height().ok()?);
                Some(WindowMetadata {
                    id: u64::from(w.id().ok()?),
                    pid: w.pid().ok(),
                    app_name: w.app_name().unwrap_or_default(),
                    process_name: String::new(),
                    title: w.title().unwrap_or_default(),
                    url: None,
                    frame,
                    z_order: z as u32,
                    display_id: w.current_monitor().ok().and_then(|m| m.id().ok()),
                    minimized: w.is_minimized().unwrap_or(false),
                    focused: w.is_focused().unwrap_or(false),
                    elements: Vec::new(),
                })
            })
            .collect())
    }

    fn cursor(&self) -> Result<Option<DetachedCursor>> {
        Ok(None)
    }

    fn grab_display(&self, id: u32) -> Result<RgbaFrame> {
        image(find_monitor(id)?.capture_image().map_err(map_err)?)
    }

    fn grab_window(&self, id: u64) -> Result<RgbaFrame> {
        image(find_window(id)?.capture_image().map_err(map_err)?)
    }

    fn start_session(
        &self,
        target: SessionTarget,
        opts: SessionOptions,
    ) -> Result<Box<dyn CaptureSession>> {
        match target {
            SessionTarget::Display(id) => {
                let m = find_monitor(id)?;
                let rect = display(&m)?.rect;
                Ok(Box::new(PollingSession::new(
                    rect,
                    opts.fps,
                    // By id, re-resolved per frame: xcap handles are not `Send` everywhere.
                    Box::new(move || image(find_monitor(id)?.capture_image().map_err(map_err)?)),
                )))
            }
            SessionTarget::Window(id) => {
                let w = find_window(id)?;
                let rect = Rect::from_xywh(
                    w.x().map_err(map_err)?,
                    w.y().map_err(map_err)?,
                    w.width().map_err(map_err)?,
                    w.height().map_err(map_err)?,
                );
                Ok(Box::new(PollingSession::new(
                    rect,
                    opts.fps,
                    Box::new(move || image(find_window(id)?.capture_image().map_err(map_err)?)),
                )))
            }
            SessionTarget::Region(rect) => {
                let backend = *self;
                Ok(Box::new(PollingSession::new(
                    rect,
                    opts.fps,
                    Box::new(move || backend.grab_region(rect).map(|(f, _)| f)),
                )))
            }
        }
    }
}
