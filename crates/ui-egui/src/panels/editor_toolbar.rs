//! Zone 1: the studio toolbar, 58 px of icon-over-label buttons.
//!
//! ```text
//! Library Assets │ Capture Create▾      Favorites Arrow Text … Selection More▾ │ Undo Redo      Copy All [Share ▾]
//! └──── left ─────────────────────┘      └──────────── centred tool strip ─────────────────┘      └──── right ─────┘
//! ```
//!
//! The tool strip is centred in the toolbar; when the window is too narrow
//! for every pinned tool, the last ones move behind More until it fits.
//! More lists the remaining tools, each with a pin to put it on the bar
//! (right-click a pinned tool to unpin it). Favorites lists the Quick
//! Styles starred in the drawer. Choosing a tool, a zoom or a favourite is
//! view state; everything else is a [`UiIntent`].

use capturecraft_format::CanvasBackdrop;
use egui::{Id, Popup, Rect, Ui, pos2, vec2};
use serde_json::json;

use crate::UiIntent;
use crate::icons::{self, Icon};
use crate::panels::gizmo::backdrop_intent;
use crate::state::{EditorState, ImageFormat, ToolId, ToolbarState, Zoom};
use crate::theme::Tokens;
use crate::widgets::{
    self, TOOL_BUTTON, TOOL_GAP, ToolButton, icon_style, split_button, tool_icon,
};

/// The zoom menu's presets (utility bar, View menu).
pub const ZOOM_PRESETS: [Zoom; 6] = [
    Zoom::Fit,
    Zoom::Percent { value: 25.0 },
    Zoom::Percent { value: 50.0 },
    Zoom::Percent { value: 100.0 },
    Zoom::Percent { value: 200.0 },
    Zoom::Percent { value: 400.0 },
];

/// Width of a vertical divider slot.
const DIVIDER_W: f32 = 13.0;
/// Copy All and Share at the right edge.
const COPY_W: f32 = 56.0;
const SHARE_W: f32 = 128.0;
const SHARE_H: f32 = 32.0;
/// Space kept between the clusters.
const CLUSTER_GAP: f32 = 16.0;

/// Tools that act once and hand back to the previous tool.
fn one_shot(t: ToolId) -> bool {
    matches!(t, ToolId::Crop | ToolId::Simplify)
}

/// Makes `tool` active, remembering where a one-shot tool returns to.
pub fn select_tool(bar: &mut ToolbarState, tool: ToolId) {
    if bar.active == tool {
        return;
    }
    bar.previous = if one_shot(tool) {
        Some(bar.active)
    } else {
        None
    };
    bar.active = tool;
}

/// Picks a zoom preset; the view recentres.
pub fn set_zoom(editor: &mut EditorState, zoom: Zoom) {
    editor.canvas.zoom = zoom;
    editor.canvas.scroll = [0.0, 0.0];
}

/// The Combine trigger: the tray's multi-selection, two or more.
pub fn combine_intent(editor: &EditorState) -> Option<UiIntent> {
    (editor.tray.selected.len() >= 2).then(|| UiIntent::CombineImages {
        doc_ids: editor.tray.selected.clone(),
    })
}

/// The Frame toggle: on adds the default rounded shadow frame, off removes
/// the frame.
pub fn frame_toggle_intent(current: &CanvasBackdrop) -> UiIntent {
    backdrop_intent(&match current {
        CanvasBackdrop::None => CanvasBackdrop::rounded_shadow_frame(),
        CanvasBackdrop::RoundedShadowFrame { .. } => CanvasBackdrop::None,
    })
}

