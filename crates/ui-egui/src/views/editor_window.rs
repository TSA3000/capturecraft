//! The editor window: Snagit's five zones in the root viewport.
//!
//! ```text
//! ┌──────────────── toolbar (zone 1) ────────────────┐
//! │                                    │  Quick      │
//! │   canvas (zone 3)                  │  Styles +   │
//! │   or the Library (zone 5)          │  inspector  │
//! │                                    │  (zone 2)   │
//! ├──────────── Recent Captures (zone 4) ────────────┤
//! └──────────── status: zoom, size ──────────────────┘
//! ```
//!
//! The app calls [`EditorWindow::show`] once per frame from
//! `eframe::App::ui` and hands the returned intents to [`crate::dispatch`].
//! The window applies the theme `SidebarState::theme` names (PhotoCraft's
//! `theme::apply`) when it changes.

use capturecraft_engine::{DocState, Session};
use egui::{Align, Context, Frame, Id, Layout, Margin, Ui};

use crate::UiIntent;
use crate::panels::canvas_viewport::{CanvasImage, CanvasViewport, MAX_ZOOM, MIN_ZOOM};
use crate::panels::editor_toolbar::EditorToolbar;
use crate::panels::recent_tray::{RecentTray, Thumbnail};
use crate::panels::right_sidebar::RightSidebar;
use crate::state::{AppState, EditorState, Zoom};
use crate::theme::{ThemeKind, Tokens};
use crate::views::library_view::LibraryPanel;

/// Heights of the fixed strips, points.
const TOOLBAR_H: f32 = 44.0;
const STATUS_H: f32 = 26.0;
const TRAY_HEADER_H: f32 = 30.0;

/// Applies `t`'s theme to `ctx` unless it is already the applied one.
pub fn ensure_theme(ctx: &Context, t: &Tokens) {
    let id = Id::new("capturecraft.applied_theme");
    if ctx.data(|d| d.get_temp::<ThemeKind>(id)) != Some(t.kind) {
        t.apply(ctx);
        ctx.data_mut(|d| d.insert_temp(id, t.kind));
    }
}

/// The zoom after a status-bar step (`factor` > 1 zooms in); Fit counts
/// as 100 %.
pub fn step_zoom(z: Zoom, factor: f32) -> Zoom {
    let now = match z {
        Zoom::Fit => 100.0,
        Zoom::Percent { value } => value,
    };
    Zoom::Percent {
        value: (now * factor).clamp(MIN_ZOOM, MAX_ZOOM),
    }
}

pub fn zoom_label(z: Zoom) -> String {
    match z {
        Zoom::Fit => "Fit".into(),
        Zoom::Percent { value } => format!("{value:.0} %"),
    }
}

pub struct EditorWindow<'a> {
    pub session: &'a Session,
    /// The active document rendered by the app (`GpuCanvas`/`CpuCanvas`).
    pub image: Option<CanvasImage>,
    /// Thumbnail of a document id, when loaded.
    pub thumbnail: &'a dyn Fn(&str) -> Option<Thumbnail>,
    /// Local date `YYYY-MM-DD`, for the Library's date filters.
    pub today: &'a str,
}

