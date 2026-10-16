//! egui window for the annotation editor: toolbar, canvas, save and copy.
//! The UI is a thin shell — every edit goes through [`Document`], the same
//! split PhotoCraft keeps between its engine and `ui-egui`.

use crate::config::Config;
use crate::editor::{Document, Shape, Tool, rect_from_points};
use crate::prefs_ui::Prefs;
use crate::theme::{self, ThemeKind};
use crate::tray::open_path;
use crate::{RgbaImage, icon_rgba, tray};
use anyhow::{Result, anyhow};
use eframe::egui::{self, Color32, RichText};
use photocraft_geom::Point;
use std::path::PathBuf;
use std::sync::Arc;

const REPO: &str = "https://github.com/TSA3000/capturecraft";

const SWATCHES: [[u8; 3]; 6] = [
    [0xEF, 0x44, 0x44], // red
    [0xF9, 0x73, 0x16], // orange
    [0xFA, 0xCC, 0x15], // yellow
    [0x22, 0xC5, 0x5E], // green
    [0x3B, 0x82, 0xF6], // blue
    [0x11, 0x18, 0x27], // near-black
];

/// Opens the editor for `image` and blocks until the window is closed.
/// `source` is the file the capture came from: Save overwrites it, so the
/// editor edits the capture in place instead of leaving two files behind.
/// Returns the saved path, if the user saved.
pub fn run(image: RgbaImage, config: Config, source: Option<PathBuf>) -> Result<Option<PathBuf>> {
    let (rgba, width, height) = icon_rgba();
    let (w, h) = (image.width() as f32, image.height() as f32);
    // Open near the capture's size, within something sensible for a window.
    let size = [
        (w + 40.0).clamp(560.0, 1400.0),
        (h + 150.0).clamp(420.0, 900.0),
    ];
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(match &source {
                Some(path) => format!(
                    "CaptureCraft Editor — {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                None => "CaptureCraft Editor".to_string(),
            })
            .with_inner_size(size)
            .with_min_inner_size([480.0, 360.0])
            .with_icon(Arc::new(egui::IconData {
                rgba,
                width,
                height,
            })),
        ..Default::default()
    };

    let saved = std::sync::Arc::new(std::sync::Mutex::new(None));
    let result = saved.clone();
    eframe::run_native(
        "CaptureCraft Editor",
        options,
        Box::new(move |_cc| Ok(Box::new(Editor::new(image, config, source, saved)))),
    )
    .map_err(|e| anyhow!("editor: {e}"))?;

    let path = result
        .lock()
        .map(|mut p| p.take())
        .unwrap_or_else(|e| e.into_inner().take());
    Ok(path)
}

struct Editor {
    doc: Document,
    config: Config,
    tool: Tool,
    color: [u8; 3],
    width: f32,
    /// Drag in progress, in image coordinates.
    drag: Option<(Point, Point)>,
    texture: Option<egui::TextureHandle>,
    /// Set when the rendered image changed and the texture must be rebuilt.
    dirty: bool,
    status: String,
    /// File the capture came from; Save writes back to it.
    source: Option<PathBuf>,
    /// Theme applied to the context; re-applied when it changes.
    applied_theme: Option<ThemeKind>,
    show_about: bool,
    prefs: Prefs,
    saved: std::sync::Arc<std::sync::Mutex<Option<PathBuf>>>,
}

impl Editor {
    fn new(
        image: RgbaImage,
        config: Config,
        source: Option<PathBuf>,
        saved: std::sync::Arc<std::sync::Mutex<Option<PathBuf>>>,
    ) -> Self {
        Self {
            doc: Document::new(image),
            config,
            tool: Tool::default(),
            color: SWATCHES[0],
            width: 4.0,
            drag: None,
            texture: None,
            dirty: true,
            status: String::new(),
            source,
            applied_theme: None,
            show_about: false,
            prefs: Prefs::default(),
            saved,
        }
    }

    fn texture(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        if self.dirty || self.texture.is_none() {
            let rendered = self.doc.render();
            let size = [rendered.width() as usize, rendered.height() as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, rendered.as_raw());
            self.texture = Some(ctx.load_texture("capture", image, egui::TextureOptions::LINEAR));
            self.dirty = false;
        }
        match &self.texture {
            Some(t) => t.clone(),
            // Unreachable in practice; a 1x1 placeholder beats a panic.
            None => ctx.load_texture(
                "empty",
                egui::ColorImage::new([1, 1], vec![Color32::TRANSPARENT]),
                egui::TextureOptions::LINEAR,
            ),
        }
    }

