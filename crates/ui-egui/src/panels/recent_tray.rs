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

use capturecraft_engine::{DocState, Session};
use capturecraft_format::CaptureMode;

use crate::UiIntent;
use crate::state::{MediaKind, RecentTray as TrayState, TrayItem};
use crate::theme::{Tokens, stroke};

/// A document thumbnail the app has uploaded: texture and pixel size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thumbnail {
    pub texture: TextureId,
    pub size: [u32; 2],
}

/// The tray entry of an open document, from its capture metadata.
pub fn tray_item(doc: &DocState) -> TrayItem {
    let m = &doc.manifest;
    let title = if m.title.trim().is_empty() {
        // `2026-10-19T14:03:11+02:00` → `Capture 14:03`.
        match m.capture.captured_at.get(11..16) {
            Some(hm) => format!("Capture {hm}"),
            None => "Capture".into(),
        }
    } else {
        m.title.clone()
    };
    TrayItem {
        doc_id: m.id.clone(),
        title,
        kind: match m.capture.mode {
            CaptureMode::Scrolling => MediaKind::Panorama,
            _ => MediaKind::Image,
        },
        thumbnail_path: None,
        file_path: None,
        captured_at: m.capture.captured_at.clone(),
        // Documents live in the session until Save exists.
        unsaved: true,
        tags: m.library.tags.clone(),
        app_name: m
            .capture
            .source
            .as_ref()
            .map(|s| s.app_name.clone())
            .unwrap_or_default(),
        favorite: m.library.favorite,
        simplified: doc.simplify_state().is_some(),
    }
}

/// Brings the tray in line with the session: every open document has an
/// item (new ones in front), items refresh from their document, items that
/// are neither open nor saved to a file are dropped (a restart empties the
/// session), and the active document is the highlighted one.
pub fn sync_tray(tray: &mut TrayState, session: &Session) {
    let docs = session.documents();
    let open = |id: &str| docs.iter().any(|d| d.manifest.id == id);
    tray.items
        .retain(|i| open(&i.doc_id) || i.file_path.is_some());
    tray.selected.retain(|id| open(id));
    for doc in docs {
        let fresh = tray_item(doc);
        match tray.items.iter_mut().find(|i| i.doc_id == fresh.doc_id) {
            Some(item) => {
                let file_path = item.file_path.take();
                *item = TrayItem { file_path, ..fresh };
            }
            None => tray.push(fresh),
        }
    }
    tray.active = session.active().map(|d| d.manifest.id.clone());
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod sync_tests {
    use super::*;
    use capturecraft_format::Manifest;
    use photocraft_geom::Size;

    fn open(s: &mut Session, id: &str, at: &str) {
        let mut m = Manifest::new(Size::new(4, 4));
        m.id = id.into();
        m.capture.captured_at = at.into();
        s.open(m, None).unwrap();
    }

    #[test]
    fn the_tray_follows_the_session() {
        let mut tray = TrayState::default();
        // An item left over from an earlier run: not open, never saved.
        tray.push(TrayItem {
            doc_id: "stale".into(),
            ..TrayItem::default()
        });
        tray.push(TrayItem {
            doc_id: "saved".into(),
            file_path: Some("/tmp/a.capturecraft".into()),
            ..TrayItem::default()
        });
        let mut s = Session::new();
        open(&mut s, "a", "2026-10-19T09:15:00+02:00");
        open(&mut s, "b", "2026-10-19T09:20:00+02:00");
        sync_tray(&mut tray, &s);
        let ids: Vec<_> = tray.items.iter().map(|i| i.doc_id.as_str()).collect();
        assert_eq!(ids, ["b", "a", "saved"]);
        assert_eq!(tray.active.as_deref(), Some("b"));
        assert_eq!(tray.items[0].title, "Capture 09:20");
        assert!(tray.items[0].unsaved);

        // Activating another document moves the highlight, not the order.
        s.activate(0).unwrap();
        sync_tray(&mut tray, &s);
        assert_eq!(tray.active.as_deref(), Some("a"));
        assert_eq!(tray.items.len(), 3);
    }
}
