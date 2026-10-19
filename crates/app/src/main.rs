//! `capturecraft-app`: the CaptureCraft desktop app on eframe + wgpu.
//!
//! Follows PhotoCraft's bootstrap: one `NativeOptions` with the wgpu
//! renderer and a persisted window layout, then `run_native` with an app
//! that takes eframe's wgpu render state, so the editor canvas is
//! composited on the window's own device and queue. Everything else is in
//! [`app`] (viewports) and `capturecraft-ui-egui` (drawing).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod app;
mod capture_flow;

/// Window class / Wayland app id.
pub const APP_ID: &str = "capturecraft";

/// The editor window: 1280 × 800, centred, layout restored from `ui.ron`
/// in the config directory.
fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id(APP_ID)
            .with_title("CaptureCraft")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 460.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        persistence_path: dirs::config_dir().map(|d| d.join("capturecraft").join("ui.ron")),
        ..Default::default()
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "CaptureCraft",
        native_options(),
        Box::new(|cc| Ok(Box::new(app::CaptureCraftApp::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_editor_window_uses_wgpu_and_persists_its_layout() {
        let o = native_options();
        assert_eq!(o.renderer, eframe::Renderer::Wgpu);
        assert!(o.persistence_path.is_some_and(|p| p.ends_with("ui.ron")));
        assert_eq!(o.viewport.inner_size, Some(egui::vec2(1280.0, 800.0)));
    }
}
