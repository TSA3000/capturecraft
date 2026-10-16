//! Main window: a compact capture panel with the Capture button, hotkey,
//! clipboard and autostart settings. Changes are saved to config.toml
//! right away; a running tray picks them up within a few seconds.

use crate::config::{Config, SelectMode, config_dir, config_path};
use crate::prefs_ui::Prefs;
use crate::theme::{self, ThemeKind};
use crate::tray::{self, open_path};
use crate::{autostart, icon_rgba, sibling_exe};
use anyhow::{Result, anyhow};
use eframe::egui::{self, Color32, RichText, ViewportCommand};
use global_hotkey::hotkey::HotKey;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ACCENT: Color32 = Color32::from_rgb(0x3B, 0x82, 0xF6);
/// Always kept as second choice so a taken hotkey still leaves one working.
const FALLBACK: &str = "ctrl+shift+s";
pub const REPO: &str = "https://github.com/TSA3000/capturecraft";
pub const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));
const SETTINGS_WIDTH: f32 = 330.0;
const BUTTON_SIZE: egui::Vec2 = egui::vec2(130.0, 96.0);

/// Opens the window and blocks until it is closed.
/// If a window is already open, that one is brought to the front instead.
pub fn run() -> Result<()> {
    let Some(_lock) = instance_lock() else {
        focus_existing();
        return Ok(());
    };
    let (rgba, width, height) = icon_rgba();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(window_title())
            .with_inner_size([560.0, 170.0])
            .with_resizable(false)
            // Own title bar: Windows always draws a maximize button in a
            // standard caption that has a minimize button.
            .with_decorations(false)
            .with_maximize_button(false)
            .with_icon(Arc::new(egui::IconData {
                rgba,
                width,
                height,
            })),
        ..Default::default()
    };
    eframe::run_native(
        "CaptureCraft",
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
    .map_err(|e| anyhow!("window: {e}"))
}

/// Exclusive lock on `<config dir>/app.lock`, held while the window is open.
fn instance_lock() -> Option<std::fs::File> {
    let _ = std::fs::create_dir_all(config_dir());
    let file = std::fs::File::create(config_dir().join("app.lock")).ok()?;
    file.try_lock().ok()?;
    Some(file)
}

