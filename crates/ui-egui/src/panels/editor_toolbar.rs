//! Zone 1: the editor's top toolbar.
//!
//! Left: Capture, the Library toggle, Undo and Redo, the zoom combo (Fit,
//! 100 %, 200 %) and the Cut Out / Simplify / Combine quick triggers.
//! Middle: the tool strip (pinned tools, the rest behind More) and the
//! Frame toggle (the document's rounded shadow frame). Right: Copy All
//! and the Share / Export menu. Choosing a tool or a zoom is view state;
//! everything else is a [`UiIntent`].

use capturecraft_format::CanvasBackdrop;
use egui::{Align, Layout, RichText, Ui, vec2};

use crate::UiIntent;
use crate::panels::gizmo::backdrop_intent;
use crate::state::{EditorState, ImageFormat, ToolId, ToolbarState, Zoom};
use crate::theme::Tokens;
use crate::views::editor_window::zoom_label;

/// The zoom combo's presets.
pub const ZOOM_PRESETS: [Zoom; 3] = [
    Zoom::Fit,
    Zoom::Percent { value: 100.0 },
    Zoom::Percent { value: 200.0 },
];

/// A glyph per tool (egui's bundled emoji font has all of them).
pub fn tool_icon(t: ToolId) -> &'static str {
    match t {
        ToolId::Move => "⬉",
        ToolId::Arrow => "➚",
        ToolId::Text => "T",
        ToolId::Callout => "💬",
        ToolId::Shape => "⬜",
        ToolId::Stamp => "★",
        ToolId::Step => "①",
        ToolId::Fill => "🪣",
        ToolId::CutOut => "✂",
        ToolId::Pen => "✏",
        ToolId::Highlighter => "🖍",
        ToolId::Blur => "▦",
        ToolId::Magnify => "🔍",
        ToolId::Spotlight => "🔦",
        ToolId::SmartMove => "✥",
        ToolId::Simplify => "◫",
        ToolId::Crop => "⛶",
    }
}

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

/// A Share / Export menu entry.
pub fn export_label(f: ImageFormat) -> String {
    match f {
        ImageFormat::Pdf => "PDF (every open capture, one per page)".into(),
        ImageFormat::Svg => "SVG (image inside)".into(),
        f => f.label().into(),
    }
}

pub struct EditorToolbar<'a> {
    /// The open document's backdrop; `None` with no document.
    pub backdrop: Option<&'a CanvasBackdrop>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub tokens: &'a Tokens,
}