impl EditorWindow<'_> {
    pub fn show(self, ui: &mut Ui, state: &mut AppState) -> Vec<UiIntent> {
        let tokens = Tokens::from_name(&state.editor.sidebar.theme);
        ensure_theme(ui.ctx(), &tokens);
        let t = &tokens;
        let mut intents = Vec::new();
        let doc = self.session.active();
        let editor = &mut state.editor;
        let strip = |fill| {
            Frame::new()
                .fill(fill)
                .inner_margin(Margin::symmetric(t.space_md as i8, t.space_xs as i8))
        };

        egui::Panel::top("capturecraft.toolbar")
            .exact_size(TOOLBAR_H)
            .frame(strip(t.chrome))
            .show(ui, |ui| {
                EditorToolbar {
                    backdrop: doc.map(|d| &d.manifest.canvas.backdrop),
                    can_undo: doc.is_some_and(DocState::can_undo),
                    can_redo: doc.is_some_and(DocState::can_redo),
                    tokens: t,
                }
                .show(ui, editor, &mut intents);
            });
        egui::Panel::bottom("capturecraft.status")
            .exact_size(STATUS_H)
            .frame(strip(t.chrome))
            .show(ui, |ui| status_bar(ui, editor, doc));
        let tray = egui::Panel::bottom("capturecraft.tray").frame(strip(t.panel));
        let tray = if editor.tray.visible {
            tray.resizable(true).default_size(editor.tray.height_px)
        } else {
            tray.exact_size(TRAY_HEADER_H)
        };
        let shown = tray.show(ui, |ui| {
            RecentTray {
                thumbnail: self.thumbnail,
                tokens: t,
            }
            .show(ui, &mut editor.tray, &mut intents);
        });
        if editor.tray.visible {
            editor.tray.height_px = shown.response.rect.height();
        }
        if editor.sidebar.visible {
            let shown = egui::Panel::right("capturecraft.sidebar")
                .resizable(true)
                .default_size(editor.sidebar.width_px)
                .frame(strip(t.panel))
                .show(ui, |ui| {
                    RightSidebar { doc, tokens: t }.show(ui, editor, &mut intents);
                });
            editor.sidebar.width_px = shown.response.rect.width();
        }
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(t.canvas_surround))
            .show(ui, |ui| {
                if editor.library.visible {
                    Frame::new()
                        .inner_margin(Margin::same(t.space_md as i8))
                        .show(ui, |ui| {
                            LibraryPanel {
                                items: &editor.tray.items,
                                thumbnail: self.thumbnail,
                                today: self.today,
                                tokens: t,
                            }
                            .show(
                                ui,
                                &mut editor.library,
                                &mut intents,
                            );
                        });
                } else {
                    CanvasViewport {
                        doc,
                        image: self.image,
                        tokens: t,
                    }
                    .show(ui, editor, &mut intents);
                }
            });
        intents
    }
}

fn status_bar(ui: &mut Ui, editor: &mut EditorState, doc: Option<&DocState>) {
    ui.horizontal_centered(|ui| {
        if let Some(d) = doc {
            let c = &d.manifest.canvas;
            ui.label(format!("{} × {} px", c.size.width, c.size.height));
            let f = c.backdrop.frame_size(c.size);
            if f != c.size {
                ui.weak(format!("framed {} × {} px", f.width, f.height));
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let z = &mut editor.canvas.zoom;
            if ui.small_button("100 %").clicked() {
                *z = Zoom::Percent { value: 100.0 };
                editor.canvas.scroll = [0.0, 0.0];
            }
            if ui.small_button("Fit").clicked() {
                *z = Zoom::Fit;
                editor.canvas.scroll = [0.0, 0.0];
            }
            if ui.small_button("+").clicked() {
                *z = step_zoom(*z, 1.25);
            }
            ui.label(zoom_label(*z));
            if ui.small_button("−").clicked() {
                *z = step_zoom(*z, 0.8);
            }
            ui.separator();
            let side = &mut editor.sidebar.visible;
            if ui.selectable_label(*side, "◨ Styles").clicked() {
                *side = !*side;
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_from_fit_and_clamp() {
        assert_eq!(step_zoom(Zoom::Fit, 2.0), Zoom::Percent { value: 200.0 });
        assert_eq!(
            step_zoom(Zoom::Percent { value: 3000.0 }, 2.0),
            Zoom::Percent { value: MAX_ZOOM }
        );
        assert_eq!(zoom_label(Zoom::Percent { value: 125.4 }), "125 %");
        assert_eq!(zoom_label(Zoom::Fit), "Fit");
    }

    /// One frame of the whole window, headless: every zone lays out and
    /// clicking nothing emits nothing.
    #[test]
    fn the_editor_window_draws_headless() {
        use capturecraft_engine::Pixels;
        use capturecraft_format::Manifest;
        use photocraft_geom::Size;
        let mut session = Session::new();
        let size = Size::new(64, 48);
        let _ = session.open(
            Manifest::new(size),
            Pixels::new(size, vec![128; 64 * 48 * 4]),
        );
        let mut state = AppState::default();
        let ctx = Context::default();
        for library in [false, true] {
            state.editor.library.visible = library;
            let mut out = Vec::new();
            let mut full = ctx.run_ui(egui::RawInput::default(), |ui| {
                out = EditorWindow {
                    session: &session,
                    image: None,
                    thumbnail: &|_| None,
                    today: "2026-10-19",
                }
                .show(ui, &mut state);
            });
            full.textures_delta.clear();
            assert!(out.is_empty(), "{out:?}");
        }
        let applied = ctx.data(|d| d.get_temp::<ThemeKind>(Id::new("capturecraft.applied_theme")));
        assert_eq!(applied, Some(ThemeKind::Studio));
    }
}