/// Brings an already-open CaptureCraft window to the front.
#[cfg(windows)]
fn focus_existing() {
    use windows::Win32::UI::WindowsAndMessaging::{SW_RESTORE, SetForegroundWindow, ShowWindow};
    if let Some(hwnd) = find_window(&window_title()) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

#[cfg(not(windows))]
fn focus_existing() {}

struct App {
    config: Config,
    autostart: bool,
    recording: bool,
    status: String,
    /// Filled by the background thread when the capture process exits.
    capture: Option<Arc<Mutex<Option<String>>>>,
    tray_running: bool,
    last_tray_check: Instant,
    /// One-time window tweaks that need a real window handle.
    styled: bool,
    /// Theme applied to the context; re-applied when it changes.
    applied_theme: Option<ThemeKind>,
    prefs: Prefs,
    /// Last minimized state of Preferences, to mirror it onto this window.
    show_about: bool,
    /// Newest file in the output folder, for "Open last capture".
    last_capture: Option<PathBuf>,
    /// Title-bar icon, uploaded to egui on first use.
    icon: Option<egui::TextureHandle>,
}

impl App {
    fn new() -> Self {
        let (config, status) = match Config::load_or_create() {
            Ok(c) => (c, String::new()),
            Err(e) => (Config::default(), format!("{e:#}")),
        };
        let tray_running = tray::is_running();
        let output_dir = config.output_dir.clone();
        Self {
            config,
            autostart: autostart::is_enabled(),
            recording: false,
            status,
            capture: None,
            tray_running,
            last_tray_check: Instant::now(),
            styled: false,
            applied_theme: None,
            prefs: Prefs::default(),
            show_about: false,
            last_capture: newest_capture(&output_dir),
            icon: None,
        }
    }

    fn hotkey(&self) -> String {
        self.config.hotkeys.first().cloned().unwrap_or_default()
    }

    fn save(&mut self) {
        self.status = match self.config.save_at(&config_path()) {
            Ok(()) => "Settings saved".to_string(),
            Err(e) => format!("{e:#}"),
        };
    }

    fn set_hotkey(&mut self, hotkey: String) {
        self.config.hotkeys = hotkey_list(&hotkey);
        self.save();
    }

    /// Hides the window and runs `capturecraft --delay-ms …` (overlay).
    fn start_capture(&mut self, ctx: &egui::Context) {
        if self.capture.is_some() {
            return;
        }
        // Spawn *this* binary, never a sibling: a stale capturecraft.exe
        // from an older build would otherwise run the overlay.
        let exe = std::env::current_exe().unwrap_or_else(|_| sibling_exe("capturecraft-app"));
        let mode = match self.config.selection {
            SelectMode::Window => "window",
            SelectMode::Fullscreen => "fullscreen",
            SelectMode::Region => "region",
        };
        let mut cmd = Command::new(&exe);
        let delay = self.config.overlay.hide_delay_ms.to_string();
        cmd.args(["--overlay", mode, "--delay-ms", &delay])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut cmd);
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("Could not start {}: {e}", exe.display());
                return;
            }
        };

        hide_window(ctx, true);
        self.status = "Capturing…".to_string();
        let slot = Arc::new(Mutex::new(None));
        self.capture = Some(slot.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match child.wait_with_output() {
                Ok(out) => {
                    let text = format!(
                        "{}{}",
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    );
                    summarize(&text, out.status.success())
                }
                Err(e) => format!("Capture failed: {e}"),
            };
            if let Ok(mut s) = slot.lock() {
                *s = Some(msg);
            }
            hide_window(&ctx, false);
            ctx.request_repaint();
        });
    }

    fn start_tray(&mut self) {
        let exe = sibling_exe("capturecraft-tray");
        let mut cmd = Command::new(&exe);
        hide_console(&mut cmd);
        self.status = match cmd.spawn() {
            Ok(_) => "Tray started".to_string(),
            Err(e) => format!("Could not start {}: {e}", exe.display()),
        };
        self.last_tray_check = Instant::now() - Duration::from_secs(2);
    }

    fn poll(&mut self) {
        let finished = self
            .capture
            .as_ref()
            .and_then(|slot| slot.lock().ok().and_then(|mut s| s.take()));
        if let Some(msg) = finished {
            if let Some(path) = msg.strip_prefix("Saved ") {
                self.last_capture = Some(PathBuf::from(path));
            }
            self.status = msg;
            self.capture = None;
        }
        if self.last_tray_check.elapsed() >= Duration::from_secs(1) {
            self.tray_running = tray::is_running();
            self.last_tray_check = Instant::now();
        }
    }

    fn record_hotkey(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for event in events {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                if key == egui::Key::Escape {
                    self.recording = false;
                    return;
                }
                if let Some(hotkey) = hotkey_from_key(key, modifiers) {
                    self.recording = false;
                    self.set_hotkey(hotkey);
                    return;
                }
            }
        }
    }

    /// The capture panel's left column: what to capture and where it goes.
    /// Hotkey, theme, tray and autostart live in Edit › Preferences.
    fn settings(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("settings")
            .num_columns(2)
            .spacing([16.0, 10.0])
            .show(ui, |ui| {
                ui.label("Selection");
                let before = self.config.selection;
                egui::ComboBox::from_id_salt("selection")
                    .width(190.0)
                    .selected_text(before.label())
                    .show_ui(ui, |ui| {
                        for mode in SelectMode::ALL {
                            ui.selectable_value(&mut self.config.selection, mode, mode.label())
                                .on_hover_text(mode_hint(mode));
                        }
                    });
                if self.config.selection != before {
                    self.save();
                }
                ui.end_row();

                ui.label("Copy to clipboard");
                if theme::toggle(ui, &mut self.config.clipboard).changed() {
                    self.save();
                }
                ui.end_row();

                ui.label("Open in editor");
                if theme::toggle(ui, &mut self.config.editor).changed() {
                    self.save();
                }
                ui.end_row();

                ui.label("Save to");
                ui.horizontal(|ui| {
                    let dir = self.config.output_dir.display().to_string();
                    ui.add(egui::Label::new(RichText::new(short_path(
                        &self.config.output_dir,
                    ))))
                    .on_hover_text(&dir);
                    if ui.small_button("Open").clicked() {
                        let _ = std::fs::create_dir_all(&self.config.output_dir);
                        open_path(&self.config.output_dir);
                    }
                });
                ui.end_row();
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.applied_theme != Some(self.config.theme) {
            theme::apply(ctx, self.config.theme);
            self.applied_theme = Some(self.config.theme);
        }
        if !self.styled {
            self.styled = true;
            // Keep the hotkey working without the user starting the tray.
            if self.config.start_tray && !self.tray_running {
                self.start_tray();
            }
        }
        // Esc closes the window (but not while recording a hotkey, where it
        // cancels the recording instead).
        if !self.recording && !self.prefs.open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        self.poll();
        if self.recording {
            self.record_hotkey(ctx);
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        if let Some(updated) = self.prefs.show(ctx, &self.config) {
            self.config = updated;
            self.applied_theme = None;
            self.autostart = autostart::is_enabled();
        }
        self.about_window(ctx);

        let title_height = self.title_bar(ctx);
        let menu_height = egui::TopBottomPanel::top("menu")
            .show(ctx, |ui| self.menu_bar(ui))
            .response
            .rect
            .height();
        let status_height = egui::TopBottomPanel::bottom("status")
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (dot, text) = if self.tray_running {
                        (Color32::from_rgb(0x22, 0xC5, 0x5E), "Tray running")
                    } else {
                        (Color32::GRAY, "Tray not running")
                    };
                    // Drawn, not a text glyph: the default font has no "●".
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.5, dot);
                    ui.label(text);
                    if !self.tray_running && ui.small_button("Start").clicked() {
                        self.start_tray();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(VERSION).weak());
                        ui.separator();
                        let last = self.last_capture.clone();
                        let enabled = last.as_ref().is_some_and(|p| p.exists());
                        if ui
                            .add_enabled(enabled, egui::Button::new("Open last").small())
                            .on_hover_text(
                                last.as_ref()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_default(),
                            )
                            .clicked()
                            && let Some(path) = last
                        {
                            open_path(&path);
                        }
                        ui.separator();
                        ui.add(
                            egui::Label::new(RichText::new(&self.status).weak().small()).truncate(),
                        );
                    });
                });
            })
            .response
            .rect
            .height();

        let content = egui::CentralPanel::default()
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(SETTINGS_WIDTH);
                        self.settings(ui);
                    });
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.set_width(BUTTON_SIZE.x);
                        ui.vertical_centered(|ui| self.capture_button(ui));
                    });
                })
                .response
                .rect
            })
            .inner;

        // Shrink-wrap the window around the content (no empty space).
        let margin = ctx.style().spacing.window_margin.rightf().max(8.0);
        // `content` is in screen coordinates, so its top already sits below
        // the title bar and menu; only the bottom panel is still to come.
        let _ = (title_height, menu_height);
        let mut wanted = egui::vec2(
            content.right() + margin,
            content.bottom() + margin + status_height,
        )
        .round();
        // Grow while the Preferences popup is open so it is never clipped,
        // then shrink back to the compact panel when it closes.
        if self.prefs.open {
            wanted = wanted.max(crate::prefs_ui::MODAL_SIZE + egui::vec2(60.0, 130.0));
        }
        let current = ctx.content_rect().size();
        if (wanted - current).abs().max_elem() > 1.0 {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(wanted));
        }
    }
}

