//! [`CaptureCraftApp`]: the `eframe::App` hosting CaptureCraft's three
//! viewports on one wgpu device.
//!
//! 1. The root viewport: the five-zone editor (or the Library),
//!    `capturecraft-ui-egui`'s `EditorWindow`.
//! 2. The floating Capture HUD, a borderless always-on-top immediate
//!    viewport (`CaptureHudView`).
//! 3. The crosshair overlay over the frozen desktop, a borderless
//!    always-on-top immediate viewport spanning every display (see
//!    [`crate::capture_flow`]).
//!
//! The app owns the engine [`Session`] and the UI [`AppState`]; drawing is
//! `capturecraft-ui-egui`'s, every edit is an engine command run through
//! [`dispatch`], and the editor canvas is composited by the wgpu
//! compositor on eframe's own device and queue (`GpuCanvas`), or by the
//! CPU compositor when no usable GPU device exists (`CpuCanvas`).
//!
//! A finished capture opens as a new document, gets the HUD's Effects
//! (a canvas backdrop command, so it is undoable), goes to the HUD's Share
//! destination, appears in front of the Recent Captures tray and the
//! editor comes to the front.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use capturecraft_capture::{CaptureBackend, HitTestOptions, backends};
use capturecraft_engine::Session;
use capturecraft_engine::render::{Flattened, flatten, render_scene};
use capturecraft_format::CaptureMode;
use capturecraft_ui_egui::panels::canvas_viewport::{CanvasImage, CpuCanvas, GpuCanvas};
use capturecraft_ui_egui::panels::gizmo::backdrop_intent;
use capturecraft_ui_egui::panels::recent_tray::sync_tray;
use capturecraft_ui_egui::state::{
    AppState, CaptureSettings, CaptureTab, CrosshairOverlayState, ImageFormat, SelectionSource,
    ShareDestination,
};
use capturecraft_ui_egui::theme::Tokens;
use capturecraft_ui_egui::views::capture_hud::{
    CaptureHudView, effects_backdrop, unsupported_effects,
};
use capturecraft_ui_egui::views::crosshair_overlay::{
    CrosshairOverlay, DesktopMapping, DesktopTextures, OverlayAction,
};
use capturecraft_ui_egui::views::editor_window::EditorWindow;
use capturecraft_ui_egui::{Dispatched, UiIntent, dispatch};
use eframe::egui_wgpu;
use egui::{ViewportCommand, ViewportId};

use crate::capture_flow::{CaptureFlow, open_capture};
use crate::export;

/// eframe storage key of [`AppState`].
pub const STATE_KEY: &str = "capturecraft.ui";

/// File name of an export when the HUD's pattern gives none.
const DEFAULT_PATTERN: &str = "Capture %Y-%m-%d %H%M%S";

