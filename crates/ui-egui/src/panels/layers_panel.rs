//! The drawer's Layers and History tabs, after PhotoCraft's panels.
//!
//! **Layers** lists the annotations top first above the capture: an eye
//! cell (its own click target, so it never starts a drag), a lock cell, the
//! tool icon and the name (double-click renames). A click selects the
//! annotation and its group (Ctrl/Cmd toggles); dragging a row shows a
//! 2 px drop line above or below the row under the pointer and moves it
//! there on release (`annotation.reorder`). Visibility, lock and name go
//! through `annotation.setProps`.
//!
//! **History** lists *Open Capture*, every edit made (the undo stack) and
//! the undone ones (greyed, the redo stack). Clicking a row jumps there in
//! one `edit.jump` command.

use capturecraft_engine::DocState;
use capturecraft_engine::node_commands::{expand_groups, is_text_box};
use capturecraft_format::{Annotation, AnnotationNode, BlendMode, FilterMode, NodeId};
use egui::{Align2, FontId, Id, Rect as ERect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;

use crate::UiIntent;
use crate::icons::{self, Icon, IconStyle};
use crate::state::{EditorState, ToolId};
use crate::theme::Tokens;
use crate::widgets::tool_icon;

const ROW_H: f32 = 30.0;
const CELL: f32 = 26.0;

/// The row icon of `n`, made by `tool`: a pixelated blur shows the mosaic.
fn node_icon(n: &AnnotationNode, tool: ToolId) -> Icon {
    match n.annotation() {
        Some(Annotation::BlurRegion(b)) if b.effect.mode() == FilterMode::Pixelate => {
            Icon::Pixelate
        }
        _ => tool_icon(tool),
    }
}

/// The tool that makes annotations like `n` (its row icon).
pub fn node_tool(n: &AnnotationNode) -> Option<ToolId> {
    Some(match n.annotation()? {
        Annotation::Callout(c) if is_text_box(c) => ToolId::Text,
        Annotation::Callout(_) => ToolId::Callout,
        Annotation::StepBadge(_) => ToolId::Step,
        Annotation::CurvedArrow(_) => ToolId::Arrow,
        Annotation::BlurRegion(_) => ToolId::Blur,
        Annotation::MagnifierLens(_) => ToolId::Magnify,
        Annotation::Spotlight(_) => ToolId::Spotlight,
        Annotation::CutOutSeam(_) => ToolId::CutOut,
        Annotation::Shape(s) if s.blend == BlendMode::Multiply => ToolId::Highlighter,
        Annotation::Shape(_) => ToolId::Shape,
        Annotation::SmartObject(_) => ToolId::SmartMove,
        Annotation::Pen(_) => ToolId::Pen,
    })
}

/// The row label: the node's name, else what it is (and its text).
pub fn node_label(n: &AnnotationNode) -> String {
    if !n.name.is_empty() {
        return n.name.clone();
    }
    match n.annotation() {
        Some(Annotation::Callout(c)) => {
            let text = c.text.text();
            let first = text.lines().next().unwrap_or_default().trim();
            let kind = if is_text_box(c) { "Text" } else { "Callout" };
            if first.is_empty() {
                kind.into()
            } else {
                format!("{kind}: {first}")
            }
        }
        Some(Annotation::StepBadge(b)) => format!("Step {}", b.label()),
        Some(Annotation::SmartObject(o)) if !o.element.is_empty() => {
            format!("Object: {}", o.element)
        }
        Some(Annotation::BlurRegion(b)) if b.effect.mode() == FilterMode::Pixelate => {
            "Pixelate".into()
        }
        _ => node_tool(n).map_or_else(|| "Unknown".into(), |t| t.label().into()),
    }
}

/// Where a dragged row lands relative to the row under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drop {
    Above,
    Below,
}

/// The `annotation.reorder` index (bottom first) that puts the node at
/// `from` above or below the node at `target` (both bottom-first indices
/// before the move); `None` when nothing moves.
pub fn drop_index(from: usize, target: usize, drop: Drop) -> Option<usize> {
    if from == target {
        return None;
    }
    // Index of the target once the dragged node is out of the list.
    let t = if from < target { target - 1 } else { target };
    let to = match drop {
        Drop::Above => t + 1,
        Drop::Below => t,
    };
    (to != from).then_some(to)
}

/// History rows: *Open Capture*, the edits made, the undone edits; the
/// current row and how many rows are undone edits.
pub fn history_rows(doc: &DocState) -> (Vec<String>, usize) {
    let (undo, redo) = doc.history_labels();
    let mut rows = vec!["Open Capture".to_string()];
    rows.extend(undo.iter().map(|s| (*s).to_string()));
    let current = rows.len() - 1;
    rows.extend(redo.iter().map(|s| (*s).to_string()));
    (rows, current)
}