impl App {
    /// Menu bar, in PhotoCraft's grammar: File, Edit, View, Tools, Help.
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Capture").clicked() {
                    let ctx = ui.ctx().clone();
                    self.start_capture(&ctx);
                    ui.close();
                }
                if ui.button("Open last capture").clicked() {
                    if let Some(path) = self.last_capture.clone() {
                        open_path(&path);
                    }
                    ui.close();
                }
                if ui.button("Open captures folder").clicked() {
                    let _ = std::fs::create_dir_all(&self.config.output_dir);
                    open_path(&self.config.output_dir);
                    ui.close();
                }
                ui.separator();
                if ui.button("Exit\tEsc").clicked() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                }
                if ui
                    .button("Quit CaptureCraft")
                    .on_hover_text("Closes this window, the editor and the tray")
                    .clicked()
                {
                    tray::request_quit();
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                }
            });

            ui.menu_button("Edit", |ui| {
                if ui.button("Preferences…").clicked() {
                    self.prefs.open(&self.config);
                    ui.close();
                }
                if ui.button("Edit config.toml").clicked() {
                    open_path(&config_path());
                    ui.close();
                }
            });

            ui.menu_button("View", |ui| {
                ui.label(RichText::new("Theme").small());
                for kind in ThemeKind::ALL {
                    if ui.radio(self.config.theme == kind, kind.label()).clicked() {
                        self.config.theme = kind;
                        self.applied_theme = None;
                        self.save();
                        ui.close();
                    }
                }
            });

            ui.menu_button("Tools", |ui| {
                if ui.button("Open editor").clicked() {
                    match &self.last_capture {
                        Some(path) => crate::open_editor(path),
                        None => self.status = "No captures yet".to_string(),
                    }
                    ui.close();
                }
                let running = self.tray_running;
                if ui
                    .add_enabled(!running, egui::Button::new("Start tray"))
                    .clicked()
                {
                    self.start_tray();
                    ui.close();
                }
            });

            ui.menu_button("Help", |ui| {
                if ui.button("GitHub repository").clicked() {
                    open_path(std::path::Path::new(REPO));
                    ui.close();
                }
                if ui.button("Report an issue").clicked() {
                    open_path(std::path::Path::new(&format!("{REPO}/issues")));
                    ui.close();
                }
                ui.separator();
                if ui.button("About CaptureCraft").clicked() {
                    self.show_about = true;
                    ui.close();
                }
            });
        });
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        egui::Window::new("About CaptureCraft")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading("CaptureCraft");
                    ui.label(VERSION);
                    ui.add_space(6.0);
                    ui.label("Open-source screen capture and annotation, in pure Rust.");
                    ui.label(
                        RichText::new("Themes, history model and blur from PhotoCraft v0.9.0")
                            .small()
                            .weak(),
                    );
                    ui.add_space(10.0);
                    if ui.link(REPO).clicked() {
                        open_path(std::path::Path::new(REPO));
                    }
                    ui.label(RichText::new("Apache-2.0").small().weak());
                });
            });
        self.show_about = open;
    }

    /// Custom title bar: app name, drag area, minimize and close.
    /// Returns its height.
    fn title_bar(&mut self, ctx: &egui::Context) -> f32 {
        const HEIGHT: f32 = 32.0;
        egui::TopBottomPanel::top("title_bar")
            .exact_height(HEIGHT)
            .frame(egui::Frame::NONE.fill(theme::tokens(ctx).chrome))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                // Dragging anywhere on the bar moves the window.
                let drag = ui.interact(rect, ui.id().with("drag"), egui::Sense::click_and_drag());
                if drag.drag_started() {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }

                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink2(egui::vec2(10.0, 0.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| {
                        let texture = self.icon_texture(ui.ctx());
                        ui.add(
                            egui::Image::new(&texture).fit_to_exact_size(egui::vec2(18.0, 18.0)),
                        );
                        ui.add_space(6.0);
                        ui.label(RichText::new(window_title()).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if caption_button(ui, CaptionButton::Close) {
                                ctx.send_viewport_cmd(ViewportCommand::Close);
                            }
                            if caption_button(ui, CaptionButton::Minimize) {
                                ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                            }
                        });
                    },
                );
            })
            .response
            .rect
            .height()
    }

    fn icon_texture(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        self.icon
            .get_or_insert_with(|| {
                let (rgba, width, height) = icon_rgba();
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [width as usize, height as usize],
                    &rgba,
                );
                ctx.load_texture("app_icon", image, egui::TextureOptions::LINEAR)
            })
            .clone()
    }

    fn capture_button(&mut self, ui: &mut egui::Ui) {
        let busy = self.capture.is_some();
        let button = egui::Button::new(
            RichText::new(if busy { "…" } else { "Capture" })
                .size(20.0)
                .strong()
                .color(Color32::WHITE),
        )
        .fill(ACCENT)
        .corner_radius(12.0)
        .min_size(BUTTON_SIZE);
        if ui.add_enabled(!busy, button).clicked() {
            let ctx = ui.ctx().clone();
            self.start_capture(&ctx);
        }

        // The hotkey under the button: click it to record a new combination.
        let text = if self.recording {
            "Press keys… (Esc cancels)".to_string()
        } else {
            self.hotkey()
        };
        let label = RichText::new(text).small();
        let label = if self.recording {
            label.color(ACCENT).strong()
        } else {
            label.weak()
        };
        let response = ui
            .add(egui::Label::new(label).sense(egui::Sense::click()))
            .on_hover_text("Click to record a new hotkey");
        if response.clicked() {
            self.recording = !self.recording;
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
}

/// Last two path components, e.g. `…\\Pictures\\CaptureCraft`.
fn short_path(path: &std::path::Path) -> String {
    let parts: Vec<_> = path.components().collect();
    let tail: Vec<String> = parts
        .iter()
        .rev()
        .take(2)
        .rev()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let joined = tail.join(std::path::MAIN_SEPARATOR_STR);
    if parts.len() > 2 {
        format!("…{}{joined}", std::path::MAIN_SEPARATOR)
    } else {
        joined
    }
}

fn mode_hint(mode: SelectMode) -> &'static str {
    match mode {
        SelectMode::Region => "Drag a region, or click a window to snap to it",
        SelectMode::Window => "Click a window",
        SelectMode::Fullscreen => "Whole primary screen, no selection",
    }
}

