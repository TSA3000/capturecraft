//! [`CaptureCraftApp`]: the `eframe::App` hosting CaptureCraft's three
//! viewports on one wgpu device.
//!
//! 1. The root viewport: the five-zone editor (or the Library),
//!    `capturecraft-ui-egui`'s `EditorWindow`.
//! 2. The floating Capture HUD, a borderless always-on-top immediate
//!    viewport (`CaptureHudView`).
//! 3. The crosshair overlay over the frozen desktop, a fullscreen immediate
//!    viewport (see [`crate::capture_flow`]).
//!
//! The app owns the engine [`Session`] and the UI [`AppState`]; drawing is
//! `capturecraft-ui-egui`'s, every edit is an engine command run through
//! [`dispatch`], and the editor canvas is composited by the wgpu
//! compositor on eframe's own device and queue (`GpuCanvas`).

use std::time::{Duration, Instant};

use capturecraft_capture::{CaptureBackend, HitTestOptions, backends};
use capturecraft_engine::Session;
use capturecraft_format::CaptureMode;
use capturecraft_ui_egui::panels::canvas_viewport::GpuCanvas;
use capturecraft_ui_egui::state::{AppState, SelectionSource};
use capturecraft_ui_egui::theme::Tokens;
use capturecraft_ui_egui::views::capture_hud::CaptureHudView;
use capturecraft_ui_egui::views::crosshair_overlay::{
    CrosshairOverlay, DesktopMapping, DesktopTextures, OverlayAction,
};
use capturecraft_ui_egui::views::editor_window::EditorWindow;
use capturecraft_ui_egui::{Dispatched, UiIntent, dispatch};
use eframe::egui_wgpu;
use egui::{ViewportCommand, ViewportId};

use crate::capture_flow::{CaptureFlow, open_capture};

/// eframe storage key of [`AppState`].
pub const STATE_KEY: &str = "capturecraft.ui";

/// What the shell does with an intent the engine does not run.
#[derive(Debug, PartialEq)]
pub enum ShellAction {
    /// Start a capture after `delay`.
    Capture {
        delay: Duration,
    },
    HideHud,
    ShowEditor,
    /// Not wired up yet; shown as a notice.
    Unavailable(&'static str),
}

pub fn shell_action(intent: &UiIntent) -> ShellAction {
    match intent {
        UiIntent::NewCapture => ShellAction::Capture {
            delay: Duration::ZERO,
        },
        UiIntent::StartCapture { delay_secs, .. } => ShellAction::Capture {
            delay: Duration::from_secs(u64::from(*delay_secs)),
        },
        UiIntent::CloseHud => ShellAction::HideHud,
        UiIntent::ShowEditor => ShellAction::ShowEditor,
        UiIntent::OpenSettings => ShellAction::Unavailable("Preferences"),
        UiIntent::CopyImage => ShellAction::Unavailable("Copy image"),
        UiIntent::Share => ShellAction::Unavailable("Share"),
        UiIntent::CombineImages { .. } => ShellAction::Unavailable("Combine Images"),
        UiIntent::DeleteDocuments { .. } => ShellAction::Unavailable("Delete from Library"),
        UiIntent::Command { .. } | UiIntent::ActivateDocument { .. } => {
            ShellAction::Unavailable("engine intent")
        }
    }
}

pub struct CaptureCraftApp {
    session: Session,
    state: AppState,
    render_state: Option<egui_wgpu::RenderState>,
    canvas: Option<GpuCanvas>,
    backend: Option<Box<dyn CaptureBackend>>,
    flow: CaptureFlow,
    textures: Option<DesktopTextures>,
    hud_open: bool,
    /// One-line status (failed commands, capture errors), drawn over the
    /// editor until the next one.
    notice: Option<String>,
}

impl CaptureCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
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

    fn start_capture(&mut self, ctx: &egui::Context, delay: Duration) {
        if self.backend.is_none() {
            self.notice = Some("No capture backend for this platform".into());
            return;
        }
        // A delayed capture counts its delay as part of the hiding time.
        if self.flow.start(Instant::now() + delay) {
            self.state.overlay = Default::default();
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(false));
            ctx.request_repaint();
        }
    }

    /// Freezes the desktop once our windows are gone.
    fn advance_capture(&mut self, ctx: &egui::Context) {
        if !self.flow.ready_to_freeze(Instant::now()) {
            if self.flow.hides_windows() {
                ctx.request_repaint_after(Duration::from_millis(50));
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
                Dispatched::Shell(intent) => match shell_action(&intent) {
                    ShellAction::Capture { delay } => self.start_capture(ctx, delay),
                    ShellAction::HideHud => self.hud_open = false,
                    ShellAction::ShowEditor => {
                        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
                    }
                    ShellAction::Unavailable(what) => {
                        self.notice = Some(format!("{what} is not available yet"));
                    }
                },
            }
        }
    }

    fn show_hud(&mut self, ctx: &egui::Context) -> Vec<UiIntent> {
        if !self.hud_open || self.flow.hides_windows() {
            return Vec::new();
        }
        let t = self.tokens();
        CaptureHudView { tokens: &t }.show_viewport(ctx, &mut self.state.hud)
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

    fn show_editor(&mut self, ui: &mut egui::Ui) -> Vec<UiIntent> {
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
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        // Thumbnails are not loaded yet: the tray and Library show
        // placeholders.
        let intents = EditorWindow {
            session: &self.session,
            image,
            thumbnail: &|_| None,
            today: &today,
        }
        .show(ui, &mut self.state);
        if let Some(n) = &self.notice {
            let t = self.tokens();
            egui::Area::new(egui::Id::new("capturecraft.notice"))
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -72.0])
                .show(ui.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(t.badge_bg)
                        .corner_radius(Tokens::corner(t.radius))
                        .inner_margin(t.space_md)
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(n).color(t.badge_text))
                        });
                });
        }
        intents
    }
}

impl eframe::App for CaptureCraftApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
    use capturecraft_ui_egui::state::{CaptureTab, SelectionType};

    #[test]
    fn capture_intents_start_a_capture_with_their_delay() {
        assert_eq!(
            shell_action(&UiIntent::NewCapture),
            ShellAction::Capture {
                delay: Duration::ZERO
            }
        );
        let hud = UiIntent::StartCapture {
            tab: CaptureTab::Image,
            selection: SelectionType::default(),
            delay_secs: 3,
        };
        assert_eq!(
            shell_action(&hud),
            ShellAction::Capture {
                delay: Duration::from_secs(3)
            }
        );
    }

    #[test]
    fn window_intents_map_to_window_actions() {
        assert_eq!(shell_action(&UiIntent::CloseHud), ShellAction::HideHud);
        assert_eq!(shell_action(&UiIntent::ShowEditor), ShellAction::ShowEditor);
        assert!(matches!(
            shell_action(&UiIntent::Share),
            ShellAction::Unavailable(_)
        ));
    }
}