    fn save(&mut self) {
        let image = self.doc.render();
        let written = match &self.source {
            Some(path) => image
                .save(path)
                .map(|()| path.clone())
                .map_err(|e| anyhow!("failed to save {}: {e}", path.display())),
            None => tray::save_capture(&image, &self.config.output_dir),
        };
        match written {
            Ok(path) => {
                self.status = format!("Saved {}", path.display());
                if let Ok(mut slot) = self.saved.lock() {
                    *slot = Some(path);
                }
            }
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    fn copy(&mut self) {
        self.status = match crate::copy_to_clipboard(&self.doc.render()) {
            Ok(()) => "Copied to clipboard".to_string(),
            Err(e) => format!("{e:#}"),
        };
    }

    fn commit(&mut self, from: Point, to: Point) {
        let rect = rect_from_points(from, to);
        let shape = match self.tool {
            Tool::Arrow => Shape::Arrow {
                from,
                to,
                color: self.color,
                width: self.width,
            },
            Tool::Box => Shape::Box {
                rect,
                color: self.color,
                width: self.width,
            },
            Tool::Highlight => Shape::Highlight {
                rect,
                color: self.color,
            },
            Tool::Pixelate => Shape::Pixelate {
                rect,
                block: (self.width * 3.0).max(4.0) as u32,
            },
            Tool::Blur => Shape::Blur {
                rect,
                sigma: self.width,
            },
            Tool::Step => Shape::Step {
                at: to,
                number: self.doc.next_step(),
                color: self.color,
                radius: self.width * 3.0,
            },
            Tool::Crop => {
                self.doc.apply_crop(rect);
                self.dirty = true;
                return;
            }
        };
        if is_degenerate(&shape) {
            return;
        }
        self.doc.push(shape);
        self.dirty = true;
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Save\tCtrl+S").clicked() {
                    self.save();
                    ui.close();
                }
                if ui.button("Save as…").clicked() {
                    self.save_as();
                    ui.close();
                }
                if ui.button("Copy\tCtrl+C").clicked() {
                    self.copy();
                    ui.close();
                }
                ui.separator();
                if ui.button("Open captures folder").clicked() {
                    open_path(&self.config.output_dir);
                    ui.close();
                }
                ui.separator();
                if ui.button("Exit\tEsc").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui
                    .button("Quit CaptureCraft")
                    .on_hover_text("Closes this window, the capture window and the tray")
                    .clicked()
                {
                    tray::request_quit();
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            ui.menu_button("Edit", |ui| {
                let undo = match self.doc.history().undo_label() {
                    Some(label) => format!("Undo {label}\tCtrl+Z"),
                    None => "Undo\tCtrl+Z".to_string(),
                };
                if ui
                    .add_enabled(self.doc.can_undo(), egui::Button::new(undo))
                    .clicked()
                {
                    self.doc.undo();
                    self.dirty = true;
                    ui.close();
                }
                let redo = match self.doc.history().redo_label() {
                    Some(label) => format!("Redo {label}\tCtrl+Y"),
                    None => "Redo\tCtrl+Y".to_string(),
                };
                if ui
                    .add_enabled(self.doc.can_redo(), egui::Button::new(redo))
                    .clicked()
                {
                    self.doc.redo();
                    self.dirty = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Preferences…").clicked() {
                    self.prefs.open(&self.config);
                    ui.close();
                }
            });

            ui.menu_button("View", |ui| {
                ui.label(RichText::new("Theme").small());
                for kind in ThemeKind::ALL {
                    if ui.radio(self.config.theme == kind, kind.label()).clicked() {
                        self.set_theme(kind);
                        ui.close();
                    }
                }
            });

            ui.menu_button("Help", |ui| {
                if ui.button("GitHub repository").clicked() {
                    open_url(REPO);
                    ui.close();
                }
                if ui.button("Report an issue").clicked() {
                    open_url(&format!("{REPO}/issues"));
                    ui.close();
                }
                ui.separator();
                if ui.button("About CaptureCraft").clicked() {
                    self.show_about = true;
                    ui.close();
                }
            });

            // Theme selector on the right of the bar.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut theme = self.config.theme;
                egui::ComboBox::from_id_salt("theme")
                    .width(150.0)
                    .selected_text(theme.label())
                    .show_ui(ui, |ui| {
                        for kind in ThemeKind::ALL {
                            ui.selectable_value(&mut theme, kind, kind.label());
                        }
                    });
                if theme != self.config.theme {
                    self.set_theme(theme);
                }
            });
        });
    }

    fn set_theme(&mut self, kind: ThemeKind) {
        self.config.theme = kind;
        self.applied_theme = None;
        if let Err(e) = self.config.save_at(&crate::config::config_path()) {
            self.status = format!("{e:#}");
        }
    }

    /// Saves to a new timestamped file in the output folder.
    fn save_as(&mut self) {
        let image = self.doc.render();
        match tray::save_capture(&image, &self.config.output_dir) {
            Ok(path) => {
                self.status = format!("Saved {}", path.display());
                self.source = Some(path.clone());
                if let Ok(mut slot) = self.saved.lock() {
                    *slot = Some(path);
                }
            }
            Err(e) => self.status = format!("{e:#}"),
        }
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
                    ui.label(crate::app::VERSION);
                    ui.add_space(6.0);
                    ui.label("Open-source screen capture and annotation, in pure Rust.");
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Themes and document model from PhotoCraft v0.9.0")
                            .small()
                            .weak(),
                    );
                    ui.add_space(10.0);
                    if ui.link(REPO).clicked() {
                        open_url(REPO);
                    }
                    ui.label(RichText::new("Apache-2.0").small().weak());
                });
            });
        self.show_about = open;
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for tool in Tool::ALL {
                if ui
                    .selectable_label(self.tool == tool, tool.label())
                    .clicked()
                {
                    self.tool = tool;
                }
            }
            ui.separator();

            for color in SWATCHES {
                let fill = Color32::from_rgb(color[0], color[1], color[2]);
                let selected = self.color == color;
                let size = egui::vec2(20.0, 20.0);
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                if ui.is_rect_visible(rect) {
                    ui.painter().rect_filled(rect, 4.0, fill);
                    if selected {
                        ui.painter().rect_stroke(
                            rect,
                            4.0,
                            egui::Stroke::new(2.0_f32, ui.visuals().text_color()),
                            egui::StrokeKind::Outside,
                        );
                    }
                }
                if response.clicked() {
                    self.color = color;
                }
            }
            ui.separator();

            ui.add(egui::Slider::new(&mut self.width, 1.0..=16.0).text("size"));
            ui.separator();

            if ui
                .add_enabled(self.doc.can_undo(), egui::Button::new("Undo"))
                .clicked()
            {
                self.doc.undo();
                self.dirty = true;
            }
            if ui
                .add_enabled(self.doc.can_redo(), egui::Button::new("Redo"))
                .clicked()
            {
                self.doc.redo();
                self.dirty = true;
            }
        });
    }

    /// Draws the image and handles drawing on it. Returns nothing; all edits
    /// land in the document.
    fn canvas(&mut self, ui: &mut egui::Ui) {
        let texture = self.texture(ui.ctx());
        let size = self.doc.size();
        let available = ui.available_size();
        // Fit the image, never scaling above 1:1.
        let fit =
            (available.x / size.width.max(1) as f32).min(available.y / size.height.max(1) as f32);
        let scale = fit.clamp(0.05, 1.0);
        let shown = egui::vec2(size.width as f32 * scale, size.height as f32 * scale);

        let (rect, response) = ui.allocate_exact_size(shown, egui::Sense::click_and_drag());
        ui.painter().image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        let to_image = |pos: egui::Pos2| {
            Point::new(
                f64::from((pos.x - rect.left()) / scale),
                f64::from((pos.y - rect.top()) / scale),
            )
        };

        if let Some(pos) = response.interact_pointer_pos() {
            let point = to_image(pos);
            if response.drag_started() {
                self.drag = Some((point, point));
            } else if response.dragged() {
                if let Some((start, _)) = self.drag {
                    self.drag = Some((start, point));
                }
            } else if response.clicked() && !self.tool.is_drag() {
                self.commit(point, point);
            }
        }
        if response.drag_stopped()
            && let Some((start, end)) = self.drag.take()
        {
            self.commit(start, end);
        }

        // Live preview of the shape being dragged.
        if let Some((start, end)) = self.drag {
            let a = egui::pos2(
                rect.left() + start.x as f32 * scale,
                rect.top() + start.y as f32 * scale,
            );
            let b = egui::pos2(
                rect.left() + end.x as f32 * scale,
                rect.top() + end.y as f32 * scale,
            );
            let stroke = egui::Stroke::new(
                (self.width * scale).max(1.0),
                Color32::from_rgb(self.color[0], self.color[1], self.color[2]),
            );
            let painter = ui.painter_at(rect);
            match self.tool {
                Tool::Arrow => {
                    painter.line_segment([a, b], stroke);
                }
                Tool::Crop => {
                    painter.rect_stroke(
                        egui::Rect::from_two_pos(a, b),
                        0.0,
                        egui::Stroke::new(1.5_f32, Color32::WHITE),
                        egui::StrokeKind::Inside,
                    );
                }
                _ => {
                    painter.rect_stroke(
                        egui::Rect::from_two_pos(a, b),
                        0.0,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
    }
}

impl eframe::App for Editor {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Keyboard: Ctrl+Z / Ctrl+Y / Ctrl+S / Ctrl+C, Esc closes.
        ctx.input(|i| {
            let ctrl = i.modifiers.command;
            (
                ctrl && i.key_pressed(egui::Key::Z),
                ctrl && i.key_pressed(egui::Key::Y),
            )
        })
        .pipe_undo_redo(&mut self.doc, &mut self.dirty);

        let (save, copy, close) = ctx.input(|i| {
            let ctrl = i.modifiers.command;
            (
                ctrl && i.key_pressed(egui::Key::S),
                ctrl && i.key_pressed(egui::Key::C),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if save {
            self.save();
        }
        if copy {
            self.copy();
        }
        if close && !self.prefs.open {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if self.applied_theme != Some(self.config.theme) {
            theme::apply(ctx, self.config.theme);
            self.applied_theme = Some(self.config.theme);
        }

        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            self.menu_bar(ui);
        });

        egui::TopBottomPanel::top("tools").show(ctx, |ui| {
            ui.add_space(4.0);
            self.toolbar(ui);
            ui.add_space(4.0);
        });

        egui::TopBottomPanel::bottom("actions").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(RichText::new("Save").strong()))
                    .clicked()
                {
                    self.save();
                }
                if ui.button("Copy").clicked() {
                    self.copy();
                }
                if ui.button("Close").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                let size = self.doc.size();
                ui.label(RichText::new(format!("{} × {}", size.width, size.height)).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(&self.status).weak()).truncate());
                });
            });
        });

        let canvas_fill = theme::tokens(ctx).canvas;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(canvas_fill).inner_margin(8))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| self.canvas(ui));
            });

        self.about_window(ctx);
        if let Some(updated) = self.prefs.show(ctx, &self.config) {
            self.config = updated;
            self.applied_theme = None;
        }
    }
}