#[derive(Clone, Copy, PartialEq)]
enum CaptionButton {
    Minimize,
    Close,
}

/// Title-bar button, drawn as lines so no icon font is needed.
fn caption_button(ui: &mut egui::Ui, kind: CaptionButton) -> bool {
    let size = egui::vec2(34.0, 26.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            let fill = if kind == CaptionButton::Close {
                Color32::from_rgb(0xC4, 0x2B, 0x1C)
            } else {
                ui.visuals().widgets.hovered.bg_fill
            };
            ui.painter().rect_filled(rect, 4.0, fill);
        }
        let color = if response.hovered() && kind == CaptionButton::Close {
            Color32::WHITE
        } else {
            ui.visuals().text_color()
        };
        let stroke = egui::Stroke::new(1.2_f32, color);
        let c = rect.center();
        let r = 4.0;
        let p = ui.painter();
        match kind {
            CaptionButton::Minimize => {
                p.line_segment([egui::pos2(c.x - r, c.y), egui::pos2(c.x + r, c.y)], stroke);
            }
            CaptionButton::Close => {
                p.line_segment(
                    [egui::pos2(c.x - r, c.y - r), egui::pos2(c.x + r, c.y + r)],
                    stroke,
                );
                p.line_segment(
                    [egui::pos2(c.x + r, c.y - r), egui::pos2(c.x - r, c.y + r)],
                    stroke,
                );
            }
        }
    }
    response.clicked()
}

