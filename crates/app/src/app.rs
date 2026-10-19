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

use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use capturecraft_capture::{CaptureBackend, HitTestOptions, backends};
use capturecraft_engine::Session;
use capturecraft_engine::render::{Flattened, flatten, render_scene};
use capturecraft_format::CaptureMode;
use capturecraft_ui_egui::panels::canvas_viewport::{CanvasImage, CpuCanvas, GpuCanvas};
use capturecraft_ui_egui::panels::gizmo::backdrop_intent;
use capturecraft_ui_egui::panels::recent_tray::{Thumbnail, sync_tray};
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
use capturecraft_ui_egui::views::editor_window::{EditorWindow, document_file_name, window_title};
use capturecraft_ui_egui::{Dispatched, UiIntent, dispatch};
use eframe::egui_wgpu;
use egui::{ViewportCommand, ViewportId};

use crate::capture_flow::{CaptureFlow, open_capture};
use crate::documents::{self, index_of, open_blank, open_image};
use crate::export;
use crate::file_dialog::{self, Answer, FileDialogs, Request};
use crate::library::{self, LibraryFiles};

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
    /// Close documents (the tray's ×, Delete: nothing is on disk yet).
    Close(Vec<String>),
    /// Open an image file (the Library's captures folder entries).
    Open(PathBuf),
    Duplicate(String),
    CopyDocument(String),
    NewBlank,
    /// Show a message in the toast.
    Notice(String),
    /// Save to the document's file (Save As when it has none); `None` =
    /// the active document.
    Save(Option<String>),
    SaveAs(Option<String>),
    OpenDialog,
    /// Run `ocr.grabText` and open the Grab Text modal.
    GrabText(Option<serde_json::Value>),
    CopyText(String),
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
        UiIntent::DeleteDocuments { doc_ids } | UiIntent::CloseDocuments { doc_ids } => {
            ShellAction::Close(doc_ids.clone())
        }
        UiIntent::DuplicateDocument { doc_id } => ShellAction::Duplicate(doc_id.clone()),
        UiIntent::CopyDocument { doc_id } => ShellAction::CopyDocument(doc_id.clone()),
        UiIntent::SaveAs { doc_id } => ShellAction::SaveAs(doc_id.clone()),
        UiIntent::NewBlankImage => ShellAction::NewBlank,
        UiIntent::OpenFile { path } => ShellAction::Open(PathBuf::from(path)),
        UiIntent::Unavailable(what) => ShellAction::Unavailable(what),
        UiIntent::Notice(text) => ShellAction::Notice(text.clone()),
        UiIntent::GrabText { region } => ShellAction::GrabText(region.clone()),
        UiIntent::CopyText(text) => ShellAction::CopyText(text.clone()),
        UiIntent::Save { doc_id } => ShellAction::Save(doc_id.clone()),
        UiIntent::OpenDialog => ShellAction::OpenDialog,
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
    /// Recent Captures thumbnails by document id, with the content key
    /// they were drawn from.
    thumbnails: HashMap<String, (u64, Thumbnail, egui::TextureHandle)>,
    /// The captures folders' image files the Library lists.
    library: LibraryFiles,
    /// The tray app's output folder (its config.toml), read at start.
    tray_dir: Option<PathBuf>,
    /// Content key of each document when it was last saved or opened.
    saved: HashMap<String, u64>,
    /// Whether each document changed since then (checked while active).
    dirty: HashMap<String, bool>,
    /// Native Open / Save dialogs.
    dialogs: FileDialogs,
    /// The `.capturecraft` file of each saved or opened document, by id.
    paths: HashMap<String, PathBuf>,
    /// Folder of the last file opened or saved (where dialogs start).
    last_dir: Option<PathBuf>,
    /// The toast on screen and when it appeared.
    toast: Option<(String, Instant)>,
    /// The window title last sent.
    title: String,
}

