//! The canvas right-click menu, after PhotoCraft's `canvas_tool_menu`: not
//! egui's `context_menu` but an `Area` in a menu frame at the click,
//! clamped to the screen, built from a static row table (`None` is a
//! separator) whose items are enabled by [`actions::enabled`]. It closes
//! on Esc, on an item, or on a press outside it (not the press that
//! opened it). Below the edit rows: the Quick Styles of the selection's
//! tool and a strip of tool buttons to switch tools.

use capturecraft_engine::DocState;
use capturecraft_format::NodeId;
use egui::{Context, Id, Pos2, Rect as ERect, Sense, Vec2, vec2};

use crate::UiIntent;
use crate::actions::{self, EditAction};
use crate::icons;
use crate::panels::layers_panel::node_tool;
use crate::panels::right_sidebar::apply_to_selection;
use crate::state::{EditorState, ToolId};
use crate::theme::Tokens;
use crate::widgets::{icon_style, tool_icon};

/// Menu rows: label, shortcut hint and action; `None` separates.
pub const ROWS: &[Option<(&str, &str, Row)>] = &[
    Some(("Cut", "Ctrl+X", Row::Edit(EditAction::Cut))),
    Some(("Copy", "Ctrl+C", Row::Edit(EditAction::Copy))),
    Some(("Paste", "Ctrl+V", Row::Edit(EditAction::Paste))),
    Some(("Duplicate", "Ctrl+D", Row::Edit(EditAction::Duplicate))),
    None,
    Some(("Bring to Front", "", Row::Edit(EditAction::BringToFront))),
    Some(("Bring Forward", "", Row::Edit(EditAction::BringForward))),
    Some(("Send Backward", "", Row::Edit(EditAction::SendBackward))),
    Some(("Send to Back", "", Row::Edit(EditAction::SendToBack))),
    None,
    Some(("Group", "Ctrl+G", Row::Edit(EditAction::Group))),
    Some(("Ungroup", "Ctrl+Shift+G", Row::Edit(EditAction::Ungroup))),
    Some(("Delete", "Del", Row::Edit(EditAction::Delete))),
    None,
    Some(("Grab Text", "", Row::GrabText)),
    Some(("Copy All", "", Row::Edit(EditAction::CopyAll))),
];

/// What a row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Edit(EditAction),
    /// Switch to the Grab Text tool (over the selection's area when there
    /// is one).
    GrabText,
}

/// Tools on the menu's tool strip.
pub const TOOL_STRIP: [ToolId; 8] = [
    ToolId::Move,
    ToolId::Arrow,
    ToolId::Text,
    ToolId::Shape,
    ToolId::Highlighter,
    ToolId::Blur,
    ToolId::Pen,
    ToolId::Crop,
];

const WIDTH: f32 = 220.0;

fn state_id() -> Id {
    Id::new("canvas.context_menu")
}

/// Opens the menu at `pos` (the right-click).
pub fn open(ctx: &Context, pos: Pos2) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.insert_temp(state_id(), (pos, frame)));
}

pub fn close(ctx: &Context) {
    ctx.data_mut(|d| d.remove::<(Pos2, u64)>(state_id()));
}

pub fn is_open(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<(Pos2, u64)>(state_id()))
        .is_some()
}

/// The selection a right-click on `hit` makes: kept when `hit` is part
/// of it, otherwise `hit` and its group (nothing under the pointer keeps
/// the selection, so Paste and Copy All still apply).
pub fn select_for_menu(editor: &mut EditorState, doc: &DocState, hit: Option<NodeId>) {
    if let Some(id) = hit
        && !editor.canvas.selection.contains(&id)
    {
        actions::select_with_group(editor, doc, id);
    }
}

