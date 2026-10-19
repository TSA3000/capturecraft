//! The editor window: the dark studio layout in the root viewport.
//!
//! ```text
//! ┌ menu strip 28 px: File Edit Image Video Share View Help │ name.capturecraft ▾ ┐
//! ├ toolbar 58 px: Library Assets │ Capture Create   Favorites … More │ Undo Redo   Copy All [Share ▾] ┤
//! │                                                         │ Tool Properties ?  │
//! │   canvas on a deep surround (zone 3)                    │ (or Effects)       │
//! │   or the Library (zone 5)                               │ drawer, 280 px     │
//! ├ utility bar 32 px: Hide Recent  Tag      100% ▾  W × H px ▾      Effects│Properties ┤
//! └ Recent Captures filmstrip 110 px (zone 4) ──────────────────────────────────────────┘
//! ```
//!
//! The app calls [`EditorWindow::show`] once per frame from
//! `eframe::App::ui` and hands the returned intents to [`crate::dispatch`].
//! The window applies the theme `SidebarState::theme` names (PhotoCraft's
//! `theme::apply`) when it changes.

use capturecraft_engine::{DocState, Session};
use capturecraft_format::{Annotation, CanvasBackdrop, Manifest};
use egui::{Context, DragValue, Frame, Id, Margin, Popup, RichText, Ui, Vec2, pos2, vec2};
use serde_json::json;

use crate::UiIntent;
use crate::icons::{self, Icon};
use crate::panels::canvas_viewport::{CanvasImage, CanvasViewport, MAX_ZOOM, MIN_ZOOM};
use crate::panels::editor_toolbar::{
    EditorToolbar, SHARE_MENU, ZOOM_PRESETS, frame_toggle_intent, select_tool, set_zoom,
    share_entry,
};
use crate::panels::recent_tray::{RecentTray, Thumbnail};
use crate::panels::right_sidebar::RightSidebar;
use crate::state::{AppState, EditorState, InspectorMode, ToolId, TrayItem, Zoom};
use crate::theme::{ThemeKind, Tokens};
use crate::views::library_view::LibraryPanel;
use crate::widgets::{self, icon_style, pill, segmented};

/// Heights of the fixed strips, points.
pub const MENU_H: f32 = 28.0;
pub const TOOLBAR_H: f32 = 58.0;
pub const UTILITY_H: f32 = 32.0;
pub const TRAY_H: f32 = 110.0;
/// The drawer's width.
pub const DRAWER_W: f32 = 280.0;

/// Applies `t`'s theme to `ctx` unless it is already the applied one.
pub fn ensure_theme(ctx: &Context, t: &Tokens) {
    let id = Id::new("capturecraft.applied_theme");
    if ctx.data(|d| d.get_temp::<ThemeKind>(id)) != Some(t.kind) {
        t.apply(ctx);
        ctx.data_mut(|d| d.insert_temp(id, t.kind));
    }
}

/// The zoom after a step (`factor` > 1 zooms in); Fit counts as 100 %.
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
        Zoom::Percent { value } => format!("{value:.0}%"),
    }
}

/// A zoom menu entry.
pub fn zoom_menu_label(z: Zoom) -> String {
    match z {
        Zoom::Fit => "Fit in Window".into(),
        z => zoom_label(z),
    }
}