/// What saved documents name as their generator.
const GENERATOR: &str = concat!("capturecraft-editor ", env!("CARGO_PKG_VERSION"));

/// How long a toast stays, seconds; the last half second fades it out.
pub const TOAST_SECS: f32 = 3.0;
const TOAST_FADE: f32 = 0.5;

/// Opacity of a toast `age` seconds old.
pub fn toast_alpha(age: f32) -> f32 {
    ((TOAST_SECS - age) / TOAST_FADE).clamp(0.0, 1.0)
}

/// Blank images start at this size.
const BLANK_SIZE: photocraft_geom::Size = photocraft_geom::Size::new(800, 600);

/// Changes whenever what a document looks like changes (its manifest or
/// its edited pixels).
fn content_key(doc: &capturecraft_engine::DocState) -> u64 {
    let mut h = DefaultHasher::new();
    serde_json::to_vec(&doc.manifest)
        .unwrap_or_default()
        .hash(&mut h);
    doc.edited_pixels().map(|p| p.revision()).hash(&mut h);
    h.finish()
}

impl CaptureCraftApp {
    /// The app with `open` (image files) as its first documents.
    pub fn new(cc: &eframe::CreationContext<'_>, open: &[PathBuf]) -> Self {
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
        let mut session = Session::new();
        session.set_analyzer(Box::new(capturecraft_ocr::Analyzer::auto()));
        let mut app = Self {
            session,
            state,
            render_state,
            canvas,
            cpu_canvas: CpuCanvas::default(),
            backend: backends::default_backend(),
            flow: CaptureFlow::default(),
            textures: None,
            pending: None,
            clipboard: None,
            // Opened with images (Open editor): straight to the editor.
            hud_open: open.is_empty(),
            notice,
            thumbnails: HashMap::new(),
            library: LibraryFiles::default(),
            tray_dir: library::tray_output_dir(),
            toast: None,
            dialogs: FileDialogs::default(),
            paths: HashMap::new(),
            saved: HashMap::new(),
            dirty: HashMap::new(),
            last_dir: None,
            title: String::new(),
        };
        for p in open {
            app.open_file(p);
        }
        app
    }