/// What the shell does with an intent the engine does not run.
#[derive(Debug, PartialEq)]
pub enum ShellAction {
    /// Start a capture with `tab`'s settings after `delay`.
    Capture {
        tab: Option<CaptureTab>,
        delay: Duration,
    },
    HideHud,
    ShowEditor,
    /// Copy All: the flattened active document to the clipboard.
    Copy,
    /// Share / Export to the captures folder.
    Export(ImageFormat),
    /// Not wired up yet; shown as a notice.
    Unavailable(&'static str),
}

pub fn shell_action(intent: &UiIntent) -> ShellAction {
    match intent {
        UiIntent::NewCapture => ShellAction::Capture {
            tab: None,
            delay: Duration::ZERO,
        },
        UiIntent::StartCapture {
            tab, delay_secs, ..
        } => ShellAction::Capture {
            tab: Some(*tab),
            delay: Duration::from_secs(u64::from(*delay_secs)),
        },
        UiIntent::CloseHud => ShellAction::HideHud,
        UiIntent::ShowEditor => ShellAction::ShowEditor,
        UiIntent::CopyImage => ShellAction::Copy,
        UiIntent::Export { format } => ShellAction::Export(*format),
        UiIntent::OpenSettings => ShellAction::Unavailable("Preferences"),
        UiIntent::CombineImages { .. } => ShellAction::Unavailable("Combine Images"),
        UiIntent::DeleteDocuments { .. } => ShellAction::Unavailable("Delete from Library"),
        UiIntent::Command { .. } | UiIntent::ActivateDocument { .. } => {
            ShellAction::Unavailable("engine intent")
        }
    }
}

/// An export file name from a `chrono` pattern; an invalid or empty
/// pattern falls back to [`DEFAULT_PATTERN`] instead of panicking.
pub fn file_stem<Tz: chrono::TimeZone>(pattern: &str, now: &chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let expand = |p: &str| {
        let mut s = String::new();
        write!(s, "{}", now.format(p)).ok().map(|()| s)
    };
    match expand(pattern) {
        Some(s) if !s.trim().is_empty() => s,
        _ => expand(DEFAULT_PATTERN).unwrap_or_else(|| "Capture".into()),
    }
}

/// The folder a Share › File destination names; empty = the default.
pub fn share_dir(directory: &str) -> PathBuf {
    if directory.trim().is_empty() {
        export::default_dir()
    } else {
        PathBuf::from(directory)
    }
}

pub struct CaptureCraftApp {
    session: Session,
    state: AppState,
    render_state: Option<egui_wgpu::RenderState>,
    canvas: Option<GpuCanvas>,
    /// Draws the canvas when there is no usable GPU canvas.
    cpu_canvas: CpuCanvas,
    backend: Option<Box<dyn CaptureBackend>>,
    flow: CaptureFlow,
    textures: Option<DesktopTextures>,
    /// The HUD tab settings the running capture started with.
    pending: Option<CaptureSettings>,
    /// Kept open: on Linux the clipboard empties when its owner goes.
    clipboard: Option<arboard::Clipboard>,
    hud_open: bool,
    /// One-line status (failed commands, capture errors, saved files),
    /// drawn over the editor until the next one.
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
            Some(Err(e)) => (
                None,
                Some(format!("GPU canvas unavailable ({e}); drawing on the CPU")),
            ),
            None => (None, None),
        };
        Self {
            session: Session::new(),
            state,
            render_state,
            canvas,
            cpu_canvas: CpuCanvas::default(),
            backend: backends::default_backend(),
            flow: CaptureFlow::default(),
            textures: None,
            pending: None,
            clipboard: None,
            hud_open: true,
            notice,
        }
    }

    fn tokens(&self) -> Tokens {
        Tokens::from_name(&self.state.editor.sidebar.theme)
    }

    fn start_capture(&mut self, ctx: &egui::Context, tab: Option<CaptureTab>, delay: Duration) {
        if self.backend.is_none() {
            self.notice = Some("No capture backend for this platform".into());
            return;
        }
        let tab = tab.unwrap_or(self.state.hud.tab);
        // A delayed capture counts its delay as part of the hiding time.
        if self.flow.start(Instant::now() + delay) {
            let settings = match tab {
                CaptureTab::AllInOne => &self.state.hud.all_in_one,
                CaptureTab::Image => &self.state.hud.image,
                CaptureTab::Video => &self.state.hud.video.capture,
            };
            self.pending = Some(settings.clone());
            // The Image tab captures on release; All-in-One shows the
            // action bar (video, panorama).
            self.state.overlay = CrosshairOverlayState {
                capture_on_select: tab == CaptureTab::Image,
                ..CrosshairOverlayState::default()
            };
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
        self.pending = None;
        self.state.overlay = CrosshairOverlayState::default();
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
        let opened = desktop
            .crop(rect, mode, window)
            .map_err(|e| e.to_string())
            .and_then(|p| open_capture(&mut self.session, p));
        let settings = self.pending.take();
        match opened {
            Ok(_) => {
                self.finish_capture(ctx, None);
                // The new document is active: it leads the tray.
                sync_tray(&mut self.state.editor.tray, &self.session);
                self.state.editor.library.visible = false;
                if let Some(s) = settings {
                    self.after_capture(ctx, &s);
                }
            }
            Err(e) => self.finish_capture(ctx, Some(format!("Capture failed: {e}"))),
        }
    }

    /// The HUD's Effects and Share destination for the capture just opened.
    fn after_capture(&mut self, ctx: &egui::Context, s: &CaptureSettings) {
        if let Some(b) = effects_backdrop(&s.effects) {
            self.run_intents(ctx, vec![backdrop_intent(&b)]);
        }
        let missing = unsupported_effects(&s.effects);
        if !missing.is_empty() {
            self.notice = Some(format!("{} is not available yet", missing.join(", ")));
        }
        match &s.share {
            ShareDestination::Editor => {}
            ShareDestination::Clipboard => self.copy_active(),
            ShareDestination::File {
                directory,
                name_pattern,
                format,
            } => {
                let stem = file_stem(name_pattern, &chrono::Local::now());
                self.export(*format, Some(share_dir(directory)), Some(stem));
            }
        }
        if s.copy_to_clipboard && s.share != ShareDestination::Clipboard {
            self.copy_active();
        }
    }

    fn flatten_active(&self) -> Result<Flattened, String> {
        let doc = self.session.active().ok_or("No capture open")?;
        flatten(doc).map_err(|e| e.to_string())
    }

    fn copy_active(&mut self) {
        self.notice = Some(
            match self
                .flatten_active()
                .and_then(|f| export::copy(&mut self.clipboard, &f).map(|()| f))
            {
                Ok(f) => format!("Copied {} × {} px", f.width, f.height),
                Err(e) => format!("Copy failed: {e}"),
            },
        );
    }

    /// Writes the active document (a PDF: every open one) as `format`.
    fn export(&mut self, format: ImageFormat, dir: Option<PathBuf>, stem: Option<String>) {
        let bytes = if format == ImageFormat::Pdf {
            self.session
                .documents()
                .iter()
                .map(|d| flatten(d).map_err(|e| e.to_string()))
                .collect::<Result<Vec<_>, _>>()
                .and_then(|pages| export::pdf(&pages))
        } else {
            self.flatten_active()
                .and_then(|f| export::encode(&f, format))
        };
        let stem =
            stem.unwrap_or_else(
                || match self.session.active().map(|d| d.manifest.title.trim()) {
                    Some(t) if !t.is_empty() => t.to_string(),
                    _ => file_stem(DEFAULT_PATTERN, &chrono::Local::now()),
                },
            );
        let dir = dir.unwrap_or_else(export::default_dir);
        self.notice = Some(
            match bytes.and_then(|b| export::write_new(&dir, &stem, format, &b)) {
                Ok(path) => format!("Saved {}", path.display()),
                Err(e) => format!("{} export failed: {e}", format.label()),
            },
        );
    }

    fn run_intents(&mut self, ctx: &egui::Context, intents: Vec<UiIntent>) {
        for d in dispatch(&mut self.session, intents) {
            match d {
                Dispatched::Ran { .. } => {}
                Dispatched::Failed { id, error } => self.notice = Some(format!("{id}: {error}")),
                Dispatched::Shell(intent) => match shell_action(&intent) {
                    ShellAction::Capture { tab, delay } => {
                        self.hud_open = true;
                        self.start_capture(ctx, tab, delay);
                    }
                    ShellAction::HideHud => self.hud_open = false,
                    ShellAction::ShowEditor => {
                        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
                    }
                    ShellAction::Copy => self.copy_active(),
                    ShellAction::Export(format) => self.export(format, None, None),
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

    /// The active document on the GPU canvas, or on the CPU when the GPU
    /// canvas is missing or fails (it is then dropped for good).
    fn render_canvas(&mut self, ctx: &egui::Context) -> Option<CanvasImage> {
        let doc = self.session.active()?;
        let scene = render_scene(doc);
        if let (Some(rs), Some(canvas)) = (self.render_state.as_ref(), self.canvas.as_mut()) {
            match canvas.render(rs, &scene) {
                Ok(img) => return Some(img),
                Err(e) => {
                    canvas.free(rs);
                    self.canvas = None;
                    self.notice = Some(format!("GPU canvas failed ({e}); drawing on the CPU"));
                }
            }
        }
        self.cpu_canvas.render(ctx, &scene)
    }

    fn show_editor(&mut self, ui: &mut egui::Ui) -> Vec<UiIntent> {
        let image = self.render_canvas(ui.ctx());
        sync_tray(&mut self.state.editor.tray, &self.session);
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

    fn on_exit(&mut self) {
        if let (Some(rs), Some(canvas)) = (self.render_state.as_ref(), self.canvas.as_mut()) {
            canvas.free(rs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capturecraft_ui_egui::state::SelectionType;
    use chrono::TimeZone;

    #[test]
    fn capture_intents_start_a_capture_with_their_delay() {
        assert_eq!(
            shell_action(&UiIntent::NewCapture),
            ShellAction::Capture {
                tab: None,
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
                tab: Some(CaptureTab::Image),
                delay: Duration::from_secs(3)
            }
        );
    }

    #[test]
    fn window_and_output_intents_map_to_shell_actions() {
        assert_eq!(shell_action(&UiIntent::CloseHud), ShellAction::HideHud);
        assert_eq!(shell_action(&UiIntent::ShowEditor), ShellAction::ShowEditor);
        assert_eq!(shell_action(&UiIntent::CopyImage), ShellAction::Copy);
        assert_eq!(
            shell_action(&UiIntent::Export {
                format: ImageFormat::Pdf
            }),
            ShellAction::Export(ImageFormat::Pdf)
        );
        assert!(matches!(
            shell_action(&UiIntent::CombineImages { doc_ids: vec![] }),
            ShellAction::Unavailable(_)
        ));
    }

    #[test]
    fn file_names_follow_the_pattern_and_survive_bad_ones() {
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 10, 19, 14, 3, 11)
            .single()
            .unwrap_or_default();
        assert_eq!(
            file_stem("Capture %Y-%m-%d %H%M%S", &now),
            "Capture 2026-10-19 140311"
        );
        // An unknown specifier would panic in `format!`; it falls back.
        assert_eq!(file_stem("%Q oops", &now), "Capture 2026-10-19 140311");
        assert_eq!(file_stem("  ", &now), "Capture 2026-10-19 140311");
        assert_eq!(share_dir(""), export::default_dir());
        assert_eq!(share_dir("/tmp/x"), PathBuf::from("/tmp/x"));
    }
}