/// The document's file name: its title, or its capture time as
/// `2026-10-19_18-43-26`, with the `.capturecraft` extension.
pub fn document_file_name(m: &Manifest) -> String {
    let stem: String = if m.title.trim().is_empty() {
        // `2026-10-19T18:43:26+02:00` → `2026-10-19_18-43-26`.
        m.capture
            .captured_at
            .get(..19)
            .map(|s| s.replace('T', "_").replace(':', "-"))
            .unwrap_or_else(|| "Untitled".into())
    } else {
        m.title
            .trim()
            .chars()
            .map(|c| if r#"/\:*?"<>|"#.contains(c) { '-' } else { c })
            .collect()
    };
    format!("{stem}.capturecraft")
}

/// The window title for the active document.
pub fn window_title(doc: Option<&DocState>) -> String {
    match doc {
        Some(d) => format!("{} - CaptureCraft Editor", document_file_name(&d.manifest)),
        None => "CaptureCraft Editor".into(),
    }
}

/// `canvas.expandBounds` margins that bring the canvas to `w × h`, growing
/// or trimming on the right and bottom; `None` when nothing changes.
pub fn resize_params(m: &Manifest, w: u32, h: u32) -> Option<serde_json::Value> {
    let c = m.canvas.size;
    let right = i64::from(w) - i64::from(c.width);
    let bottom = i64::from(h) - i64::from(c.height);
    (right != 0 || bottom != 0).then(
        || json!({ "right": right, "bottom": bottom, "background": { "kind": "transparent" } }),
    )
}

/// Margins that trim the canvas back to the capture (undoing padding);
/// `None` when it already is the capture or a cut out changed its size.
pub fn trim_params(m: &Manifest) -> Option<serde_json::Value> {
    let cut = m
        .annotations
        .iter()
        .any(|n| matches!(n.annotation(), Some(Annotation::CutOutSeam(_))));
    if cut {
        return None;
    }
    let (o, b, c) = (m.canvas.base_origin, m.base.size, m.canvas.size);
    let left = -i64::from(o.x);
    let top = -i64::from(o.y);
    let right = i64::from(o.x) + i64::from(b.width) - i64::from(c.width);
    let bottom = i64::from(o.y) + i64::from(b.height) - i64::from(c.height);
    (left != 0 || top != 0 || right != 0 || bottom != 0)
        .then(|| json!({ "left": left, "top": top, "right": right, "bottom": bottom }))
}

/// The tags typed into the Tag popover: comma separated.
pub fn parse_tags(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

pub struct EditorWindow<'a> {
    pub session: &'a Session,
    /// The active document rendered by the app (`GpuCanvas`/`CpuCanvas`).
    pub image: Option<CanvasImage>,
    /// Thumbnail of a document id, when loaded.
    pub thumbnail: &'a dyn Fn(&str) -> Option<Thumbnail>,
    /// Local date `YYYY-MM-DD`, for the Library's date filters.
    pub today: &'a str,
    /// Image files in the captures folders, listed by the Library after
    /// the open documents (`library_view::file_item`).
    pub library_files: &'a [TrayItem],
}

impl EditorWindow<'_> {
    pub fn show(self, ui: &mut Ui, state: &mut AppState) -> Vec<UiIntent> {
        let tokens = Tokens::from_name(&state.editor.sidebar.theme);
        ensure_theme(ui.ctx(), &tokens);
        let t = &tokens;
        let mut intents = Vec::new();
        let doc = self.session.active();
        let editor = &mut state.editor;
        editor.toolbar.normalize();
        let strip = |fill| Frame::new().fill(fill).inner_margin(Margin::ZERO);

        egui::Panel::top("capturecraft.menu")
            .exact_size(MENU_H)
            .frame(strip(t.title_bar))
            .show(ui, |ui| {
                self.menu_strip(ui, editor, doc, &mut intents, t);
            });
        egui::Panel::top("capturecraft.toolbar")
            .exact_size(TOOLBAR_H)
            .frame(strip(t.chrome))
            .show(ui, |ui| {
                EditorToolbar {
                    has_doc: doc.is_some(),
                    can_undo: doc.is_some_and(DocState::can_undo),
                    can_redo: doc.is_some_and(DocState::can_redo),
                    tokens: t,
                }
                .show(ui, editor, &mut intents);
                widgets::hborder(ui, ui.max_rect(), true, t);
            });
        if editor.tray.visible {
            egui::Panel::bottom("capturecraft.tray")
                .exact_size(TRAY_H)
                .frame(strip(t.panel))
                .show(ui, |ui| {
                    RecentTray {
                        thumbnail: self.thumbnail,
                        tokens: t,
                    }
                    .show(ui, &mut editor.tray, &mut intents);
                });
        }
        egui::Panel::bottom("capturecraft.utility")
            .exact_size(UTILITY_H)
            .frame(strip(t.chrome))
            .show(ui, |ui| {
                utility_bar(ui, editor, doc, &mut intents, t);
                let r = ui.max_rect();
                widgets::hborder(ui, r, false, t);
                widgets::hborder(ui, r, true, t);
            });
        if editor.sidebar.visible {
            egui::Panel::right("capturecraft.sidebar")
                .exact_size(DRAWER_W)
                .frame(
                    Frame::new()
                        .fill(t.panel)
                        .inner_margin(Margin::symmetric(t.space_lg as i8 - 4, 0))
                        .stroke(egui::Stroke::new(1.0, t.chrome_border)),
                )
                .show(ui, |ui| {
                    // A child of exactly the drawer's size, clipped: a row
                    // that is too wide is cut at the edge instead of making
                    // egui push the whole right panel off the window.
                    let r = ui.max_rect();
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
                    child.set_clip_rect(r.intersect(ui.clip_rect()));
                    RightSidebar { doc, tokens: t }.show(&mut child, editor, &mut intents);
                    ui.allocate_rect(r, egui::Sense::hover());
                });
        }
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(t.canvas_surround))
            .show(ui, |ui| {
                if editor.library.visible {
                    Frame::new()
                        .inner_margin(Margin::same(t.space_md as i8))
                        .show(ui, |ui| {
                            let items: Vec<TrayItem> = editor
                                .tray
                                .items
                                .iter()
                                .chain(self.library_files)
                                .cloned()
                                .collect();
                            LibraryPanel {
                                items: &items,
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

    /// File … Help on the left, the document name menu in the centre.
    fn menu_strip(
        &self,
        ui: &mut Ui,
        editor: &mut EditorState,
        doc: Option<&DocState>,
        intents: &mut Vec<UiIntent>,
        t: &Tokens,
    ) {
        let full = ui.max_rect();
        let has_doc = doc.is_some();
        ui.horizontal_centered(|ui| {
            ui.add_space(8.0);
            let (logo, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
            icons::paint(ui.painter(), logo, Icon::Capture, icon_style(t, true, true));
            ui.add_space(6.0);
            ui.spacing_mut().button_padding = vec2(8.0, 2.0);
            ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            let item = |ui: &mut Ui, label: &str, enabled: bool, intent: UiIntent| {
                if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                    intents.push(intent);
                    ui.close();
                }
            };
            let mut item = item;
            ui.menu_button("File", |ui| {
                item(ui, "New Capture", true, UiIntent::NewCapture);
                item(ui, "New Blank Image", true, UiIntent::NewBlankImage);
                ui.separator();
                item(ui, "Save As…", has_doc, UiIntent::SaveAs { doc_id: None });
                item(ui, "Copy All", has_doc, UiIntent::CopyImage);
                ui.separator();
                if let Some(d) = doc {
                    item(
                        ui,
                        "Close Capture",
                        true,
                        UiIntent::CloseDocuments {
                            doc_ids: vec![d.manifest.id.clone()],
                        },
                    );
                }
                item(ui, "Preferences…", true, UiIntent::OpenSettings);
            });
            ui.menu_button("Edit", |ui| {
                let undo = doc.is_some_and(DocState::can_undo);
                let redo = doc.is_some_and(DocState::can_redo);
                item(ui, "Undo", undo, UiIntent::command("edit.undo", json!({})));
                item(ui, "Redo", redo, UiIntent::command("edit.redo", json!({})));
                ui.separator();
                if let Some(d) = doc {
                    item(
                        ui,
                        "Duplicate Capture",
                        true,
                        UiIntent::DuplicateDocument {
                            doc_id: d.manifest.id.clone(),
                        },
                    );
                }
            });
            ui.menu_button("Image", |ui| {
                ui.add_enabled_ui(has_doc, |ui| {
                    for tool in [
                        ToolId::Crop,
                        ToolId::CutOut,
                        ToolId::Simplify,
                        ToolId::SmartMove,
                    ] {
                        if ui.button(tool.label()).clicked() {
                            select_tool(&mut editor.toolbar, tool);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if let Some(d) = doc {
                        let b = &d.manifest.canvas.backdrop;
                        let label = if matches!(b, CanvasBackdrop::None) {
                            "Add Rounded Shadow Frame"
                        } else {
                            "Remove Frame"
                        };
                        item(ui, label, true, frame_toggle_intent(b));
                        if let Some(p) = trim_params(&d.manifest) {
                            item(
                                ui,
                                "Trim Canvas to Capture",
                                true,
                                UiIntent::command("canvas.expandBounds", p),
                            );
                        }
                    }
                    if ui.button("Effects…").clicked() {
                        editor.sidebar.visible = true;
                        editor.sidebar.mode = InspectorMode::Effects;
                        ui.close();
                    }
                });
            });
            ui.menu_button("Video", |ui| {
                item(
                    ui,
                    "Record Screen…",
                    true,
                    UiIntent::Unavailable("Screen recording"),
                );
                item(
                    ui,
                    "Video from Images…",
                    true,
                    UiIntent::Unavailable("Video from Images"),
                );
            });
            ui.menu_button("Share", |ui| {
                ui.add_enabled_ui(has_doc, |ui| {
                    for e in SHARE_MENU {
                        let (label, intent) = share_entry(e);
                        item(ui, &label, true, intent);
                    }
                });
            });
            ui.menu_button("View", |ui| {
                for z in ZOOM_PRESETS {
                    if ui
                        .selectable_label(editor.canvas.zoom == z, zoom_menu_label(z))
                        .clicked()
                    {
                        set_zoom(editor, z);
                        ui.close();
                    }
                }
                ui.separator();
                ui.checkbox(&mut editor.tray.visible, "Recent Captures");
                ui.checkbox(&mut editor.sidebar.visible, "Tool Properties");
                ui.checkbox(&mut editor.library.visible, "Library");
                ui.checkbox(&mut editor.canvas.checkerboard, "Transparency Grid");
            });
            ui.menu_button("Help", |ui| {
                ui.label(format!("CaptureCraft {}", env!("CARGO_PKG_VERSION")));
                ui.label(
                    RichText::new("Open-source screen capture and annotation").color(t.text_dim),
                );
            });
        });

        // The document name, centred in the strip.
        let name = doc.map_or_else(
            || "No capture open".to_string(),
            |d| document_file_name(&d.manifest),
        );
        // `name ⌄ - CaptureCraft Editor`; the chevron is drawn (the UI font
        // has no ▾).
        let font = egui::FontId::proportional(12.0);
        let name_g = ui.painter().layout_no_wrap(name, font.clone(), t.text_dim);
        let app_g = ui
            .painter()
            .layout_no_wrap("-  CaptureCraft Editor".into(), font, t.text_dim);
        let chevron = 8.0;
        let gap = 6.0;
        let w = name_g.size().x + gap + chevron + 2.0 * gap + app_g.size().x + 16.0;
        let rect = egui::Rect::from_center_size(full.center(), vec2(w, MENU_H - 6.0));
        // Keep clear of the menus on narrow windows.
        if rect.min.x > full.min.x + 420.0 {
            let resp = ui.interact(rect, Id::new("menu.document"), egui::Sense::click());
            if resp.hovered() {
                ui.painter()
                    .rect_filled(rect, Tokens::corner(t.radius_sm), t.card_hover);
            }
            let cy = rect.center().y;
            let mut x = rect.min.x + 8.0;
            let name_w = name_g.size().x;
            ui.painter()
                .galley(pos2(x, cy - name_g.size().y / 2.0), name_g, t.text_dim);
            x += name_w + gap;
            icons::paint(
                ui.painter(),
                egui::Rect::from_min_size(pos2(x, cy - chevron / 2.0), Vec2::splat(chevron)),
                Icon::ChevronDown,
                icon_style(t, resp.hovered(), true),
            );
            x += chevron + 2.0 * gap;
            ui.painter()
                .galley(pos2(x, cy - app_g.size().y / 2.0), app_g, t.text_dim);
            Popup::menu(&resp).show(|ui| {
                for d in self.session.documents() {
                    let id = d.manifest.id.clone();
                    let active = doc.is_some_and(|a| a.manifest.id == id);
                    if ui
                        .selectable_label(active, document_file_name(&d.manifest))
                        .clicked()
                    {
                        intents.push(UiIntent::ActivateDocument { doc_id: id });
                        ui.close();
                    }
                }
            });
        }
    }
}

/// Recent and Tag on the left, zoom and canvas size in the middle, the
/// drawer's Effects / Properties tabs under the drawer.
fn utility_bar(
    ui: &mut Ui,
    editor: &mut EditorState,
    doc: Option<&DocState>,
    intents: &mut Vec<UiIntent>,
    t: &Tokens,
) {
    let full = ui.max_rect();
    let y = full.center().y;
    let place = |x: f32, w: f32| egui::Rect::from_min_size(pos2(x, y - 12.0), vec2(w, 24.0));

    // Left
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(place(full.min.x + 8.0, 300.0)),
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let text = if editor.tray.visible {
                    "Hide Recent"
                } else {
                    "Show Recent"
                };
                if pill(
                    ui,
                    Id::new("utility.recent"),
                    Some(Icon::Recent),
                    text,
                    false,
                    false,
                    t,
                )
                .clicked()
                {
                    editor.tray.visible = !editor.tray.visible;
                }
                ui.add_enabled_ui(doc.is_some(), |ui| {
                    let tag = pill(
                        ui,
                        Id::new("utility.tag"),
                        Some(Icon::Tag),
                        "Tag",
                        false,
                        false,
                        t,
                    )
                    .on_hover_text("Tags for this capture (the Library filters by them)");
                    if let Some(d) = doc {
                        tag_popover(&tag, d, intents, t);
                    }
                });
            });
        },
    );

    // Centre
    let mid = full.center().x
        - if editor.sidebar.visible {
            DRAWER_W / 2.0
        } else {
            0.0
        };
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(place(mid - 110.0, 260.0)),
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let zoom = pill(
                    ui,
                    Id::new("utility.zoom"),
                    Some(Icon::Zoom),
                    &zoom_label(editor.canvas.zoom),
                    false,
                    true,
                    t,
                );
                Popup::menu(&zoom).show(|ui| {
                    for z in ZOOM_PRESETS {
                        if ui
                            .selectable_label(editor.canvas.zoom == z, zoom_menu_label(z))
                            .clicked()
                        {
                            set_zoom(editor, z);
                            ui.close();
                        }
                    }
                });
                if let Some(d) = doc {
                    let c = d.manifest.canvas.size;
                    let size = pill(
                        ui,
                        Id::new("utility.size"),
                        None,
                        &format!("{} x {}px", c.width, c.height),
                        false,
                        true,
                        t,
                    )
                    .on_hover_text("Resize or trim the canvas");
                    Popup::menu(&size)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show(|ui| canvas_size_popover(ui, d, intents, t));
                }
            });
        },
    );

    // Right, under the drawer.
    let tabs_w = 210.0;
    let x = if editor.sidebar.visible {
        full.max.x - DRAWER_W + (DRAWER_W - tabs_w) / 2.0
    } else {
        full.max.x - tabs_w - 8.0
    };
    ui.scope_builder(egui::UiBuilder::new().max_rect(place(x, tabs_w)), |ui| {
        let selected = match (editor.sidebar.visible, editor.sidebar.mode) {
            (true, InspectorMode::Effects) => 0,
            (true, InspectorMode::Properties) => 1,
            (false, _) => 2,
        };
        let items = [
            (Some(Icon::Effects), "Effects"),
            (Some(Icon::Properties), "Properties"),
        ];
        if let Some(i) = segmented(ui, Id::new("utility.drawer"), &items, selected, t) {
            let mode = if i == 0 {
                InspectorMode::Effects
            } else {
                InspectorMode::Properties
            };
            // Clicking the shown tab hides the drawer; the other shows it.
            if editor.sidebar.visible && editor.sidebar.mode == mode {
                editor.sidebar.visible = false;
            } else {
                editor.sidebar.visible = true;
                editor.sidebar.mode = mode;
            }
        }
    });
}