/// How a rename field ends this frame: `None` keeps editing; `Some(None)`
/// ends it without a change (Esc, or the name is the same); `Some(name)`
/// commits (Enter or a click away). `ended` is the field losing focus.
pub fn rename_outcome(
    first: bool,
    ended: bool,
    escape: bool,
    text: &str,
    current: &str,
) -> Option<Option<String>> {
    if first || !ended {
        return None;
    }
    let name = text.trim();
    Some((!escape && name != current).then(|| name.to_string()))
}

fn cell_icon(ui: &Ui, r: ERect, on: bool, eye: bool, t: &Tokens) {
    let p = ui.painter();
    let c = r.center();
    let col = if on { t.text } else { t.text_faint };
    if eye {
        let w = 7.0;
        p.add(egui::Shape::convex_polygon(
            vec![
                pos2(c.x - w, c.y),
                pos2(c.x - w * 0.45, c.y - 3.5),
                pos2(c.x + w * 0.45, c.y - 3.5),
                pos2(c.x + w, c.y),
                pos2(c.x + w * 0.45, c.y + 3.5),
                pos2(c.x - w * 0.45, c.y + 3.5),
            ],
            egui::Color32::TRANSPARENT,
            Stroke::new(1.2, col),
        ));
        if on {
            p.circle_filled(c, 2.2, col);
        } else {
            p.line_segment(
                [c + vec2(-6.0, 5.0), c + vec2(6.0, -5.0)],
                Stroke::new(1.2, col),
            );
        }
    } else {
        // Padlock: shackle and body; open when unlocked.
        let body = ERect::from_center_size(c + vec2(0.0, 2.0), vec2(9.0, 7.0));
        let lift = if on { 0.0 } else { -2.5 };
        p.add(egui::Shape::line(
            vec![
                pos2(body.min.x + 1.5, body.min.y),
                pos2(body.min.x + 1.5, body.min.y - 3.0 + lift),
                pos2(body.max.x - 1.5, body.min.y - 3.0 + lift),
                pos2(body.max.x - 1.5, body.min.y - if on { 0.0 } else { 3.0 }),
            ],
            Stroke::new(1.2, col),
        ));
        if on {
            p.rect_filled(body, 1.5, col);
        } else {
            p.rect_stroke(body, 1.5, Stroke::new(1.2, col), StrokeKind::Inside);
        }
    }
}

pub struct LayersPanel<'a> {
    pub doc: Option<&'a DocState>,
    pub tokens: &'a Tokens,
}