/// Draws the open menu; returns the intents of a chosen row.
pub fn show(
    ctx: &Context,
    editor: &mut EditorState,
    doc: Option<&DocState>,
    t: &Tokens,
) -> Vec<UiIntent> {
    let mut out = Vec::new();
    let Some((pos, opened)) = ctx.data(|d| d.get_temp::<(Pos2, u64)>(state_id())) else {
        return out;
    };
    // Clamp with last frame's size so the menu never leaves the screen.
    let size_id = state_id().with("size");
    let size: Vec2 = ctx
        .data(|d| d.get_temp(size_id))
        .unwrap_or(vec2(WIDTH, 420.0));
    let screen = ctx.content_rect();
    let at = Pos2::new(
        pos.x.min(screen.max.x - size.x - 4.0).max(screen.min.x),
        pos.y.min(screen.max.y - size.y - 4.0).max(screen.min.y),
    );
    let mut done = false;
    let area = egui::Area::new(state_id().with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing.y = 0.0;
                for row in ROWS {
                    let Some((label, hint, what)) = row else {
                        ui.separator();
                        continue;
                    };
                    let enabled = match what {
                        Row::Edit(a) => actions::enabled(*a, editor, doc),
                        Row::GrabText => doc.is_some(),
                    };
                    let b = egui::Button::new(*label)
                        .shortcut_text(*hint)
                        .frame_when_inactive(false)
                        .min_size(vec2(WIDTH, 22.0));
                    if ui.add_enabled(enabled, b).clicked() {
                        done = true;
                        match what {
                            Row::Edit(a) => out.extend(actions::run(*a, editor, doc)),
                            Row::GrabText => editor.toolbar.active = ToolId::GrabText,
                        }
                    }
                }
                // Quick Styles of what is selected.
                let sel_tool = doc.and_then(|d| {
                    let id = editor.canvas.selection.first()?;
                    node_tool(d.manifest.node(*id)?)
                });
                if let Some(tool) = sel_tool
                    && let Some(styles) = editor.sidebar.quick_styles.for_tool(tool).cloned()
                {
                    ui.separator();
                    ui.menu_button(format!("{} Quick Style", tool.label()), |ui| {
                        for (i, s) in styles.styles.iter().enumerate().filter(|(_, s)| !s.hidden) {
                            let on = styles.selected == Some(i);
                            if ui
                                .add(egui::Button::selectable(on, s.name.as_str()))
                                .clicked()
                            {
                                editor.sidebar.quick_styles.select(tool, i);
                                editor.sidebar.draft = None;
                                out.extend(apply_to_selection(editor, tool));
                                done = true;
                            }
                        }
                    });
                }
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for tool in TOOL_STRIP {
                        let active = editor.toolbar.active == tool;
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
                        if active {
                            ui.painter().rect_filled(r, 4.0, t.tool_active_fill);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(r, 4.0, t.card_hover);
                        }
                        icons::paint(
                            ui.painter(),
                            r.shrink(4.0),
                            tool_icon(tool),
                            icon_style(t, active || resp.hovered(), true),
                        );
                        if resp.on_hover_text(tool.label()).clicked() {
                            editor.toolbar.active = tool;
                            done = true;
                        }
                    }
                });
                ui.add_space(2.0);
            });
        });
    let rect: ERect = area.response.rect;
    ctx.data_mut(|d| d.insert_temp(size_id, rect.size()));
    let outside = opened != ctx.cumulative_frame_nr()
        && ctx.input(|i| {
            i.pointer.any_pressed()
                && i
                    .pointer
                    .interact_pos()
                    .is_some_and(|p| !rect.contains(p))
        })
        // A submenu (the Quick Styles) is its own popup.
        && !egui::Popup::is_any_open(ctx);
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if done || outside || escape {
        close(ctx);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_edit_row_is_an_action_and_separators_split_groups() {
        let rows: Vec<_> = ROWS.iter().flatten().collect();
        assert!(
            rows.iter().any(
                |(l, _, r)| *l == "Bring to Front" && *r == Row::Edit(EditAction::BringToFront)
            )
        );
        assert!(!matches!(ROWS.first(), Some(None)) && !matches!(ROWS.last(), Some(None)));
        assert!(ROWS.windows(2).all(|w| w[0].is_some() || w[1].is_some()));
    }
}