/// The Share menu, in order: `None` is Save As….
pub const SHARE_MENU: [Option<ShareEntry>; 7] = [
    None,
    Some(ShareEntry::Format(ImageFormat::Png)),
    Some(ShareEntry::Format(ImageFormat::Jpeg)),
    Some(ShareEntry::Format(ImageFormat::Webp)),
    Some(ShareEntry::Format(ImageFormat::Svg)),
    Some(ShareEntry::Format(ImageFormat::Pdf)),
    Some(ShareEntry::Clipboard),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareEntry {
    Format(ImageFormat),
    Clipboard,
}

/// A Share menu entry's label and intent.
pub fn share_entry(e: Option<ShareEntry>) -> (String, UiIntent) {
    match e {
        None => ("Save As…".into(), UiIntent::SaveAs { doc_id: None }),
        Some(ShareEntry::Clipboard) => ("Clipboard".into(), UiIntent::CopyImage),
        Some(ShareEntry::Format(format)) => (export_label(format), UiIntent::Export { format }),
    }
}

/// A Share / Export menu entry.
pub fn export_label(f: ImageFormat) -> String {
    match f {
        ImageFormat::Pdf => "PDF (every open capture, one per page)".into(),
        ImageFormat::Svg => "SVG (image inside)".into(),
        ImageFormat::Jpeg => "JPG".into(),
        f => f.label().into(),
    }
}

/// How many pinned tools fit in `width` points of tool strip next to
/// Favorites, More and Undo/Redo.
pub fn pinned_that_fit(pinned: usize, width: f32) -> usize {
    let cell = TOOL_BUTTON.x + TOOL_GAP;
    // Favorites, More, Undo, Redo and the divider are always shown.
    let fixed = strip_width(0);
    (((width - fixed) / cell).floor().max(0.0) as usize).min(pinned)
}

/// The tool strip's natural width with `tools` pinned tools.
fn strip_width(tools: usize) -> f32 {
    let cell = TOOL_BUTTON.x + TOOL_GAP;
    (tools + 4) as f32 * cell + DIVIDER_W - TOOL_GAP
}

const RIGHT_W: f32 = COPY_W + TOOL_GAP + 8.0 + SHARE_W;

pub struct EditorToolbar<'a> {
    pub has_doc: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub tokens: &'a Tokens,
}

/// Lays buttons out left to right from `x`, vertically centred on `y`.
struct Row {
    x: f32,
    cy: f32,
}

impl Row {
    fn next(&mut self, w: f32) -> Rect {
        let r = Rect::from_min_size(
            pos2(self.x, self.cy - TOOL_BUTTON.y / 2.0),
            vec2(w, TOOL_BUTTON.y),
        );
        self.x += w + TOOL_GAP;
        r
    }

    fn divider(&mut self) -> Rect {
        let r = Rect::from_min_size(
            pos2(self.x, self.cy - TOOL_BUTTON.y / 2.0),
            vec2(DIVIDER_W, TOOL_BUTTON.y),
        );
        self.x += DIVIDER_W;
        r
    }
}

