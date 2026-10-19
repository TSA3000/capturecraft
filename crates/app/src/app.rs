//! [`CaptureCraftApp`]: the `eframe::App` hosting CaptureCraft's three
//! viewports on one wgpu device.
//!
//! 1. The root viewport: the five-zone editor (or the Library).
//! 2. The floating Capture HUD, a borderless always-on-top immediate
//!    viewport.
//! 3. The crosshair overlay over the frozen desktop, a fullscreen immediate
//!    viewport (see [`crate::capture_flow`]).
//!
//! The app owns the engine [`Session`] and the UI [`AppState`]; drawing is
//! `capturecraft-ui-egui`'s, every edit is an engine command run through
//! [`dispatch`], and the editor canvas is composited by the wgpu
//! compositor on eframe's own device and queue (`GpuCanvas`).

use std::time::Instant;

use capturecraft_capture::{CaptureBackend, HitTestOptions, backends};
use capturecraft_engine::Session;
use capturecraft_format::CaptureMode;
use capturecraft_ui_egui::panels::canvas_viewport::{CanvasViewport, GpuCanvas};
use capturecraft_ui_egui::panels::right_sidebar::RightSidebar;
use capturecraft_ui_egui::state::{AppState, SelectionSource};
use capturecraft_ui_egui::theme::Tokens;
use capturecraft_ui_egui::views::crosshair_overlay::{
    CrosshairOverlay, DesktopMapping, DesktopTextures, OverlayAction,
};
use capturecraft_ui_egui::{Dispatched, UiIntent, dispatch};
use eframe::egui_wgpu;
use egui::{ViewportBuilder, ViewportCommand, ViewportId};

use crate::capture_flow::{CaptureFlow, open_capture};

/// eframe storage key of [`AppState`].
pub const STATE_KEY: &str = "capturecraft.ui";

pub fn hud_viewport_id() -> ViewportId {
    ViewportId::from_hash_of("capture_hud")
}

/// The HUD window: small, borderless, above other windows.
pub fn hud_viewport(state: &AppState) -> ViewportBuilder {
    let mut b = ViewportBuilder::default()
        .with_title("CaptureCraft Capture")
        .with_decorations(false)
        .with_resizable(false)
        .with_transparent(true)
        .with_inner_size([420.0, 64.0]);
    if state.hud.window.always_on_top {
        b = b.with_always_on_top();
    }
    if let Some(p) = state.hud.window.position {
        b = b.with_position(p);
    }
    b
}

pub struct CaptureCraftApp {
    session: Session,
    state: AppState,
    /// Name of the theme last applied (re-applied when the sidebar
    /// changes it).
    applied_theme: Option<String>,
    render_state: Option<egui_wgpu::RenderState>,
    canvas: Option<GpuCanvas>,
    backend: Option<Box<dyn CaptureBackend>>,
    flow: CaptureFlow,
    textures: Option<DesktopTextures>,
    hud_open: bool,
    /// One-line status (failed commands, capture errors).
    notice: Option<String>,
}