impl LayersPanel<'_> {
    pub fn layers(&self, ui: &mut Ui, editor: &mut EditorState, intents: &mut Vec<UiIntent>) {
        let t = self.tokens;
        let Some(doc) = self.doc else {
            ui.label(egui::RichText::new("No capture open").color(t.text_faint));
            return;
        };
        let m = &doc.manifest;
        let n = m.annotations.len();
        let drag_id = Id::new("layers.drag");
        let rename_id = Id::new("layers.rename");
        let dragging: Option<NodeId> = ui.ctx().data(|d| d.get_temp(drag_id));
        let renaming: Option<(NodeId, String)> = ui.ctx().data(|d| d.get_temp(rename_id));
        let pointer = ui.ctx().pointer_interact_pos();
        let released = ui.input(|i| i.pointer.any_released());
        let toggle = ui.input(|i| i.modifiers.command);
        let w = ui.available_width();
        let mut drop_at: Option<(usize, Drop)> = None;
        // Top first.
        for (row, node) in m.annotations.iter().rev().enumerate() {
            let index = n - 1 - row;
            let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click_and_drag());
            let selected = editor.canvas.selection.contains(&node.id);
            let p = ui.painter();
            let r = Tokens::corner(t.radius_sm);
            if selected {
                p.rect_filled(rect, r, t.tool_active_fill);
            } else if resp.hovered() {
                p.rect_filled(rect, r, t.card_hover.gamma_multiply(0.5));
            }
            // Eye and lock: their own click targets.
            let eye = ERect::from_min_size(rect.min + vec2(2.0, 2.0), vec2(CELL, ROW_H - 4.0));
            let lock = eye.translate(vec2(CELL, 0.0));
            let eye_resp = ui.interact(eye, Id::new(("layers.eye", node.id.0)), Sense::click());
            let lock_resp = ui.interact(lock, Id::new(("layers.lock", node.id.0)), Sense::click());
            cell_icon(ui, eye, node.visible, true, t);
            cell_icon(ui, lock, node.locked, false, t);
            if eye_resp.clicked() {
                intents.push(UiIntent::command(
                    "annotation.setProps",
                    json!({ "ids": [node.id.0], "visible": !node.visible }),
                ));
            }
            if lock_resp.clicked() {
                intents.push(UiIntent::command(
                    "annotation.setProps",
                    json!({ "ids": [node.id.0], "locked": !node.locked }),
                ));
                if !node.locked {
                    editor.canvas.selection.retain(|id| *id != node.id);
                }
            }
            eye_resp.on_hover_text(if node.visible { "Hide" } else { "Show" });
            lock_resp.on_hover_text(if node.locked { "Unlock" } else { "Lock" });
            let icon = ERect::from_center_size(
                pos2(lock.max.x + 14.0, rect.center().y),
                egui::Vec2::splat(18.0),
            );
            if let Some(tool) = node_tool(node) {
                icons::paint(
                    ui.painter(),
                    icon,
                    node_icon(node, tool),
                    IconStyle {
                        fg: if node.visible {
                            t.text_dim
                        } else {
                            t.text_faint
                        },
                        accent: t.record,
                    },
                );
            }
            let text_x = icon.max.x + 8.0;
            let name_rect = ERect::from_min_max(
                pos2(text_x, rect.min.y + 3.0),
                pos2(rect.max.x - 4.0, rect.max.y - 3.0),
            );
            match &renaming {
                Some((id, text)) if *id == node.id => {
                    let mut text = text.clone();
                    let edit_id = Id::new("layers.rename.edit");
                    let edit = ui.put(name_rect, egui::TextEdit::singleline(&mut text).id(edit_id));
                    // Focus once, when renaming starts: requesting it every
                    // frame would take it back before `lost_focus` is read,
                    // so Enter, Esc and a click away could never end it.
                    let first = ui.ctx().data(|d| d.get_temp::<bool>(edit_id)).is_none();
                    if first {
                        edit.request_focus();
                        ui.ctx().data_mut(|d| d.insert_temp(edit_id, true));
                    }
                    match rename_outcome(
                        first,
                        edit.lost_focus() || (!first && !edit.has_focus()),
                        ui.input(|i| i.key_pressed(egui::Key::Escape)),
                        &text,
                        &node_label(node),
                    ) {
                        Some(commit) => {
                            ui.ctx().data_mut(|d| {
                                d.remove::<(NodeId, String)>(rename_id);
                                d.remove::<bool>(edit_id);
                            });
                            if let Some(name) = commit
                                && name != node.name
                            {
                                intents.push(UiIntent::command(
                                    "annotation.setProps",
                                    json!({ "ids": [node.id.0], "name": name }),
                                ));
                            }
                        }
                        None => {
                            ui.ctx()
                                .data_mut(|d| d.insert_temp(rename_id, (node.id, text)));
                        }
                    }
                }
                _ => {
                    let color = if node.visible { t.text } else { t.text_faint };
                    let mut label = node_label(node);
                    if let Some(g) = node.group {
                        label = format!("{label}  ·  group {g}");
                    }
                    let galley = ui.painter().layout(
                        label,
                        FontId::proportional(12.0),
                        color,
                        name_rect.width(),
                    );
                    ui.painter().with_clip_rect(name_rect).galley(
                        pos2(name_rect.min.x, rect.center().y - galley.size().y / 2.0),
                        galley,
                        color,
                    );
                }
            }
            if resp.double_clicked() {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(rename_id, (node.id, node_label(node))));
            } else if resp.clicked() {
                let group = expand_groups(m, &[node.id]);
                if toggle {
                    for id in group {
                        if let Some(i) = editor.canvas.selection.iter().position(|s| *s == id) {
                            editor.canvas.selection.remove(i);
                        } else {
                            editor.canvas.selection.push(id);
                        }
                    }
                } else {
                    editor.canvas.selection = group;
                }
                if !node.locked && node.visible {
                    editor.toolbar.active = ToolId::Move;
                }
            }
            if resp.drag_started() {
                ui.ctx().data_mut(|d| d.insert_temp(drag_id, node.id));
            }
            if let (Some(d), Some(pos)) = (dragging, pointer)
                && d != node.id
                && rect.contains(pos)
            {
                let drop = if (pos.y - rect.top()) / rect.height() < 0.5 {
                    Drop::Above
                } else {
                    Drop::Below
                };
                let s = Stroke::new(2.0, t.accent);
                let y = if drop == Drop::Above {
                    rect.top()
                } else {
                    rect.bottom()
                };
                ui.painter()
                    .line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], s);
                drop_at = Some((index, drop));
            }
        }
        // The capture itself, always at the bottom.
        let (rect, _) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::hover());
        let icon = ERect::from_center_size(
            pos2(rect.min.x + 2.0 + 2.0 * CELL + 14.0, rect.center().y),
            egui::Vec2::splat(18.0),
        );
        icons::paint(
            ui.painter(),
            icon,
            Icon::Capture,
            IconStyle {
                fg: t.text_dim,
                accent: t.record,
            },
        );
        ui.painter().text(
            pos2(icon.max.x + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            "Capture",
            FontId::proportional(12.0),
            t.text_dim,
        );
        if let Some(d) = dragging {
            if released {
                ui.ctx().data_mut(|data| data.remove::<NodeId>(drag_id));
                if let (Some((target, drop)), Some(from)) =
                    (drop_at, m.annotations.iter().position(|a| a.id == d))
                    && let Some(index) = drop_index(from, target, drop)
                {
                    intents.push(UiIntent::command(
                        "annotation.reorder",
                        json!({ "id": d.0, "index": index }),
                    ));
                }
            } else if let Some(pos) = pointer
                && let Some(node) = m.node(d)
            {
                // The ghost: the dragged row's label under the pointer.
                let painter = ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    Id::new("layers.ghost"),
                ));
                painter.text(
                    pos + vec2(12.0, 0.0),
                    Align2::LEFT_CENTER,
                    node_label(node),
                    FontId::proportional(12.0),
                    t.text,
                );
            }
        }
        if n == 0 {
            ui.add_space(t.space_md);
            ui.label(
                egui::RichText::new("Annotations you add are listed here, topmost first.")
                    .color(t.text_faint)
                    .small(),
            );
        }
    }

    pub fn history(&self, ui: &mut Ui, intents: &mut Vec<UiIntent>) {
        let t = self.tokens;
        let Some(doc) = self.doc else {
            ui.label(egui::RichText::new("No capture open").color(t.text_faint));
            return;
        };
        let (rows, current) = history_rows(doc);
        let w = ui.available_width();
        for (i, label) in rows.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
            let p = ui.painter();
            if i == current {
                p.rect_filled(rect, Tokens::corner(t.radius_sm), t.tool_active_fill);
            } else if resp.hovered() {
                p.rect_filled(rect, 0.0, t.card_hover.gamma_multiply(0.5));
            }
            let undone = i > current;
            p.text(
                pos2(rect.min.x + 10.0, rect.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(12.0),
                if undone { t.text_faint } else { t.text },
            );
            if resp.clicked() && i != current {
                let steps = i as i64 - current as i64;
                intents.push(UiIntent::command("edit.jump", json!({ "steps": steps })));
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use capturecraft_engine::{Pixels, Session};
    use capturecraft_format::Manifest;
    use photocraft_geom::Size;

    #[test]
    fn drop_lines_become_reorder_indices() {
        // Bottom-first [a, b, c, d]: drag a (0) above c (2) -> index 2.
        assert_eq!(drop_index(0, 2, Drop::Above), Some(2));
        assert_eq!(drop_index(0, 2, Drop::Below), Some(1));
        // Drag d (3) below b (1) -> index 1.
        assert_eq!(drop_index(3, 1, Drop::Below), Some(1));
        assert_eq!(drop_index(3, 1, Drop::Above), Some(2));
        // Onto itself, or where it already is.
        assert_eq!(drop_index(2, 2, Drop::Above), None);
        assert_eq!(drop_index(1, 2, Drop::Below), None);
    }

    #[test]
    fn renames_commit_on_enter_or_click_away_and_esc_cancels() {
        // Still typing, or the frame the field opens.
        assert_eq!(rename_outcome(false, false, false, "New", "Old"), None);
        assert_eq!(rename_outcome(true, true, false, "New", "Old"), None);
        // Enter or a click away commits the trimmed name.
        assert_eq!(
            rename_outcome(false, true, false, "  New ", "Old"),
            Some(Some("New".into()))
        );
        // Esc ends without a command, as does an unchanged name.
        assert_eq!(rename_outcome(false, true, true, "New", "Old"), Some(None));
        assert_eq!(
            rename_outcome(false, true, false, "Old ", "Old"),
            Some(None)
        );
    }

    #[test]
    fn rows_name_annotations_and_history_has_open_capture_first() {
        let mut s = Session::new();
        let size = Size::new(100, 100);
        s.open(
            Manifest::new(size),
            Pixels::new(size, vec![255; 100 * 100 * 4]),
        )
        .unwrap();
        s.run(
            "annotation.addText",
            &json!({ "bounds": { "x": 0, "y": 0, "w": 50, "h": 20 }, "text": "Hello\nworld" }),
        )
        .unwrap();
        s.run("annotation.addStep", &json!({ "at": { "x": 5, "y": 5 } }))
            .unwrap();
        s.run("edit.undo", &json!({})).unwrap();
        let doc = s.active().unwrap();
        let n = &doc.manifest.annotations[0];
        assert_eq!(node_label(n), "Text: Hello");
        assert_eq!(node_tool(n), Some(ToolId::Text));
        let (rows, current) = history_rows(doc);
        assert_eq!(rows, ["Open Capture", "Add Text", "Add Step"]);
        assert_eq!(current, 1);
    }
}