impl EditorToolbar<'_> {
    pub fn show(self, ui: &mut Ui, editor: &mut EditorState, intents: &mut Vec<UiIntent>) {
        let t = self.tokens;
        let bar = ui.max_rect();
        let cy = bar.center().y;
        let btn = |ui: &mut Ui, rect, id: &str, icon, label: &str, active, enabled| {
            ToolButton {
                id: Id::new(("toolbar", id)),
                icon,
                label,
                active,
                enabled,
                chevron: false,
            }
            .show(ui, rect, t)
        };

        // --- Left ----------------------------------------------------------
        let mut row = Row {
            x: bar.min.x + 4.0,
            cy,
        };
        let lib = &mut editor.library.visible;
        if btn(
            ui,
            row.next(48.0),
            "library",
            Icon::Library,
            "Library",
            *lib,
            true,
        )
        .on_hover_text("Show past captures instead of the canvas")
        .clicked()
        {
            *lib = !*lib;
        }
        if btn(
            ui,
            row.next(48.0),
            "assets",
            Icon::Assets,
            "Assets",
            false,
            true,
        )
        .on_hover_text("Stamps, templates and themes")
        .clicked()
        {
            intents.push(UiIntent::Unavailable("The asset library"));
        }
        widgets::vdivider(ui, row.divider(), t);
        if btn(
            ui,
            row.next(48.0),
            "capture",
            Icon::Capture,
            "Capture",
            false,
            true,
        )
        .on_hover_text("New capture (opens the crosshair)")
        .clicked()
        {
            intents.push(UiIntent::NewCapture);
        }
        let create = ToolButton {
            id: Id::new(("toolbar", "create")),
            icon: Icon::Create,
            label: "Create",
            active: false,
            enabled: true,
            chevron: true,
        }
        .show(ui, row.next(48.0), t)
        .on_hover_text("Create a new image or video");
        Popup::menu(&create).show(|ui| create_menu(ui, intents));
        let left_end = row.x;

        // --- Right ---------------------------------------------------------
        let right_start = bar.max.x - 6.0 - RIGHT_W;
        let mut rrow = Row { x: right_start, cy };
        let copy = ToolButton {
            id: Id::new(("toolbar", "copy_all")),
            icon: Icon::CopyAll,
            label: "Copy All",
            active: false,
            enabled: self.has_doc,
            chevron: false,
        }
        .show(ui, rrow.next(COPY_W), t)
        .on_hover_text("Copy the image with every annotation");
        if copy.clicked() {
            intents.push(UiIntent::CopyImage);
        }
        rrow.x += 8.0;
        let share_rect =
            Rect::from_min_size(pos2(rrow.x, cy - SHARE_H / 2.0), vec2(SHARE_W, SHARE_H));
        let share = split_button(
            ui,
            share_rect,
            Id::new("toolbar.share"),
            Icon::Share,
            "Share / Save",
            self.has_doc,
            t,
        );
        if share
            .main
            .on_hover_text("Save as PNG in the captures folder")
            .clicked()
        {
            intents.push(UiIntent::Export {
                format: ImageFormat::Png,
            });
        }
        Popup::menu(&share.chevron).show(|ui| {
            for e in SHARE_MENU {
                let (label, intent) = share_entry(e);
                if ui.button(label).clicked() {
                    intents.push(intent);
                    ui.close();
                }
            }
        });

        // --- Centre: tools ---------------------------------------------------
        let room = (right_start - CLUSTER_GAP) - (left_end + CLUSTER_GAP);
        let shown = pinned_that_fit(editor.toolbar.pinned.len(), room);
        let width = strip_width(shown);
        let x = (bar.center().x - width / 2.0)
            .max(left_end + CLUSTER_GAP)
            .min(right_start - CLUSTER_GAP - width);
        let mut row = Row { x, cy };
        let fav = btn(
            ui,
            row.next(48.0),
            "favorites",
            Icon::Favorites,
            "Favorites",
            false,
            true,
        )
        .on_hover_text("Your favourite Quick Styles from every tool");
        Popup::menu(&fav)
            .width(240.0)
            .show(|ui| favorites_menu(ui, editor, t));
        let pinned = editor.toolbar.pinned.clone();
        let mut unpin = None;
        for &tool in pinned.iter().take(shown) {
            let on = editor.toolbar.active == tool;
            let r = btn(
                ui,
                row.next(48.0),
                tool.label(),
                tool_icon(tool),
                tool.label(),
                on,
                self.has_doc,
            )
            .on_hover_text(tool.label());
            if r.clicked() {
                select_tool(&mut editor.toolbar, tool);
            }
            r.context_menu(|ui| {
                if ui.button("Unpin from toolbar").clicked() {
                    unpin = Some(tool);
                    ui.close();
                }
            });
        }
        if let Some(tool) = unpin {
            editor.toolbar.unpin(tool);
        }
        // Pinned tools that do not fit join More, first.
        let hidden: Vec<ToolId> = pinned.iter().skip(shown).copied().collect();
        let more_active = hidden
            .iter()
            .chain(&editor.toolbar.overflow)
            .any(|t| *t == editor.toolbar.active);
        let more = ToolButton {
            id: Id::new(("toolbar", "more")),
            icon: Icon::More,
            label: "More",
            active: more_active,
            enabled: self.has_doc,
            chevron: true,
        }
        .show(ui, row.next(48.0), t)
        .on_hover_text("More tools");
        Popup::menu(&more)
            .width(220.0)
            .show(|ui| more_menu(ui, editor, &hidden, t));
        widgets::vdivider(ui, row.divider(), t);
        if btn(
            ui,
            row.next(48.0),
            "undo",
            Icon::Undo,
            "Undo",
            false,
            self.can_undo,
        )
        .on_hover_text("Undo")
        .clicked()
        {
            intents.push(UiIntent::command("edit.undo", json!({})));
        }
        if btn(
            ui,
            row.next(48.0),
            "redo",
            Icon::Redo,
            "Redo",
            false,
            self.can_redo,
        )
        .on_hover_text("Redo")
        .clicked()
        {
            intents.push(UiIntent::command("edit.redo", json!({})));
        }
    }
}