/// Title of the main window, also used to find it from the tray.
pub fn window_title() -> String {
    format!("CaptureCraft {VERSION}")
}

/// Newest `.png` in `dir`, if any.
pub fn newest_capture(dir: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("png"))
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(time, _)| *time)
        .map(|(_, path)| path)
}

/// Hotkey list for config: the chosen key first, the fallback second.
pub fn hotkey_list(hotkey: &str) -> Vec<String> {
    let mut list = vec![hotkey.to_string()];
    if !hotkey.eq_ignore_ascii_case(FALLBACK) {
        list.push(FALLBACK.to_string());
    }
    list
}

/// Converts a recorded key press to a hotkey string, e.g. `ctrl+shift+S`.
/// Plain letters/digits without modifiers are rejected (they'd block typing).
fn hotkey_from_key(key: egui::Key, m: egui::Modifiers) -> Option<String> {
    let name = key.name();
    let mut parts: Vec<&str> = Vec::new();
    if m.ctrl {
        parts.push("ctrl");
    }
    if m.alt {
        parts.push("alt");
    }
    if m.shift {
        parts.push("shift");
    }
    if m.mac_cmd {
        parts.push("super");
    }
    if parts.is_empty() && name.chars().count() == 1 {
        return None;
    }
    parts.push(name);
    let hotkey = parts.join("+");
    hotkey.parse::<HotKey>().ok().map(|_| hotkey)
}