impl EditorToolbar<'_> {
    pub fn show(self, ui: &mut Ui, editor: &mut EditorState, intents: &mut Vec<UiIntent>) {
        let t = self.tokens;
        let has_doc = self.backdrop.is_some();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing = vec2(t.space_sm, t.space_sm);
            // --- Left ------------------------------------------------------
            let capture = egui::Button::new(
                RichText::new("● Capture")
                    .color(t.accent_capture_text)
                    .strong(),
            )
            .fill(t.accent_capture)
            .corner_radius(Tokens::corner(t.radius_pill));
            if ui
                .add(capture)
                .on_hover_text("New capture (opens the crosshair)")
                .clicked()
            {
                intents.push(UiIntent::NewCapture);
            }
            let lib = &mut editor.library.visible;
            if ui
                .selectable_label(*lib, "▤ Library")
                .on_hover_text("Show past captures instead of the canvas")
                .clicked()
            {
                *lib = !*lib;
            }
            ui.separator();
            ui.add_enabled_ui(self.can_undo, |ui| {
                if ui.button("↶").on_hover_text("Undo").clicked() {
                    intents.push(UiIntent::command("edit.undo", serde_json::json!({})));
                }
            });
            ui.add_enabled_ui(self.can_redo, |ui| {
                if ui.button("↷").on_hover_text("Redo").clicked() {
                    intents.push(UiIntent::command("edit.redo", serde_json::json!({})));
                }
            });
            egui::ComboBox::from_id_salt("toolbar.zoom")
                .width(64.0)
                .selected_text(zoom_label(editor.canvas.zoom))
                .show_ui(ui, |ui| {
                    for z in ZOOM_PRESETS {
                        if ui
                            .selectable_label(editor.canvas.zoom == z, zoom_label(z))
                            .clicked()
                        {
                            set_zoom(editor, z);
                        }
                    }
                });
            ui.separator();
            ui.add_enabled_ui(has_doc, |ui| {
                for (tool, hint) in [
                    (ToolId::CutOut, "Cut Out: remove a strip and close the gap"),
                    (
                        ToolId::Simplify,
                        "Simplify: replace text and detail with shapes",
                    ),
                ] {
                    let on = editor.toolbar.active == tool;
                    if ui
                        .selectable_label(on, tool_icon(tool))
                        .on_hover_text(hint)
                        .clicked()
                    {
                        select_tool(&mut editor.toolbar, tool);
                    }
                }
            });
            let combine = combine_intent(editor);
            if ui
                .add_enabled(combine.is_some(), egui::Button::new("⧉"))
                .on_hover_text("Combine the selected captures into one")
                .on_disabled_hover_text(
                    "Combine: Ctrl/Cmd+Click two or more captures in the tray first",
                )
                .clicked()
                && let Some(i) = combine
            {
                intents.push(i);
            }
            ui.separator();
            // --- Middle: tools ---------------------------------------------
            for tool in editor.toolbar.pinned.clone() {
                let on = editor.toolbar.active == tool;
                let b = ui
                    .selectable_label(on, RichText::new(tool_icon(tool)).size(16.0))
                    .on_hover_text(tool.label());
                if b.clicked() {
                    select_tool(&mut editor.toolbar, tool);
                }
            }
            if !editor.toolbar.overflow.is_empty() {
                ui.menu_button("More ▾", |ui| {
                    for tool in editor.toolbar.overflow.clone() {
                        let label = format!("{}  {}", tool_icon(tool), tool.label());
                        if ui
                            .selectable_label(editor.toolbar.active == tool, label)
                            .clicked()
                        {
                            select_tool(&mut editor.toolbar, tool);
                            ui.close();
                        }
                    }
                });
            }
            ui.separator();
            let framed = matches!(
                self.backdrop,
                Some(CanvasBackdrop::RoundedShadowFrame { .. })
            );
            ui.add_enabled_ui(has_doc, |ui| {
                if ui
                    .selectable_label(framed, "▢ Frame")
                    .on_hover_text("Rounded corners, shadow and backdrop")
                    .clicked()
                    && let Some(b) = self.backdrop
                {
                    intents.push(frame_toggle_intent(b));
                }
            });
            // --- Right -----------------------------------------------------
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(has_doc, |ui| {
                    ui.menu_button("⤴ Share / Export ▾", |ui| {
                        for format in ImageFormat::EXPORT {
                            if ui.button(export_label(format)).clicked() {
                                intents.push(UiIntent::Export { format });
                                ui.close();
                            }
                        }
                    });
                    if ui
                        .button("⧉ Copy All")
                        .on_hover_text("Copy the image with every annotation")
                        .clicked()
                    {
                        intents.push(UiIntent::CopyImage);
                    }
                });
            });
        });
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
    fn every_tool_has_an_icon_and_the_toolbar_pins_spotlight() {
        for t in ToolId::ALL {
            assert!(!tool_icon(t).is_empty());
        }
        assert!(ToolbarState::default().pinned.contains(&ToolId::Spotlight));
    }

    #[test]
    fn zoom_presets_recentre_and_combine_needs_two() {
        let mut e = EditorState::default();
        e.canvas.scroll = [40.0, -10.0];
        set_zoom(&mut e, ZOOM_PRESETS[2]);
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

    #[test]
    fn export_menu_lists_the_formats_in_order() {
        let labels: Vec<_> = ImageFormat::EXPORT.map(|f| f.label()).to_vec();
        assert_eq!(labels, ["PNG", "JPEG", "WebP", "SVG", "PDF"]);
        assert!(export_label(ImageFormat::Pdf).contains("one per page"));
        assert_eq!(ImageFormat::Jpeg.extension(), "jpg");
    }
}
