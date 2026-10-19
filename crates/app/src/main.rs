//! `capturecraft-editor`: the CaptureCraft Editor on eframe + wgpu (crate
//! `capturecraft-app`); the tray and the capture window open it with the
//! capture's path.
//!
//! Follows PhotoCraft's bootstrap: one `NativeOptions` with the wgpu
//! renderer and a persisted window layout, then `run_native` with an app
//! that takes eframe's wgpu render state, so the editor canvas is
//! composited on the window's own device and queue ([`wgpu_options`]
//! asks that device for what the compositor needs). Everything else is in
//! [`app`] (viewports), [`export`] (Share / Export, Copy All) and
//! `capturecraft-ui-egui` (drawing).
// No console window next to the editor on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod app;
mod capture_flow;
mod documents;
mod export;

use std::sync::Arc;

use eframe::egui_wgpu::{self, wgpu};

/// Window class / Wayland app id.
pub const APP_ID: &str = "capturecraft";

/// eframe's wgpu setup with the device limits the compositor needs.
///
/// The compositor keeps a whole canvas in one storage buffer of linear
/// RGBA floats (16 bytes a pixel): a 4K capture is about 133 MB, more than
/// wgpu's default 128 MiB binding. The headless compositor asks for the
/// adapter's limits, so the window's device does the same, and for the
/// largest texture the adapter has (long panoramas).
pub fn wgpu_options() -> egui_wgpu::WgpuConfiguration {
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.device_descriptor = Arc::new(|adapter| {
        let a = adapter.limits();
        let base = if adapter.get_info().backend == wgpu::Backend::Gl {
            wgpu::Limits::downlevel_webgl2_defaults()
        } else {
            wgpu::Limits::default()
        };
        wgpu::DeviceDescriptor {
            label: Some("capturecraft device"),
            required_limits: device_limits(base, &a),
            ..Default::default()
        }
    });
    egui_wgpu::WgpuConfiguration {
        wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}

/// `base` raised to what `adapter` supports for buffers and textures.
pub fn device_limits(base: wgpu::Limits, adapter: &wgpu::Limits) -> wgpu::Limits {
    wgpu::Limits {
        max_storage_buffer_binding_size: adapter.max_storage_buffer_binding_size,
        max_buffer_size: adapter.max_buffer_size,
        max_texture_dimension_2d: adapter.max_texture_dimension_2d,
        ..base
    }
}

/// The editor window: 1280 × 800, centred, layout restored from `ui.ron`
/// in the config directory.
fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id(APP_ID)
            .with_title("CaptureCraft Editor")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 460.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: wgpu_options(),
        centered: true,
        persistence_path: dirs::config_dir().map(|d| d.join("capturecraft").join("ui.ron")),
        ..Default::default()
    }
}

/// `capturecraft-editor [image …]`: the images open as documents.
fn main() -> eframe::Result {
    let paths: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    if paths.iter().any(|p| p == "-h" || p == "--help") {
        println!("usage: capturecraft-editor [image.png …]");
        return Ok(());
    }
    eframe::run_native(
        "CaptureCraft",
        native_options(),
        Box::new(move |cc| Ok(Box::new(app::CaptureCraftApp::new(cc, &paths)))),
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
        assert!(matches!(
            o.wgpu_options.wgpu_setup,
            egui_wgpu::WgpuSetup::CreateNew(_)
        ));
    }

    #[test]
    fn the_device_gets_the_adapters_buffer_and_texture_limits() {
        let adapter = wgpu::Limits {
            max_storage_buffer_binding_size: 2 << 30,
            max_buffer_size: 4 << 30,
            max_texture_dimension_2d: 16384,
            ..wgpu::Limits::default()
        };
        let l = device_limits(wgpu::Limits::default(), &adapter);
        assert_eq!(l.max_storage_buffer_binding_size, 2 << 30);
        assert_eq!(l.max_buffer_size, 4 << 30);
        assert_eq!(l.max_texture_dimension_2d, 16384);
        assert_eq!(l.max_bind_groups, wgpu::Limits::default().max_bind_groups);
    }
}