    /// Opens a `.capturecraft` document (with its history) or an image.
    /// A document that is already open is brought to the front instead.
    fn open_file(&mut self, path: &Path) {
        self.last_dir = path.parent().map(Path::to_path_buf);
        let is_doc = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(file_dialog::DOC_EXT));
        let opened = if is_doc {
            self.open_document(path)
        } else {
            open_image(&mut self.session, path).map(|_| ())
        };
        match opened {
            Ok(()) => {
                sync_tray(&mut self.state.editor.tray, &self.session);
                self.state.editor.library.visible = false;
                self.state.editor.canvas.selection.clear();
            }
            Err(e) => self.notice = Some(format!("Could not open {e}")),
        }
    }

    fn open_document(&mut self, path: &Path) -> Result<(), String> {
        if let Some(id) = self
            .paths
            .iter()
            .find(|(_, p)| p.as_path() == path)
            .map(|(id, _)| id.clone())
            && let Some(i) = index_of(&self.session, &id)
        {
            return self.session.activate(i).map_err(|e| e.to_string());
        }
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let i = self
            .session
            .open_archive(&bytes)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        // Two copies of one document get their own ids.
        let id = self.session.documents()[i].manifest.id.clone();
        let taken = self
            .session
            .documents()
            .iter()
            .filter(|d| d.manifest.id == id)
            .count()
            > 1;
        if taken || id.is_empty() {
            let fresh = documents::unique_doc_id(&self.session);
            if let Some(d) = self.session.active_mut() {
                d.manifest.id = fresh;
            }
        }
        if let Some(d) = self.session.active() {
            self.paths.insert(d.manifest.id.clone(), path.to_path_buf());
            self.saved.insert(d.manifest.id.clone(), content_key(d));
        }
        Ok(())
    }

    /// Save (to the document's file, or Save As when it has none) or
    /// Save As.
    fn save(&mut self, id: Option<String>, ask: bool) {
        let Some(id) = id.or_else(|| self.session.active().map(|d| d.manifest.id.clone())) else {
            self.notice = Some("Nothing to save: no capture open".into());
            return;
        };
        match self.paths.get(&id).cloned() {
            Some(path) if !ask => self.write_document(&id, &path),
            known => {
                let Some(doc) =
                    index_of(&self.session, &id).and_then(|i| self.session.documents().get(i))
                else {
                    return;
                };
                let suggested = known.unwrap_or_else(|| {
                    let dir = self.last_dir.clone().unwrap_or_else(export::default_dir);
                    dir.join(document_file_name(&doc.manifest))
                });
                if !self.dialogs.ask(Request::Save {
                    doc_id: id,
                    suggested,
                }) {
                    self.notice = Some("A file dialog is already open".into());
                }
            }
        }
    }

    /// Writes document `id` to `path`: a `.capturecraft` archive, or a
    /// flattened PNG / JPEG / WebP for those extensions.
    fn write_document(&mut self, id: &str, path: &Path) {
        let Some(i) = index_of(&self.session, id) else {
            self.notice = Some("Save failed: the capture is closed".into());
            return;
        };
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let image = match ext.as_str() {
            "png" => Some(ImageFormat::Png),
            "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
            "webp" => Some(ImageFormat::Webp),
            _ => None,
        };
        let bytes = match image {
            Some(format) => flatten(&self.session.documents()[i])
                .map_err(|e| e.to_string())
                .and_then(|f| export::encode(&f, format)),
            None => self
                .session
                .save_archive(i, GENERATOR)
                .map_err(|e| e.to_string()),
        };
        let result = bytes.and_then(|b| file_dialog::atomic_write(path, &b));
        self.last_dir = path.parent().map(Path::to_path_buf);
        self.notice = Some(match result {
            Ok(()) => {
                if image.is_none() {
                    self.paths.insert(id.to_string(), path.to_path_buf());
                    if let Some(d) = self.session.documents().get(i) {
                        self.saved.insert(id.to_string(), content_key(d));
                    }
                }
                format!("Saved {}", path.display())
            }
            Err(e) => format!("Save failed: {e}"),
        });
    }

    /// Handles a finished Open or Save As dialog.
    fn on_dialog(&mut self, a: Answer) {
        match a {
            Answer::Open(paths) => {
                for p in paths {
                    self.open_file(&p);
                }
            }
            Answer::Save { doc_id, path } => self.write_document(&doc_id, &path),
        }
    }

    /// The tray's file paths and unsaved dots follow the saved files
    /// (only the active document can have changed since last frame).
    fn sync_files(&mut self) {
        if let Some(d) = self.session.active() {
            let id = &d.manifest.id;
            let dirty = self.saved.get(id).is_none_or(|k| *k != content_key(d));
            self.dirty.insert(id.clone(), dirty);
        }
        for item in &mut self.state.editor.tray.items {
            if let Some(p) = self.paths.get(&item.doc_id) {
                item.file_path = Some(p.to_string_lossy().into_owned());
            }
            item.unsaved = self
                .dirty
                .get(&item.doc_id)
                .copied()
                .unwrap_or(!self.saved.contains_key(&item.doc_id));
        }
    }

    /// Closes the documents `ids` and forgets their thumbnails.
    fn close_documents(&mut self, ids: &[String]) {
        for id in ids {
            if let Some(i) = index_of(&self.session, id) {
                let _ = self.session.close(i);
            }
            self.thumbnails.remove(id);
        }
        self.state.editor.canvas.selection.clear();
        self.state.editor.canvas.marquee = None;
        sync_tray(&mut self.state.editor.tray, &self.session);
    }

    /// Uploads thumbnails of new documents and of the active one when it
    /// changed (not mid-drag: flattening a large capture takes a while).
    fn refresh_thumbnails(&mut self, ctx: &egui::Context) {
        let busy = ctx.input(|i| i.pointer.any_down());
        let active = self.session.active().map(|d| d.manifest.id.clone());
        for doc in self.session.documents() {
            let id = &doc.manifest.id;
            let have = self.thumbnails.get(id).map(|(k, ..)| *k);
            let stale = match have {
                None => true,
                Some(k) => !busy && active.as_ref() == Some(id) && k != content_key(doc),
            };
            if !stale {
                continue;
            }
            let Some((w, h, rgba)) = flatten(doc).ok().as_ref().and_then(documents::thumbnail)
            else {
                continue;
            };
            let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
            let tex = ctx.load_texture(
                format!("capturecraft.thumb.{id}"),
                img,
                egui::TextureOptions::LINEAR,
            );
            let th = Thumbnail {
                texture: tex.id(),
                size: [w, h],
            };
            self.thumbnails
                .insert(id.clone(), (content_key(doc), th, tex));
        }
        let open: Vec<&str> = self
            .session
            .documents()
            .iter()
            .map(|d| d.manifest.id.as_str())
            .collect();
        self.thumbnails.retain(|id, _| open.contains(&id.as_str()));
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

    /// Reads the text in `region` and shows it in the Grab Text modal.
    fn grab_text(&mut self, region: Option<serde_json::Value>) {
        let params = match region {
            Some(r) => serde_json::json!({ "region": r }),
            None => serde_json::json!({}),
        };
        match self.session.run("ocr.grabText", &params) {
            Ok(v) => {
                self.state.editor.grab =
                    Some(capturecraft_ui_egui::views::ocr_modal::GrabResult::from_value(&v));
            }
            Err(e) => self.notice = Some(format!("Grab Text: {e}")),
        }
    }

    fn copy_active(&mut self) {
        let id = self.session.active().map(|d| d.manifest.id.clone());
        match id {
            Some(id) => self.copy_document(&id),
            None => self.notice = Some("Copy failed: No capture open".into()),
        }
    }

    fn copy_document(&mut self, id: &str) {
        let flat = index_of(&self.session, id)
            .and_then(|i| self.session.documents().get(i))
            .ok_or_else(|| "the capture is closed".to_string())
            .and_then(|d| flatten(d).map_err(|e| e.to_string()));
        self.notice = Some(
            match flat.and_then(|f| export::copy(&mut self.clipboard, &f).map(|()| f)) {
                Ok(_) => "Copied to clipboard!".to_string(),
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
                    ShellAction::Close(ids) => self.close_documents(&ids),
                    ShellAction::Open(path) => self.open_file(&path),
                    ShellAction::Duplicate(id) => {
                        let new_id = documents::unique_doc_id(&self.session);
                        match index_of(&self.session, &id)
                            .map(|i| self.session.duplicate(i, &new_id))
                        {
                            Some(Ok(_)) => sync_tray(&mut self.state.editor.tray, &self.session),
                            Some(Err(e)) => self.notice = Some(format!("Duplicate failed: {e}")),
                            None => {
                                self.notice = Some("Duplicate failed: the capture is closed".into())
                            }
                        }
                    }
                    ShellAction::CopyDocument(id) => self.copy_document(&id),
                    ShellAction::NewBlank => match open_blank(&mut self.session, BLANK_SIZE) {
                        Ok(_) => {
                            sync_tray(&mut self.state.editor.tray, &self.session);
                            self.state.editor.library.visible = false;
                        }
                        Err(e) => self.notice = Some(format!("New image failed: {e}")),
                    },
                    ShellAction::Notice(text) => self.notice = Some(text),
                    ShellAction::GrabText(region) => self.grab_text(region),
                    ShellAction::Save(id) => self.save(id, false),
                    ShellAction::SaveAs(id) => self.save(id, true),
                    ShellAction::OpenDialog => {
                        let dir = self
                            .last_dir
                            .clone()
                            .or_else(|| Some(export::default_dir()));
                        if !self.dialogs.ask(Request::Open { dir }) {
                            self.notice = Some("A file dialog is already open".into());
                        }
                    }
                    ShellAction::CopyText(text) => {
                        self.notice = Some(match export::copy_text(&mut self.clipboard, &text) {
                            Ok(()) => "Text copied to clipboard!".into(),
                            Err(e) => format!("Copy failed: {e}"),
                        });
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
        self.sync_files();
        self.refresh_thumbnails(ui.ctx());
        let title = window_title(self.session.active());
        if title != self.title {
            ui.ctx()
                .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        let dirs = library::library_dirs(&self.state, self.tray_dir.clone());
        self.library
            .refresh(ui.ctx(), &dirs, self.state.editor.library.visible);
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let (thumbs, files) = (&self.thumbnails, &self.library);
        let thumbnail = |id: &str| {
            thumbs
                .get(id)
                .map(|(_, th, _)| *th)
                .or_else(|| files.thumbnail(id))
        };
        EditorWindow {
            session: &self.session,
            image,
            thumbnail: &thumbnail,
            today: &today,
            library_files: &self.library.items,
        }
        .show(ui, &mut self.state)
    }

    /// The latest notice as a toast at the bottom centre that fades out
    /// after [`TOAST_SECS`]; a new notice restarts it.
    fn show_toast(&mut self, ctx: &egui::Context) {
        if let Some(n) = self.notice.take() {
            self.toast = Some((n, Instant::now()));
        }
        let Some((text, since)) = &self.toast else {
            return;
        };
        let age = since.elapsed().as_secs_f32();
        let a = toast_alpha(age);
        if a <= 0.0 {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(if a < 1.0 { 16 } else { 200 }));
        let t = self.tokens();
        egui::Area::new(egui::Id::new("capturecraft.notice"))
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -72.0])
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_opacity(a);
                egui::Frame::new()
                    .fill(t.badge_bg)
                    .corner_radius(Tokens::corner(t.radius))
                    .inner_margin(t.space_md)
                    .shadow(egui::Shadow {
                        offset: [0, 2],
                        blur: 8,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(80),
                    })
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(text.as_str()).color(t.badge_text))
                    });
            });
    }
}

