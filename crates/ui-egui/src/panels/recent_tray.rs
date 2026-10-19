//! Zone 4: the Recent Captures filmstrip along the bottom.
//!
//! Every item is an open engine document, so a click switches the canvas
//! at once (`UiIntent::ActivateDocument`, no save prompt); unsaved items
//! carry a dot and are never dropped from the strip. Ctrl/Cmd+Click
//! toggles and Shift+Click extends a multi-selection, whose context menu
//! offers Combine Images.

use egui::{
    Align2, Color32, FontId, Id, Pos2, Rect as ERect, Sense, StrokeKind, TextureId, Ui, pos2, vec2,
};

use crate::UiIntent;
use crate::state::{MediaKind, RecentTray as TrayState, TrayItem};
use crate::theme::{Tokens, stroke};

/// A document thumbnail the app has uploaded: texture and pixel size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thumbnail {
    pub texture: TextureId,
    pub size: [u32; 2],
}

/// Height of the strip's header row, points.
const HEADER_H: f32 = 24.0;

pub struct RecentTray<'a> {
    /// Thumbnail of a document id, when loaded.
    pub thumbnail: &'a dyn Fn(&str) -> Option<Thumbnail>,
    pub tokens: &'a Tokens,
}

/// `size` scaled to fit `bounds`, centred, aspect kept.
pub fn fit_rect(size: [u32; 2], bounds: ERect) -> ERect {
    let (w, h) = (size[0].max(1) as f32, size[1].max(1) as f32);
    let s = (bounds.width() / w).min(bounds.height() / h);
    ERect::from_center_size(bounds.center(), vec2(w * s, h * s))
}

impl RecentTray<'_> {
    pub fn show(self, ui: &mut Ui, tray: &mut TrayState, intents: &mut Vec<UiIntent>) {
        let t = self.tokens;
        ui.horizontal(|ui| {
            ui.set_height(HEADER_H);
            let arrow = if tray.visible { "▾" } else { "▸" };
            if ui
                .selectable_label(
                    false,
                    format!("{arrow} Recent Captures ({})", tray.items.len()),
                )
                .clicked()
            {
                tray.visible = !tray.visible;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("+ New Capture").clicked() {
                    intents.push(UiIntent::NewCapture);
                }
                if tray.selected.len() >= 2 && ui.button("Combine Images…").clicked() {
                    intents.push(UiIntent::CombineImages {
                        doc_ids: tray.selected.clone(),
                    });
                }
            });
        });
        if !tray.visible {
            return;
        }
        let card_h = (tray.height_px - HEADER_H).max(48.0);
        let card_w = card_h * 1.5;
        let out = egui::ScrollArea::horizontal()
            .id_salt("recent_tray")
            .horizontal_scroll_offset(tray.scroll_px)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if tray.items.is_empty() {
                        ui.label(
                            egui::RichText::new("Captures you take appear here.")
                                .color(t.text_faint),
                        );
                    }
                    let items = tray.items.clone();
                    for item in &items {
                        self.card(ui, item, vec2(card_w, card_h), tray, intents);
                    }
                });
            });
        tray.scroll_px = out.state.offset.x;
    }

    fn card(
        &self,
        ui: &mut Ui,
        item: &TrayItem,
        size: egui::Vec2,
        tray: &mut TrayState,
        intents: &mut Vec<UiIntent>,
    ) {
        let t = self.tokens;
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let resp = ui.interact(rect, Id::new(("tray.item", &item.doc_id)), Sense::click());
        let active = tray.active.as_deref() == Some(item.doc_id.as_str());
        let selected = tray.selected.contains(&item.doc_id);
        let corner = Tokens::corner(t.radius);
        let p = ui.painter();

        p.add(
            if resp.hovered() {
                t.card_elevation_hover
            } else {
                t.card_elevation
            }
            .as_shape(rect, corner),
        );
        p.rect_filled(
            rect,
            corner,
            if resp.hovered() { t.card_hover } else { t.card },
        );
        let inner = rect.shrink(t.space_sm);
        match (self.thumbnail)(&item.doc_id) {
            Some(th) => {
                p.image(
                    th.texture,
                    fit_rect(th.size, inner),
                    ERect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                p.text(
                    inner.center(),
                    Align2::CENTER_CENTER,
                    if item.title.is_empty() {
                        "Capture"
                    } else {
                        &item.title
                    },
                    FontId::proportional(11.0),
                    t.text_faint,
                );
            }
        }
        if selected {
            p.rect_filled(rect, corner, t.accent_soft);
        }
        let ring = if active {
            stroke(2.0, t.accent)
        } else if selected {
            stroke(1.5, t.accent)
        } else {
            stroke(1.0, t.card_border)
        };
        p.rect_stroke(rect, corner, ring, StrokeKind::Inside);

        if item.kind != MediaKind::Image {
            let text = item.kind.label();
            let galley = p.layout_no_wrap(text.into(), FontId::proportional(10.0), t.badge_text);
            let pad = vec2(t.space_sm + t.space_xs, t.space_xs);
            let b = ERect::from_min_size(
                pos2(inner.min.x, inner.max.y - galley.size().y - 2.0 * pad.y),
                galley.size() + 2.0 * pad,
            );
            p.rect_filled(b, Tokens::corner(t.radius_pill), t.badge_bg);
            p.galley(b.min + pad, galley, t.badge_text);
        }
        if item.unsaved {
            let c = pos2(inner.max.x - 4.0, inner.min.y + 4.0);
            p.circle(c, 4.0, t.dirty, stroke(1.0, t.badge_bg));
        }

        let tip = match (item.title.as_str(), item.unsaved) {
            ("", true) => format!("{} · unsaved", item.captured_at),
            ("", false) => item.captured_at.clone(),
            (title, true) => format!("{title}\n{} · unsaved", item.captured_at),
            (title, false) => format!("{title}\n{}", item.captured_at),
        };
        let resp = resp.on_hover_text(tip);
        if resp.clicked() {
            let m = ui.input(|i| i.modifiers);
            tray.click(&item.doc_id, m.command, m.shift);
            if !m.command && !m.shift && !active {
                tray.active = Some(item.doc_id.clone());
                intents.push(UiIntent::ActivateDocument {
                    doc_id: item.doc_id.clone(),
                });
            }
        }
        if resp.secondary_clicked() && !selected {
            tray.click(&item.doc_id, false, false);
        }
        resp.context_menu(|ui| {
            if ui.button("Open").clicked() {
                tray.active = Some(item.doc_id.clone());
                intents.push(UiIntent::ActivateDocument {
                    doc_id: item.doc_id.clone(),
                });
                ui.close();
            }
            let n = tray.selected.len();
            if ui
                .add_enabled(n >= 2, egui::Button::new(format!("Combine Images… ({n})")))
                .on_disabled_hover_text("Ctrl/Cmd+Click or Shift+Click to select two or more")
                .clicked()
            {
                intents.push(UiIntent::CombineImages {
                    doc_ids: tray.selected.clone(),
                });
                ui.close();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_keep_their_aspect() {
        let b = ERect::from_min_size(Pos2::ZERO, vec2(150.0, 100.0));
        let r = fit_rect([1920, 1080], b);
        assert!((r.width() - 150.0).abs() < 1e-3);
        assert!((r.height() - 150.0 * 1080.0 / 1920.0).abs() < 1e-3);
        assert!((r.center() - b.center()).length() < 1e-3);
        let tall = fit_rect([100, 4000], b);
        assert!((tall.height() - 100.0).abs() < 1e-3 && tall.width() < 3.0);
        assert!(fit_rect([0, 0], b).is_finite());
    }
}
