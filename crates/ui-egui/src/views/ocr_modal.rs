//! The Grab Text result: a modal over the editor with the recognised text
//! (editable before copying), laid out in reading order (each column read
//! top to bottom, as `ocr.grabText` returns it) or as on screen (columns
//! side by side, one row per line of the screenshot). **Copy Text** puts
//! it on the clipboard; **Replace as Editable Text** heals each line's
//! pixels and puts a live text box there (`ocr.replaceLines`, one undo
//! step).

use egui::{Context, Id, RichText};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::UiIntent;
use crate::theme::Tokens;

/// One line `ocr.grabText` found.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GrabLine {
    pub rect: GrabRect,
    pub text: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GrabRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

/// How the modal lays the lines out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GrabLayout {
    /// Column by column, each top to bottom.
    #[default]
    Reading,
    /// Rows across the columns, separated by a tab.
    Columns,
}

/// An `ocr.grabText` result the modal shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GrabResult {
    pub recognized: bool,
    pub analyzer: String,
    pub lines: Vec<GrabLine>,
    /// The raw lines, handed back to `ocr.replaceLines`.
    pub raw_lines: Vec<Value>,
    /// What the text box shows (the user may edit it before copying).
    pub text: String,
    pub layout: GrabLayout,
}

impl GrabResult {
    pub fn from_value(v: &Value) -> Self {
        let raw_lines: Vec<Value> = v["lines"].as_array().cloned().unwrap_or_default();
        let lines: Vec<GrabLine> = raw_lines
            .iter()
            .filter_map(|l| serde_json::from_value(l.clone()).ok())
            .collect();
        let mut r = Self {
            recognized: v["recognized"].as_bool().unwrap_or(false),
            analyzer: v["analyzer"].as_str().unwrap_or_default().to_string(),
            lines,
            raw_lines,
            text: String::new(),
            layout: GrabLayout::Reading,
        };
        r.text = layout_text(&r.lines, r.layout);
        r
    }

    pub fn has_text(&self) -> bool {
        self.lines
            .iter()
            .any(|l| l.text.as_deref().is_some_and(|t| !t.trim().is_empty()))
    }
}