fn create_menu(ui: &mut Ui, intents: &mut Vec<UiIntent>) {
    if ui.button("New Blank Image").clicked() {
        intents.push(UiIntent::NewBlankImage);
        ui.close();
    }
    ui.separator();
    if ui.button("Image from Template…").clicked() {
        intents.push(UiIntent::Unavailable("Image from Template"));
        ui.close();
    }
    if ui.button("Video from Images…").clicked() {
        intents.push(UiIntent::Unavailable("Video from Images"));
        ui.close();
    }
}

/// One icon + label row of a popover menu; true when clicked.
fn icon_row(ui: &mut Ui, icon: Icon, label: &str, active: bool, t: &Tokens) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        vec2(ui.available_width().max(160.0), 26.0),
        egui::Sense::click(),
    );
    let p = ui.painter();
    if active {
        p.rect_filled(rect, Tokens::corner(t.radius_sm), t.tool_active_fill);
    } else if resp.hovered() {
        p.rect_filled(rect, Tokens::corner(t.radius_sm), t.card_hover);
    }
    let style = icon_style(t, active || resp.hovered(), true);
    icons::paint(
        p,
        Rect::from_center_size(pos2(rect.min.x + 14.0, rect.center().y), vec2(16.0, 16.0)),
        icon,
        style,
    );
    p.text(
        pos2(rect.min.x + 30.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        style.fg,
    );
    resp
}

/// The tools behind More, each with a pin.
fn more_menu(ui: &mut Ui, editor: &mut EditorState, hidden: &[ToolId], t: &Tokens) {
    let tools: Vec<ToolId> = hidden
        .iter()
        .chain(&editor.toolbar.overflow)
        .copied()
        .collect();
    let mut pin = None;
    for tool in tools {
        let pinned = hidden.contains(&tool);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let w = ui.available_width() - 28.0;
            ui.allocate_ui(vec2(w, 26.0), |ui| {
                if icon_row(
                    ui,
                    tool_icon(tool),
                    tool.label(),
                    editor.toolbar.active == tool,
                    t,
                )
                .clicked()
                {
                    select_tool(&mut editor.toolbar, tool);
                    ui.close();
                }
            });
            let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), egui::Sense::click());
            if !pinned {
                let style = icon_style(t, resp.hovered(), true);
                icons::paint(
                    ui.painter(),
                    Rect::from_center_size(r.center(), vec2(14.0, 14.0)),
                    Icon::Pin,
                    style,
                );
                if resp.on_hover_text("Pin to the toolbar").clicked() {
                    pin = Some(tool);
                }
            }
        });
    }
    if let Some(tool) = pin {
        editor.toolbar.pin(tool);
        ui.close();
    }
}

