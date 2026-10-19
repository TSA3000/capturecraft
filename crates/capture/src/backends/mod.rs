//! Platform backends.
//!
//! [`xcap`] is the portable backend in use today: monitors, windows (with
//! z-order and pid), still grabs and polled sessions on Windows, macOS, X11
//! and Wayland (portal + PipeWire). It cannot return the cursor separately
//! or send scroll events, so detached cursors and auto-scroll need the
//! native backends: `windows.rs` (Windows.Graphics.Capture / DXGI
//! duplication, `GetCursorInfo` + `GetIconInfoEx`, `SendInput`), `macos.rs`
//! (ScreenCaptureKit with `showsCursor = false`, `NSCursor`, `CGEvent`)
//! and `linux.rs` (portal ScreenCast + PipeWire; X11 `XFixesGetCursorImage`,
//! XTest). They implement the same [`CaptureBackend`](crate::CaptureBackend)
//! trait and will replace `xcap` per platform.

#[cfg(feature = "xcap")]
pub mod xcap;

use crate::CaptureBackend;

/// The best backend compiled in for this platform.
pub fn default_backend() -> Option<Box<dyn CaptureBackend>> {
    #[cfg(feature = "xcap")]
    {
        Some(Box::new(xcap::XcapBackend))
    }
    #[cfg(not(feature = "xcap"))]
    {
        None
    }
}