/// The lines' text in `layout`. Lines without text are left out.
pub fn layout_text(lines: &[GrabLine], layout: GrabLayout) -> String {
    let with_text: Vec<(&GrabLine, &str)> = lines
        .iter()
        .filter_map(|l| Some((l, l.text.as_deref()?)))
        .filter(|(_, t)| !t.trim().is_empty())
        .collect();
    match layout {
        GrabLayout::Reading => with_text
            .iter()
            .map(|(_, t)| *t)
            .collect::<Vec<_>>()
            .join("\n"),
        GrabLayout::Columns => {
            // Rows: lines whose vertical extents overlap by half the
            // shorter line, ordered left to right.
            let mut sorted = with_text.clone();
            sorted.sort_by_key(|(l, _)| (l.rect.y0, l.rect.x0));
            let mut rows: Vec<Vec<(&GrabLine, &str)>> = Vec::new();
            for item in sorted {
                let r = item.0.rect;
                let fits = rows.last().is_some_and(|row| {
                    row.iter().any(|(o, _)| {
                        let overlap = r.y1.min(o.rect.y1) - r.y0.max(o.rect.y0);
                        let shorter = (r.y1 - r.y0).min(o.rect.y1 - o.rect.y0).max(1);
                        overlap * 2 >= shorter
                    })
                });
                match rows.last_mut() {
                    Some(row) if fits => row.push(item),
                    _ => rows.push(vec![item]),
                }
            }
            rows.into_iter()
                .map(|mut row| {
                    row.sort_by_key(|(l, _)| l.rect.x0);
                    row.iter().map(|(_, t)| *t).collect::<Vec<_>>().join("\t")
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

/// Shows the modal while `grab` is set; clears it when closed.
pub fn show(ctx: &Context, grab: &mut Option<GrabResult>, intents: &mut Vec<UiIntent>, t: &Tokens) {
    let Some(g) = grab.as_mut() else {
        return;
    };
    let mut close = false;
    let modal = egui::Modal::new(Id::new("grab_text.modal")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading("Grab Text");
        let n = g.lines.len();
        if !g.recognized {
            ui.label(
                RichText::new(if n == 0 {
                    "No text was found in this area.".to_string()
                } else {
                    format!(
                        "{n} line{} found, but they cannot be read: the text recognition models are missing from the models folder beside CaptureCraft (reinstall it; a source build runs `cargo xtask models`).",
                        if n == 1 { "" } else { "s" }
                    )
                })
                .color(t.text_dim),
            );
        } else {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{n} lines")).color(t.text_dim));
                ui.separator();
                let before = g.layout;
                ui.selectable_value(&mut g.layout, GrabLayout::Reading, "Reading order");
                ui.selectable_value(&mut g.layout, GrabLayout::Columns, "As on screen");
                if g.layout != before {
                    g.text = layout_text(&g.lines, g.layout);
                }
            });
        }
        ui.add_space(t.space_sm);
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut g.text)
                        .id(Id::new("grab_text.text"))
                        .desired_width(f32::INFINITY)
                        .desired_rows(10)
                        .font(egui::TextStyle::Monospace),
                );
            });
        ui.add_space(t.space_md);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!g.text.trim().is_empty(), egui::Button::new("Copy Text"))
                .clicked()
            {
                intents.push(UiIntent::CopyText(g.text.clone()));
                close = true;
            }
            if ui
                .add_enabled(g.has_text(), egui::Button::new("Replace as Editable Text"))
                .on_hover_text("Heal each line's pixels and put a live text box in its place")
                .clicked()
            {
                intents.push(UiIntent::command(
                    "ocr.replaceLines",
                    json!({ "lines": g.raw_lines }),
                ));
                close = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
    });
    if close || modal.should_close() {
        *grab = None;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn line(x0: i32, y0: i32, x1: i32, y1: i32, t: &str) -> Value {
        json!({ "id": 1, "rect": { "x0": x0, "y0": y0, "x1": x1, "y1": y1 }, "text": t })
    }

    #[test]
    fn the_modal_draws_headless_and_esc_closes_it() {
        let ctx = Context::default();
        let t = Tokens::from_name("studio");
        let mut grab = Some(GrabResult::from_value(&json!({
            "recognized": true,
            "lines": [line(0, 0, 50, 10, "hello")],
        })));
        let mut intents = Vec::new();
        let mut frame = |events: Vec<egui::Event>, grab: &mut Option<GrabResult>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| show(ui.ctx(), grab, &mut intents, &t));
            out.textures_delta.clear();
        };
        frame(Vec::new(), &mut grab);
        frame(Vec::new(), &mut grab);
        assert!(grab.is_some());
        frame(
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut grab,
        );
        assert!(grab.is_none());
        assert!(intents.is_empty());
    }

    #[test]
    fn columns_read_in_order_or_side_by_side() {
        // As ocr.grabText returns them: the left column, then the right.
        let v = json!({
            "recognized": true,
            "analyzer": "ocrs",
            "lines": [
                line(10, 10, 380, 26, "title"),
                line(10, 40, 180, 56, "left 1"),
                line(10, 60, 180, 76, "left 2"),
                line(220, 41, 380, 57, "right 1"),
                line(220, 60, 380, 76, "right 2"),
                { "id": 2, "rect": { "x0": 0, "y0": 90, "x1": 9, "y1": 99 }, "text": null },
            ],
        });
        let g = GrabResult::from_value(&v);
        assert_eq!(g.text, "title\nleft 1\nleft 2\nright 1\nright 2");
        assert_eq!(
            layout_text(&g.lines, GrabLayout::Columns),
            "title\nleft 1\tright 1\nleft 2\tright 2"
        );
        assert!(g.has_text() && g.recognized);
        assert_eq!(g.raw_lines.len(), 6);
        let empty = GrabResult::from_value(&json!({ "recognized": false, "lines": [] }));
        assert!(!empty.has_text() && empty.text.is_empty());
    }
}