/// Starred Quick Styles; a click picks the tool and the style.
fn favorites_menu(ui: &mut Ui, editor: &mut EditorState, t: &Tokens) {
    let favs = editor.sidebar.quick_styles.favorites();
    if favs.is_empty() {
        ui.label(
            egui::RichText::new(
                "No favourites yet.\nRight-click a Quick Style in Tool Properties\nand choose Add to Favorites.",
            )
            .color(t.text_dim),
        );
        return;
    }
    for (tool, i) in favs {
        let Some(style) = editor.sidebar.quick_styles.style(tool, i) else {
            continue;
        };
        let label = format!("{} · {}", tool.label(), style.name);
        if icon_row(ui, tool_icon(tool), &label, false, t).clicked() {
            editor.sidebar.quick_styles.select(tool, i);
            editor.sidebar.draft = None;
            select_tool(&mut editor.toolbar, tool);
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_tools_return_to_the_previous_tool() {
        let mut bar = ToolbarState::default();
        select_tool(&mut bar, ToolId::Step);
        select_tool(&mut bar, ToolId::Crop);
        assert_eq!(
            (bar.active, bar.previous),
            (ToolId::Crop, Some(ToolId::Step))
        );
        select_tool(&mut bar, ToolId::Arrow);
        assert_eq!(bar.previous, None);
    }

    #[test]
    fn frame_toggle_flips_the_backdrop() {
        let UiIntent::Command { id, params } = frame_toggle_intent(&CanvasBackdrop::None) else {
            unreachable!()
        };
        assert_eq!(id, "canvas.setBackdrop");
        assert_eq!(params["backdrop"]["kind"], "rounded_shadow_frame");
        let UiIntent::Command { params, .. } =
            frame_toggle_intent(&CanvasBackdrop::rounded_shadow_frame())
        else {
            unreachable!()
        };
        assert_eq!(params["backdrop"]["kind"], "none");
    }

    #[test]
    fn the_default_bar_matches_the_studio_layout() {
        let bar = ToolbarState::default();
        assert_eq!(
            bar.pinned,
            [
                ToolId::Arrow,
                ToolId::Text,
                ToolId::Callout,
                ToolId::Shape,
                ToolId::Stamp,
                ToolId::Fill,
                ToolId::Move,
                ToolId::Selection,
            ]
        );
        assert_eq!(
            bar.overflow,
            [
                ToolId::Step,
                ToolId::Blur,
                ToolId::Pixelate,
                ToolId::Highlighter,
                ToolId::Pen,
                ToolId::Eraser,
                ToolId::Magnify,
                ToolId::Spotlight,
                ToolId::CutOut,
                ToolId::SmartMove,
                ToolId::Simplify,
                ToolId::Crop,
            ]
        );
    }

    #[test]
    fn pinning_moves_tools_and_normalize_repairs_old_layouts() {
        let mut bar = ToolbarState::default();
        bar.pin(ToolId::Step);
        assert_eq!(bar.pinned.last(), Some(&ToolId::Step));
        assert!(!bar.overflow.contains(&ToolId::Step));
        bar.unpin(ToolId::Step);
        assert_eq!(
            bar.overflow.first(),
            Some(&ToolId::Step),
            "back in menu order"
        );
        // A layout saved before Selection existed, with a repeat.
        let mut old = ToolbarState {
            pinned: vec![ToolId::Move, ToolId::Arrow, ToolId::Move],
            overflow: vec![ToolId::Crop],
            ..ToolbarState::default()
        };
        old.normalize();
        assert_eq!(old.pinned, [ToolId::Move, ToolId::Arrow]);
        assert_eq!(old.pinned.len() + old.overflow.len(), ToolId::ALL.len());
        assert!(old.overflow.contains(&ToolId::Selection));
    }

    #[test]
    fn narrow_toolbars_send_pinned_tools_behind_more() {
        assert_eq!(pinned_that_fit(8, 2000.0), 8);
        let cell = TOOL_BUTTON.x + TOOL_GAP;
        let w = strip_width(3);
        assert_eq!(pinned_that_fit(8, w), 3);
        assert_eq!(pinned_that_fit(8, w - 1.0), 2);
        assert_eq!(pinned_that_fit(8, 10.0), 0);
        assert!(cell > 0.0);
    }

    #[test]
    fn share_menu_lists_save_as_formats_and_clipboard() {
        let labels: Vec<String> = SHARE_MENU.iter().map(|e| share_entry(*e).0).collect();
        assert_eq!(labels[0], "Save As…");
        assert_eq!(labels[1], "PNG");
        assert_eq!(labels[2], "JPG");
        assert!(labels[5].starts_with("PDF"));
        assert_eq!(labels[6], "Clipboard");
        assert_eq!(share_entry(SHARE_MENU[6]).1, UiIntent::CopyImage);
        assert_eq!(ImageFormat::Jpeg.extension(), "jpg");
    }

    #[test]
    fn zoom_presets_recentre_and_combine_needs_two() {
        let mut e = EditorState::default();
        e.canvas.scroll = [40.0, -10.0];
        set_zoom(&mut e, ZOOM_PRESETS[4]);
        assert_eq!(e.canvas.zoom, Zoom::Percent { value: 200.0 });
        assert_eq!(e.canvas.scroll, [0.0, 0.0]);
        assert_eq!(combine_intent(&e), None);
        e.tray.selected = vec!["a".into(), "b".into()];
        assert_eq!(
            combine_intent(&e),
            Some(UiIntent::CombineImages {
                doc_ids: vec!["a".into(), "b".into()]
            })
        );
    }
}