fn tag_popover(anchor: &egui::Response, doc: &DocState, intents: &mut Vec<UiIntent>, t: &Tokens) {
    let id = Id::new(("utility.tag_text", &doc.manifest.id));
    Popup::menu(anchor)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(240.0)
        .show(|ui| {
            let mut text = ui
                .ctx()
                .data(|d| d.get_temp::<String>(id))
                .unwrap_or_else(|| doc.manifest.library.tags.join(", "));
            ui.label(RichText::new("Tags").strong());
            let edit = ui.add(
                egui::TextEdit::singleline(&mut text)
                    .hint_text("bug, onboarding, v2")
                    .desired_width(220.0),
            );
            ui.label(RichText::new("Comma separated").color(t.text_faint).small());
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Apply").clicked() || enter {
                intents.push(UiIntent::command(
                    "document.setTags",
                    json!({ "tags": parse_tags(&text) }),
                ));
                ui.ctx().data_mut(|d| d.remove::<String>(id));
                ui.close();
            } else {
                ui.ctx().data_mut(|d| d.insert_temp(id, text));
            }
        });
}

fn canvas_size_popover(ui: &mut Ui, doc: &DocState, intents: &mut Vec<UiIntent>, t: &Tokens) {
    let m = &doc.manifest;
    let id = Id::new(("utility.resize", &m.id));
    let c = m.canvas.size;
    let mut size = ui
        .ctx()
        .data(|d| d.get_temp::<[u32; 2]>(id))
        .unwrap_or([c.width, c.height]);
    ui.label(RichText::new("Resize Canvas").strong());
    ui.label(
        RichText::new("Grows or trims the right and bottom edges")
            .color(t.text_faint)
            .small(),
    );
    egui::Grid::new("utility.resize.grid").show(ui, |ui| {
        ui.label("Width");
        ui.add(
            DragValue::new(&mut size[0])
                .range(1..=capturecraft_format::MAX_SIDE)
                .suffix(" px"),
        );
        ui.end_row();
        ui.label("Height");
        ui.add(
            DragValue::new(&mut size[1])
                .range(1..=capturecraft_format::MAX_SIDE)
                .suffix(" px"),
        );
        ui.end_row();
    });
    let params = resize_params(m, size[0], size[1]);
    if ui
        .add_enabled(params.is_some(), egui::Button::new("Apply"))
        .clicked()
        && let Some(p) = params
    {
        intents.push(UiIntent::command("canvas.expandBounds", p));
        ui.ctx().data_mut(|d| d.remove::<[u32; 2]>(id));
        ui.close();
        return;
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, size));
    ui.separator();
    ui.label(RichText::new("Trim Canvas").strong());
    let trim = trim_params(m);
    if ui
        .add_enabled(trim.is_some(), egui::Button::new("Trim to Capture"))
        .on_disabled_hover_text("The canvas already is the capture (or a cut out changed it)")
        .clicked()
        && let Some(p) = trim
    {
        intents.push(UiIntent::command("canvas.expandBounds", p));
        ui.close();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use capturecraft_engine::Pixels;
    use photocraft_geom::Size;

    #[test]
    fn zoom_steps_from_fit_and_clamp() {
        assert_eq!(step_zoom(Zoom::Fit, 2.0), Zoom::Percent { value: 200.0 });
        assert_eq!(
            step_zoom(Zoom::Percent { value: 3000.0 }, 2.0),
            Zoom::Percent { value: MAX_ZOOM }
        );
        assert_eq!(zoom_label(Zoom::Percent { value: 125.4 }), "125%");
        assert_eq!(zoom_label(Zoom::Fit), "Fit");
        assert_eq!(zoom_menu_label(Zoom::Fit), "Fit in Window");
    }

    #[test]
    fn document_names_come_from_the_title_or_the_capture_time() {
        let mut m = Manifest::new(Size::new(4, 4));
        m.capture.captured_at = "2026-10-19T18:43:26+02:00".into();
        assert_eq!(document_file_name(&m), "2026-10-19_18-43-26.capturecraft");
        m.title = " Login: bug ".into();
        assert_eq!(document_file_name(&m), "Login- bug.capturecraft");
        m.title.clear();
        m.capture.captured_at.clear();
        assert_eq!(document_file_name(&m), "Untitled.capturecraft");
        assert_eq!(window_title(None), "CaptureCraft Editor");
    }

    #[test]
    fn tags_split_on_commas() {
        assert_eq!(parse_tags(" a, b ,,c "), ["a", "b", "c"]);
        assert!(parse_tags("  ").is_empty());
    }

    #[test]
    fn resize_and_trim_run_in_the_engine() {
        let mut s = Session::new();
        let size = Size::new(40, 30);
        s.open(
            Manifest::new(size),
            Pixels::new(size, vec![255; 40 * 30 * 4]),
        )
        .unwrap();
        let m = |s: &Session| s.active().unwrap().manifest.clone();
        assert!(resize_params(&m(&s), 40, 30).is_none());
        assert!(trim_params(&m(&s)).is_none());
        s.run(
            "canvas.expandBounds",
            &json!({ "left": 10, "top": 5, "right": 3, "bottom": 2 }),
        )
        .unwrap();
        assert_eq!(m(&s).canvas.size, Size::new(53, 37));
        let p = resize_params(&m(&s), 60, 30).unwrap();
        s.run("canvas.expandBounds", &p).unwrap();
        assert_eq!(m(&s).canvas.size, Size::new(60, 30));
        s.run("canvas.expandBounds", &trim_params(&m(&s)).unwrap())
            .unwrap();
        let after = m(&s);
        assert_eq!(after.canvas.size, size);
        assert_eq!(
            (after.canvas.base_origin.x, after.canvas.base_origin.y),
            (0, 0)
        );
    }

    /// Frames of the whole window, headless: every zone lays out in both
    /// drawer modes, with and without the tray, and clicking nothing emits
    /// nothing.
    #[test]
    fn the_editor_window_draws_headless() {
        let mut session = Session::new();
        let size = Size::new(64, 48);
        let _ = session.open(
            Manifest::new(size),
            Pixels::new(size, vec![128; 64 * 48 * 4]),
        );
        let mut state = AppState::default();
        let ctx = Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1400.0, 860.0));
        for (library, mode, tray, tool) in [
            (false, InspectorMode::Properties, true, ToolId::Selection),
            (false, InspectorMode::Properties, true, ToolId::Arrow),
            (false, InspectorMode::Effects, false, ToolId::Callout),
            (true, InspectorMode::Properties, true, ToolId::CutOut),
        ] {
            state.editor.library.visible = library;
            state.editor.sidebar.mode = mode;
            state.editor.tray.visible = tray;
            state.editor.toolbar.active = tool;
            let mut out = Vec::new();
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let mut full = ctx.run_ui(input, |ui| {
                out = EditorWindow {
                    session: &session,
                    image: None,
                    thumbnail: &|_| None,
                    today: "2026-10-19",
                    library_files: &[],
                }
                .show(ui, &mut state);
            });
            full.textures_delta.clear();
            assert!(out.is_empty(), "{out:?}");
        }
        let applied = ctx.data(|d| d.get_temp::<ThemeKind>(Id::new("capturecraft.applied_theme")));
        assert_eq!(applied, Some(ThemeKind::Studio));
    }

    /// Drives the window like a user: frames with real pointer events.
    struct Harness {
        ctx: Context,
        session: Session,
        state: AppState,
        screen: egui::Rect,
    }

    impl Harness {
        fn new() -> Self {
            let mut session = Session::new();
            let size = Size::new(898, 670);
            session
                .open(
                    Manifest::new(size),
                    Pixels::new(size, vec![200; 898 * 670 * 4]),
                )
                .unwrap();
            let mut state = AppState::default();
            // One tray item so the filmstrip lays out cards.
            crate::panels::recent_tray::sync_tray(&mut state.editor.tray, &session);
            Self {
                ctx: Context::default(),
                session,
                state,
                screen: egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1400.0, 860.0)),
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<UiIntent> {
            let input = egui::RawInput {
                screen_rect: Some(self.screen),
                events,
                ..Default::default()
            };
            let mut out = Vec::new();
            let (session, state) = (&self.session, &mut self.state);
            let mut full = self.ctx.run_ui(input, |ui| {
                out = EditorWindow {
                    session,
                    image: None,
                    thumbnail: &|_| None,
                    today: "2026-10-19",
                    library_files: &[],
                }
                .show(ui, state);
            });
            full.textures_delta.clear();
            out
        }

        fn rect(&self, id: Id) -> egui::Rect {
            self.ctx
                .read_response(id)
                .unwrap_or_else(|| unreachable!("no widget {id:?}"))
                .rect
        }

        /// Clicks the centre of widget `id`; runs the intents in the engine
        /// and returns them.
        fn click(&mut self, id: Id) -> Vec<UiIntent> {
            let pos = self.rect(id).center();
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let mut out = self.frame(vec![egui::Event::PointerMoved(pos)]);
            out.extend(self.frame(vec![button(true)]));
            out.extend(self.frame(vec![button(false)]));
            out.extend(self.frame(vec![egui::Event::PointerGone]));
            crate::dispatch(&mut self.session, out.clone());
            out
        }
    }

    fn tb(name: &str) -> Id {
        Id::new(("toolbar", name))
    }

    /// Every studio button is where the layout puts it.
    #[test]
    fn every_button_sits_in_its_zone_and_order() {
        let mut h = Harness::new();
        h.frame(Vec::new());
        h.frame(Vec::new());
        let left = ["library", "assets", "capture", "create"];
        let mut centre = vec!["favorites".to_string()];
        centre.extend(ToolId::PINNED.iter().map(|t| t.label().to_string()));
        centre.extend(["more", "undo", "redo"].map(String::from));
        let right = [
            tb("copy_all"),
            Id::new("toolbar.share").with("main"),
            Id::new("toolbar.share").with("chevron"),
        ];
        let ids: Vec<Id> = left
            .iter()
            .map(|n| tb(n))
            .chain(centre.iter().map(|n| tb(n)))
            .chain(right)
            .collect();
        let rects: Vec<egui::Rect> = ids.iter().map(|id| h.rect(*id)).collect();
        for w in rects.windows(2) {
            assert!(w[0].max.x <= w[1].min.x + 0.5, "{w:?}");
        }
        for (id, r) in ids.iter().zip(&rects) {
            assert!(
                r.min.y >= MENU_H && r.max.y <= MENU_H + TOOLBAR_H,
                "{id:?} {r:?}"
            );
        }
        // 48 × 48 icon-over-label cells (Copy All is wider).
        for r in &rects[..left.len() + centre.len()] {
            assert_eq!(r.size(), vec2(48.0, 48.0));
        }
        // The tool strip is centred in the toolbar.
        let strip_l = h.rect(tb("favorites")).min.x;
        let strip_r = h.rect(tb("redo")).max.x;
        assert!(((strip_l + strip_r) / 2.0 - h.screen.center().x).abs() < 1.0);
        // Share is at the right edge.
        assert!(h.screen.max.x - rects[rects.len() - 1].max.x < 12.0);

        // The utility bar sits above the filmstrip: Recent, Tag, then zoom
        // and size, then the drawer tabs under the drawer.
        let bottom = h.screen.max.y - TRAY_H;
        let util = [
            Id::new("utility.recent"),
            Id::new("utility.tag"),
            Id::new("utility.zoom"),
            Id::new("utility.size"),
            Id::new("utility.drawer").with(0),
            Id::new("utility.drawer").with(1),
        ];
        let ur: Vec<egui::Rect> = util.iter().map(|id| h.rect(*id)).collect();
        for w in ur.windows(2) {
            assert!(w[0].max.x <= w[1].min.x + 0.5, "{w:?}");
        }
        for r in &ur {
            assert!(r.min.y >= bottom - UTILITY_H && r.max.y <= bottom, "{r:?}");
        }
        assert!(
            ur[4].min.x >= h.screen.max.x - DRAWER_W,
            "tabs under the drawer"
        );
    }

    /// Clicking each button does what it says, through the engine.
    #[test]
    fn every_button_is_wired() {
        let mut h = Harness::new();
        h.frame(Vec::new());
        let has = |out: &[UiIntent], want: &UiIntent| out.iter().any(|i| i == want);

        assert!(has(&h.click(tb("capture")), &UiIntent::NewCapture));
        assert!(has(&h.click(tb("copy_all")), &UiIntent::CopyImage));
        assert!(has(
            &h.click(Id::new("toolbar.share").with("main")),
            &UiIntent::Export {
                format: crate::state::ImageFormat::Png
            }
        ));
        assert!(has(
            &h.click(tb("assets")),
            &UiIntent::Unavailable("The asset library")
        ));
        h.click(tb("library"));
        assert!(h.state.editor.library.visible);
        h.click(tb("library"));
        assert!(!h.state.editor.library.visible);

        for tool in ToolId::PINNED {
            h.click(tb(tool.label()));
            assert_eq!(h.state.editor.toolbar.active, tool);
        }
        // Undo and Redo run engine commands once there is history.
        h.session
            .run("annotation.addStep", &json!({ "at": { "x": 5, "y": 5 } }))
            .unwrap();
        h.frame(Vec::new());
        let out = h.click(tb("undo"));
        assert!(out.iter().any(|i| matches!(
            i,
            UiIntent::Command {
                id: "edit.undo",
                ..
            }
        )));
        assert!(h.session.active().unwrap().manifest.annotations.is_empty());
        let out = h.click(tb("redo"));
        assert!(out.iter().any(|i| matches!(
            i,
            UiIntent::Command {
                id: "edit.redo",
                ..
            }
        )));
        assert_eq!(h.session.active().unwrap().manifest.annotations.len(), 1);

        // Menus open their popovers.
        for id in [
            tb("create"),
            tb("favorites"),
            tb("more"),
            Id::new("toolbar.share").with("chevron"),
            Id::new("utility.tag"),
            Id::new("utility.zoom"),
            Id::new("utility.size"),
        ] {
            h.click(id);
            assert!(Popup::is_any_open(&h.ctx), "{id:?}");
            h.frame(vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            h.frame(Vec::new());
        }

        // The drawer tabs and Hide Recent.
        h.click(Id::new("utility.drawer").with(0));
        assert_eq!(h.state.editor.sidebar.mode, InspectorMode::Effects);
        h.click(Id::new("utility.drawer").with(1));
        assert_eq!(h.state.editor.sidebar.mode, InspectorMode::Properties);
        assert!(h.state.editor.sidebar.visible);
        h.click(Id::new("utility.recent"));
        assert!(!h.state.editor.tray.visible);
        // The utility bar drops to the bottom once the strip is gone.
        h.frame(Vec::new());
        h.click(Id::new("utility.recent"));
        assert!(h.state.editor.tray.visible);
    }

    /// Nothing in the drawer is wider than the drawer: an overflowing row
    /// makes egui push the whole right panel past the window edge.
    #[test]
    fn the_drawer_fits_in_every_mode_and_tool() {
        let mut h = Harness::new();
        let right = h.screen.max.x;
        let mut cases = vec![(InspectorMode::Effects, ToolId::Arrow)];
        cases.extend(ToolId::ALL.map(|t| (InspectorMode::Properties, t)));
        for (mode, tool) in cases {
            h.state.editor.sidebar.mode = mode;
            h.state.editor.toolbar.active = tool;
            h.frame(Vec::new());
            h.frame(Vec::new());
            let over: Vec<egui::Rect> = h.ctx.viewport(|v| {
                v.prev_pass
                    .widgets
                    .layers()
                    .flat_map(|(_, ws)| ws.iter().map(|w| w.rect))
                    .filter(|r| r.max.x > right + 0.5)
                    .collect()
            });
            assert!(over.is_empty(), "{mode:?} {tool:?}: {over:?}");
            if mode == InspectorMode::Effects {
                let tile = h.rect(Id::new(("sidebar.effect", 0usize)));
                assert!(tile.min.x >= right - DRAWER_W, "{tile:?}");
            }
        }
    }
}