impl CaptureCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        photocraft_ui_egui::theme::install_fonts(&cc.egui_ctx);
        let state: AppState = cc
            .storage
            .and_then(|s| eframe::get_value(s, STATE_KEY))
            .unwrap_or_default();
        let render_state = cc.wgpu_render_state.clone();
        let (canvas, notice) = match render_state.as_ref().map(GpuCanvas::new) {
            Some(Ok(c)) => (Some(c), None),
            Some(Err(e)) => (None, Some(format!("GPU canvas unavailable: {e}"))),
            None => (
                None,
                Some("No wgpu renderer: the canvas is disabled".into()),
            ),
        };
        Self {
            session: Session::new(),
            state,
            applied_theme: None,
            render_state,
            canvas,
            backend: backends::default_backend(),
            flow: CaptureFlow::default(),
            textures: None,
            hud_open: true,
            notice,
        }
    }

    fn tokens(&self) -> Tokens {
        Tokens::from_name(&self.state.editor.sidebar.theme)
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let name = &self.state.editor.sidebar.theme;
        if self.applied_theme.as_ref() != Some(name) {
            Tokens::from_name(name).apply(ctx);
            self.applied_theme = Some(name.clone());
        }
    }

    fn start_capture(&mut self, ctx: &egui::Context) {
        if self.backend.is_none() {
            self.notice = Some("No capture backend for this platform".into());
            return;
        }
        if self.flow.start(Instant::now()) {
            self.state.overlay = Default::default();
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(false));
            ctx.request_repaint();
        }
    }

    /// Freezes the desktop once our windows are gone.
    fn advance_capture(&mut self, ctx: &egui::Context) {
        if !self.flow.ready_to_freeze(Instant::now()) {
            if self.flow.hides_windows() {
                ctx.request_repaint();
            }
            return;
        }
        let opts = HitTestOptions {
            exclude_pids: vec![std::process::id()],
            ..HitTestOptions::default()
        };
        let frozen = match self.backend.as_ref().map(|b| b.freeze(&opts)) {
            Some(Ok(d)) => d,
            Some(Err(e)) => return self.finish_capture(ctx, Some(format!("Capture failed: {e}"))),
            None => return self.finish_capture(ctx, Some("No capture backend".into())),
        };
        self.textures = Some(DesktopTextures::load(ctx, &frozen));
        self.flow = CaptureFlow::Overlay(Box::new(frozen));
        ctx.request_repaint();
    }

    fn finish_capture(&mut self, ctx: &egui::Context, notice: Option<String>) {
        self.flow = CaptureFlow::Idle;
        self.textures = None;
        self.state.overlay = Default::default();
        if notice.is_some() {
            self.notice = notice;
        }
        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(true));
        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
    }

    fn on_overlay_action(&mut self, ctx: &egui::Context, action: OverlayAction) {
        let Some(desktop) = self.flow.desktop() else {
            return;
        };
        let (rect, source) = match action {
            OverlayAction::Cancel => return self.finish_capture(ctx, None),
            OverlayAction::RecordVideo { .. } | OverlayAction::Panorama { .. } => {
                return self.finish_capture(
                    ctx,
                    Some("Video and scrolling capture are not available yet".into()),
                );
            }
            OverlayAction::CaptureImage { rect, source } => (rect, source),
        };
        let (mode, window) = match source {
            SelectionSource::Region => (CaptureMode::Region, None),
            SelectionSource::Window { id, .. } => (
                CaptureMode::Window,
                desktop.index().windows().iter().find(|w| w.id == id),
            ),
            SelectionSource::Display { .. } => (CaptureMode::Fullscreen, None),
        };
        let notice = match desktop
            .crop(rect, mode, window)
            .map_err(|e| e.to_string())
            .and_then(|p| open_capture(&mut self.session, p))
        {
            Ok(_) => None,
            Err(e) => Some(format!("Capture failed: {e}")),
        };
        self.finish_capture(ctx, notice);
    }

    fn run_intents(&mut self, ctx: &egui::Context, intents: Vec<UiIntent>) {
        for d in dispatch(&mut self.session, intents) {
            match d {
                Dispatched::Ran { .. } => {}
                Dispatched::Failed { id, error } => self.notice = Some(format!("{id}: {error}")),
                Dispatched::Shell(UiIntent::NewCapture) => self.start_capture(ctx),
                Dispatched::Shell(other) => {
                    self.notice = Some(format!("Not available yet: {other:?}"));
                }
            }
        }
    }

    fn show_hud(&mut self, ctx: &egui::Context) -> Vec<UiIntent> {
        if !self.hud_open || self.flow.hides_windows() {
            return Vec::new();
        }
        let t = self.tokens();
        let builder = hud_viewport(&self.state);
        let mut intents = Vec::new();
        let close = ctx.show_viewport_immediate(hud_viewport_id(), builder, |ui, _| {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(t.panel)
                        .corner_radius(Tokens::corner(t.radius_lg))
                        .inner_margin(t.space_md),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let capture = egui::Button::new(
                            egui::RichText::new("Capture")
                                .color(t.accent_capture_text)
                                .strong(),
                        )
                        .fill(t.accent_capture);
                        if ui.add(capture).clicked() {
                            intents.push(UiIntent::NewCapture);
                        }
                    });
                });
            ui.input(|i| i.viewport().close_requested())
        });
        if close {
            self.hud_open = false;
        }
        intents
    }

    fn show_overlay(&mut self, ctx: &egui::Context) {
        let (Some(desktop), Some(textures)) = (self.flow.desktop(), self.textures.as_ref()) else {
            return;
        };
        let t = self.tokens();
        let mapping = DesktopMapping::new(
            desktop.bounds(),
            cfg!(target_os = "macos"),
            ctx.pixels_per_point(),
        );
        let overlay = CrosshairOverlay {
            desktop,
            textures,
            mapping,
            prefs: &self.state.crosshair,
            tokens: &t,
        };
        if let Some(action) = overlay.show_viewport(ctx, &mut self.state.overlay) {
            self.on_overlay_action(ctx, action);
        }
    }

    /// The editor window's zones.
    fn show_editor(&mut self, ui: &mut egui::Ui) -> Vec<UiIntent> {
        let t = self.tokens();
        let mut intents = Vec::new();
        let image = match (
            self.session.active(),
            self.render_state.as_ref(),
            self.canvas.as_mut(),
        ) {
            (Some(doc), Some(rs), Some(canvas)) => {
                let scene = capturecraft_engine::render::render_scene(doc);
                match canvas.render(rs, &scene) {
                    Ok(img) => Some(img),
                    Err(e) => {
                        self.notice = Some(format!("Canvas: {e}"));
                        None
                    }
                }
            }
            _ => None,
        };
        egui::Panel::top("capturecraft.toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("New Capture").clicked() {
                    intents.push(UiIntent::NewCapture);
                }
                if ui.button("Undo").clicked() {
                    intents.push(UiIntent::command("edit.undo", serde_json::json!({})));
                }
                if ui.button("Redo").clicked() {
                    intents.push(UiIntent::command("edit.redo", serde_json::json!({})));
                }
                if let Some(n) = &self.notice {
                    ui.label(egui::RichText::new(n).color(t.text_dim));
                }
            });
        });
        let doc = self.session.active();
        egui::Panel::right("capturecraft.sidebar").show(ui, |ui| {
            RightSidebar { doc, tokens: &t }.show(ui, &mut self.state.editor, &mut intents);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            CanvasViewport {
                doc,
                image,
                tokens: &t,
            }
            .show(ui, &mut self.state.editor, &mut intents);
        });
        intents
    }
}

impl eframe::App for CaptureCraftApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_theme(ctx);
        self.advance_capture(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut intents = self.show_hud(&ctx);
        self.show_overlay(&ctx);
        if !self.flow.hides_windows() {
            intents.extend(self.show_editor(ui));
        }
        self.run_intents(&ctx, intents);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STATE_KEY, &self.state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hud_is_a_borderless_floating_window() {
        let state = AppState::default();
        let b = hud_viewport(&state);
        assert_eq!(b.decorations, Some(false));
        assert_eq!(b.window_level, Some(egui::WindowLevel::AlwaysOnTop));
        assert_eq!(b.transparent, Some(true));
    }

    #[test]
    fn the_hud_follows_the_always_on_top_preference() {
        let mut state = AppState::default();
        state.hud.window.always_on_top = false;
        assert_eq!(hud_viewport(&state).window_level, None);
    }
}
