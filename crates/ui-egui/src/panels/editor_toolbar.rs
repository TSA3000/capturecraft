//! Zone 1: the editor's top toolbar.
//!
//! Left to right: the pinned tools (customisable, the rest behind More),
//! the Frame toggle (the document's rounded shadow frame), Undo and Redo,
//! Library, Copy and Share, and the orange Capture button. Choosing a tool
//! is view state; everything else is a [`UiIntent`].

use capturecraft_format::CanvasBackdrop;
use egui::{Align, Layout, RichText, Ui, vec2};

use crate::UiIntent;
use crate::panels::gizmo::backdrop_intent;
use crate::state::{EditorState, ToolId, ToolbarState};
use crate::theme::Tokens;

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

/// The Frame toggle: on adds the default rounded shadow frame, off removes
/// the frame.
pub fn frame_toggle_intent(current: &CanvasBackdrop) -> UiIntent {
    backdrop_intent(&match current {
        CanvasBackdrop::None => CanvasBackdrop::rounded_shadow_frame(),
        CanvasBackdrop::RoundedShadowFrame { .. } => CanvasBackdrop::None,
    })
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
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing = vec2(t.space_sm, t.space_sm);
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
            ui.add_enabled_ui(self.backdrop.is_some(), |ui| {
                if ui
                    .selectable_label(framed, "▢ Frame")
                    .on_hover_text("Rounded corners, shadow and backdrop")
                    .clicked()
                    && let Some(b) = self.backdrop
                {
                    intents.push(frame_toggle_intent(b));
                }
            });
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
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let capture = egui::Button::new(
                    RichText::new("● Capture")
                        .color(t.accent_capture_text)
                        .strong(),
                )
                .fill(t.accent_capture)
                .corner_radius(Tokens::corner(t.radius_pill));
                if ui.add(capture).clicked() {
                    intents.push(UiIntent::NewCapture);
                }
                let has_doc = self.backdrop.is_some();
                ui.add_enabled_ui(has_doc, |ui| {
                    if ui.button("⤴ Share").clicked() {
                        intents.push(UiIntent::Share);
                    }
                    if ui.button("⧉ Copy").clicked() {
                        intents.push(UiIntent::CopyImage);
                    }
                });
                let lib = &mut editor.library.visible;
                if ui.selectable_label(*lib, "▤ Library").clicked() {
                    *lib = !*lib;
                }
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
}