/// Turns the capture process output into a one-line status.
fn summarize(output: &str, success: bool) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if let Some(saved) = lines.iter().find_map(|l| l.strip_prefix("saved ")) {
        return format!("Saved {saved}");
    }
    if lines.contains(&"cancelled") {
        return "Cancelled".to_string();
    }
    match (success, lines.last()) {
        (true, _) => "Done".to_string(),
        (false, Some(last)) => last.to_string(),
        (false, None) => "Capture failed".to_string(),
    }
}

/// Hides or shows the main window while a capture runs.
///
/// On Windows this goes straight to the Win32 API: a hidden window gets no
/// redraws, so a queued viewport command would never be processed and the
/// window would stay invisible.
#[cfg(windows)]
fn hide_window(ctx: &egui::Context, hide: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SW_HIDE, SW_SHOW, SetForegroundWindow, ShowWindow,
    };
    match window_handle(ctx) {
        Some(hwnd) => unsafe {
            let _ = ShowWindow(hwnd, if hide { SW_HIDE } else { SW_SHOW });
            if !hide {
                let _ = SetForegroundWindow(hwnd);
            }
        },
        // No handle yet: fall back to the portable path.
        None => viewport_visible(ctx, !hide),
    }
}

#[cfg(windows)]
fn window_handle(ctx: &egui::Context) -> Option<windows::Win32::Foundation::HWND> {
    let title = ctx.input(|i| i.viewport().title.clone())?;
    find_window(&title)
}

#[cfg(windows)]
fn find_window(title: &str) -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    use windows::core::HSTRING;
    let hwnd = unsafe { FindWindowW(None, &HSTRING::from(title)) }.ok()?;
    (hwnd != HWND::default()).then_some(hwnd)
}

#[cfg(not(windows))]
fn hide_window(ctx: &egui::Context, hide: bool) {
    viewport_visible(ctx, !hide);
}

fn viewport_visible(ctx: &egui::Context, visible: bool) {
    ctx.send_viewport_cmd(ViewportCommand::Visible(visible));
    if visible {
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }
}

/// Don't flash a console window for the capture process on Windows.
#[cfg(windows)]
fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_cmd: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_parse() {
        for p in crate::prefs_ui::HOTKEYS {
            assert!(p.parse::<HotKey>().is_ok(), "preset {p}");
        }
    }

    #[test]
    fn hotkey_list_keeps_fallback() {
        assert_eq!(hotkey_list("PrintScreen"), vec!["PrintScreen", FALLBACK]);
        assert_eq!(hotkey_list(FALLBACK), vec![FALLBACK]);
    }

    #[test]
    fn records_hotkeys() {
        let ctrl_shift = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            hotkey_from_key(egui::Key::S, ctrl_shift).as_deref(),
            Some("ctrl+shift+S")
        );
        assert_eq!(
            hotkey_from_key(egui::Key::F9, egui::Modifiers::NONE).as_deref(),
            Some("F9")
        );
        assert_eq!(hotkey_from_key(egui::Key::S, egui::Modifiers::NONE), None);
    }

    #[test]
    fn newest_capture_picks_latest_png() {
        let dir = std::env::temp_dir().join(format!("capturecraft-newest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(newest_capture(&dir), None);

        std::fs::write(dir.join("a.png"), [0]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("b.png"), [0]).unwrap();
        std::fs::write(dir.join("c.txt"), [0]).unwrap();
        assert_eq!(newest_capture(&dir), Some(dir.join("b.png")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn short_path_keeps_tail() {
        let p = std::path::PathBuf::from("/home/sam/Pictures/CaptureCraft");
        assert!(
            short_path(&p).ends_with("Pictures/CaptureCraft")
                || short_path(&p).ends_with("Pictures\\CaptureCraft")
        );
        assert_eq!(short_path(std::path::Path::new("shots")), "shots");
    }

    #[test]
    fn summarize_output() {
        assert_eq!(
            summarize("saved C:\\x\\a.png (10x10)\ncopied to clipboard\n", true),
            "Saved C:\\x\\a.png (10x10)"
        );
        assert_eq!(summarize("cancelled\n", true), "Cancelled");
        assert_eq!(summarize("Error: boom\n", false), "Error: boom");
        assert_eq!(summarize("", false), "Capture failed");
    }
}