/// Opens a URL in the default browser.
fn open_url(url: &str) {
    open_path(std::path::Path::new(url));
}

/// Applies undo/redo flags read from the input state.
trait UndoRedo {
    fn pipe_undo_redo(self, doc: &mut Document, dirty: &mut bool);
}

impl UndoRedo for (bool, bool) {
    fn pipe_undo_redo(self, doc: &mut Document, dirty: &mut bool) {
        let (undo, redo) = self;
        if undo {
            doc.undo();
            *dirty = true;
        }
        if redo {
            doc.redo();
            *dirty = true;
        }
    }
}

/// Drags that are too small to see produce nothing. Measured on the shape's
/// own rectangle, not its bounds, which a wide stroke would inflate.
fn is_degenerate(shape: &Shape) -> bool {
    match shape {
        Shape::Step { .. } => false,
        Shape::Arrow { from, to, .. } => (to.x - from.x).abs() < 2.0 && (to.y - from.y).abs() < 2.0,
        Shape::Box { rect, .. }
        | Shape::Highlight { rect, .. }
        | Shape::Pixelate { rect, .. }
        | Shape::Blur { rect, .. } => rect.width() < 2 || rect.height() < 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_geom::Rect;

    #[test]
    fn tiny_drags_are_discarded() {
        assert!(is_degenerate(&Shape::Box {
            rect: Rect::new(5, 5, 6, 6),
            color: [0, 0, 0],
            width: 1.0,
        }));
        assert!(!is_degenerate(&Shape::Box {
            rect: Rect::new(5, 5, 50, 50),
            color: [0, 0, 0],
            width: 1.0,
        }));
        assert!(!is_degenerate(&Shape::Step {
            at: Point::new(1.0, 1.0),
            number: 1,
            color: [0, 0, 0],
            radius: 8.0,
        }));
    }

    #[test]
    fn swatches_are_distinct() {
        for (i, a) in SWATCHES.iter().enumerate() {
            for b in SWATCHES.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
    }
}