impl eframe::App for CaptureCraftApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.advance_capture(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(a) = self.dialogs.poll(&ctx, Some(&*frame)) {
            self.on_dialog(a);
        }
        let mut intents = self.show_hud(&ctx);
        self.show_overlay(&ctx);
        if !self.flow.hides_windows() {
            intents.extend(self.show_editor(ui));
        }
        self.run_intents(&ctx, intents);
        if !self.flow.hides_windows() {
            self.show_toast(&ctx);
        }
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
    fn toasts_stay_then_fade() {
        assert_eq!(toast_alpha(0.0), 1.0);
        assert_eq!(toast_alpha(TOAST_SECS - 0.5), 1.0);
        assert!((toast_alpha(TOAST_SECS - 0.25) - 0.5).abs() < 1e-6);
        assert_eq!(toast_alpha(TOAST_SECS + 1.0), 0.0);
    }

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
        assert_eq!(
            shell_action(&UiIntent::CloseDocuments {
                doc_ids: vec!["a".into()]
            }),
            ShellAction::Close(vec!["a".into()])
        );
        assert_eq!(
            shell_action(&UiIntent::DeleteDocuments {
                doc_ids: vec!["b".into()]
            }),
            ShellAction::Close(vec!["b".into()])
        );
        assert_eq!(
            shell_action(&UiIntent::NewBlankImage),
            ShellAction::NewBlank
        );
        assert_eq!(
            shell_action(&UiIntent::Unavailable("Assets")),
            ShellAction::Unavailable("Assets")
        );
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
