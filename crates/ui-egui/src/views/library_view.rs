//! Zone 5: the Library, a searchable grid (or list) of past captures shown
//! over the canvas.
//!
//! Items come from the Recent Tray's documents (`TrayItem`, which carries
//! the capture's tags and source app). The search box matches title, tags
//! and app; tag chips, the media kind, the date range and Favorites narrow
//! the grid. A click selects (Ctrl/Cmd toggles), a double-click opens the
//! capture in the editor, Delete removes the selection. The filtering is
//! the pure [`library_items`], unit-tested without a display.

use egui::{Align2, FontId, Rect as ERect, Sense, StrokeKind, Ui, pos2, vec2};

use crate::UiIntent;
use crate::panels::recent_tray::{Thumbnail, fit_rect};
use crate::state::{
    DateFilter, LibraryState, LibraryView as ViewMode, MediaKind, SortField, TrayItem,
};
use crate::theme::{Tokens, stroke};

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The day of an ISO-8601 timestamp's `YYYY-MM-DD` prefix.
pub fn day_number(stamp: &str) -> Option<i64> {
    let date = stamp.get(..10)?;
    let mut it = date.split('-');
    let y = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then(|| days_from_civil(y, m, d))
}

fn date_matches(f: &DateFilter, captured_at: &str, today: &str) -> bool {
    let (Some(day), Some(now)) = (day_number(captured_at), day_number(today)) else {
        return matches!(f, DateFilter::Any);
    };
    match f {
        DateFilter::Any => true,
        DateFilter::Today => day == now,
        DateFilter::Last7Days => (now - 6..=now).contains(&day),
        DateFilter::Last30Days => (now - 29..=now).contains(&day),
        DateFilter::Range { from, to } => {
            day_number(from).is_none_or(|f| day >= f) && day_number(to).is_none_or(|t| day <= t)
        }
    }
}

/// The items the Library shows, filtered and sorted. `today` is the local
/// date (`YYYY-MM-DD`) the relative date filters count from.
pub fn library_items<'a>(
    items: &'a [TrayItem],
    lib: &LibraryState,
    today: &str,
) -> Vec<&'a TrayItem> {
    let q = lib.query.trim().to_lowercase();
    let f = &lib.filters;
    let mut out: Vec<&TrayItem> = items
        .iter()
        .filter(|i| {
            q.is_empty()
                || i.title.to_lowercase().contains(&q)
                || i.app_name.to_lowercase().contains(&q)
                || i.tags.iter().any(|t| t.to_lowercase().contains(&q))
        })
        .filter(|i| f.tags.iter().all(|t| i.tags.contains(t)))
        .filter(|i| f.kind.is_none_or(|k| i.kind == k))
        .filter(|i| f.app.as_ref().is_none_or(|a| &i.app_name == a))
        .filter(|i| !f.favorites_only || i.favorite)
        .filter(|i| !f.simplified_only || i.simplified)
        .filter(|i| date_matches(&f.date, &i.captured_at, today))
        .collect();
    let key = |i: &TrayItem| -> String {
        match lib.sort.field {
            SortField::Date => i.captured_at.clone(),
            SortField::Title => i.title.to_lowercase(),
            SortField::App | SortField::Website => i.app_name.to_lowercase(),
        }
    };
    out.sort_by_key(|i| key(i));
    if lib.sort.descending {
        out.reverse();
    }
    out
}

/// Every source app in `items`, sorted, once.
pub fn all_apps(items: &[TrayItem]) -> Vec<String> {
    let mut apps: Vec<String> = items
        .iter()
        .map(|i| i.app_name.clone())
        .filter(|a| !a.is_empty())
        .collect();
    apps.sort();
    apps.dedup();
    apps
}

/// Every tag in `items`, sorted, once.
pub fn all_tags(items: &[TrayItem]) -> Vec<String> {
    let mut tags: Vec<String> = items.iter().flat_map(|i| i.tags.iter().cloned()).collect();
    tags.sort();
    tags.dedup();
    tags
}

/// Click (toggle with Ctrl/Cmd) on a card.
pub fn click(selected: &mut Vec<String>, doc_id: &str, toggle: bool) {
    if toggle {
        if let Some(i) = selected.iter().position(|d| d == doc_id) {
            selected.remove(i);
        } else {
            selected.push(doc_id.into());
        }
    } else {
        *selected = vec![doc_id.into()];
    }
}

pub struct LibraryPanel<'a> {
    pub items: &'a [TrayItem],
    pub thumbnail: &'a dyn Fn(&str) -> Option<Thumbnail>,
    /// Local date, `YYYY-MM-DD`.
    pub today: &'a str,
    pub tokens: &'a Tokens,
}

impl LibraryPanel<'_> {
    pub fn show(self, ui: &mut Ui, lib: &mut LibraryState, intents: &mut Vec<UiIntent>) {
        let t = self.tokens;
        ui.horizontal(|ui| {
            ui.heading("Library");
            ui.add(
                egui::TextEdit::singleline(&mut lib.query)
                    .hint_text("🔍 Search titles, tags, apps")
                    .desired_width(220.0),
            );
            egui::ComboBox::from_id_salt("library.kind")
                .selected_text(lib.filters.kind.map_or("All media", MediaKind::label))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut lib.filters.kind, None, "All media");
                    for k in [
                        MediaKind::Image,
                        MediaKind::Panorama,
                        MediaKind::Video,
                        MediaKind::Gif,
                    ] {
                        ui.selectable_value(&mut lib.filters.kind, Some(k), k.label());
                    }
                });
            egui::ComboBox::from_id_salt("library.date")
                .selected_text(date_label(&lib.filters.date))
                .show_ui(ui, |ui| {
                    for d in [
                        DateFilter::Any,
                        DateFilter::Today,
                        DateFilter::Last7Days,
                        DateFilter::Last30Days,
                    ] {
                        let label = date_label(&d);
                        ui.selectable_value(&mut lib.filters.date, d, label);
                    }
                });
            let apps = all_apps(self.items);
            if !apps.is_empty() {
                egui::ComboBox::from_id_salt("library.app")
                    .selected_text(lib.filters.app.as_deref().unwrap_or("All apps"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut lib.filters.app, None, "All apps");
                        for a in apps {
                            let label = a.clone();
                            ui.selectable_value(&mut lib.filters.app, Some(a), label);
                        }
                    });
            }
            ui.checkbox(&mut lib.filters.simplified_only, "◫ Simplified");
            ui.checkbox(&mut lib.filters.favorites_only, "★ Favorites");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("✖ Close").clicked() {
                    lib.visible = false;
                }
                ui.selectable_value(&mut lib.view, ViewMode::List, "☰");
                ui.selectable_value(&mut lib.view, ViewMode::Grid, "⊞");
                ui.add(egui::Slider::new(&mut lib.thumb_size_px, 96.0..=320.0).show_value(false));
                egui::ComboBox::from_id_salt("library.sort")
                    .selected_text(sort_label(lib.sort.field))
                    .show_ui(ui, |ui| {
                        for s in [SortField::Date, SortField::Title, SortField::App] {
                            ui.selectable_value(&mut lib.sort.field, s, sort_label(s));
                        }
                    });
                let dir = if lib.sort.descending { "↓" } else { "↑" };
                if ui.small_button(dir).clicked() {
                    lib.sort.descending = !lib.sort.descending;
                }
            });
        });
        let tags = all_tags(self.items);
        if !tags.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for tag in tags {
                    let on = lib.filters.tags.contains(&tag);
                    if ui.selectable_label(on, format!("# {tag}")).clicked() {
                        if on {
                            lib.filters.tags.retain(|t| *t != tag);
                        } else {
                            lib.filters.tags.push(tag);
                        }
                    }
                }
            });
        }
        ui.separator();

        let shown = library_items(self.items, lib, self.today);
        if shown.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new("No captures match").color(t.text_faint));
            });
            return;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Delete)) && !lib.selected.is_empty() {
            intents.push(UiIntent::DeleteDocuments {
                doc_ids: lib.selected.clone(),
            });
        }
        let toggle = ui.input(|i| i.modifiers.command);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match lib.view {
                ViewMode::Grid => {
                    let side = lib.thumb_size_px;
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(t.space_md, t.space_md);
                        for item in &shown {
                            self.card(ui, item, side, lib, toggle, intents);
                        }
                    });
                }
                ViewMode::List => {
                    for item in &shown {
                        self.row(ui, item, lib, toggle, intents);
                    }
                }
            });
    }

    fn card(
        &self,
        ui: &mut Ui,
        item: &TrayItem,
        side: f32,
        lib: &mut LibraryState,
        toggle: bool,
        intents: &mut Vec<UiIntent>,
    ) {
        let t = self.tokens;
        let caption_h = 36.0;
        let (rect, resp) =
            ui.allocate_exact_size(vec2(side, side * 0.75 + caption_h), Sense::click());
        let selected = lib.selected.contains(&item.doc_id);
        let p = ui.painter();
        p.rect(
            rect,
            Tokens::corner(t.radius),
            if resp.hovered() { t.card_hover } else { t.card },
            stroke(
                if selected { 2.0 } else { 1.0 },
                if selected {
                    t.card_selected_border
                } else {
                    t.card_border
                },
            ),
            StrokeKind::Inside,
        );
        let thumb = ERect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - caption_h))
            .shrink(t.space_sm);
        match (self.thumbnail)(&item.doc_id) {
            Some(th) => {
                ui.painter().image(
                    th.texture,
                    fit_rect(th.size, thumb),
                    ERect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                ui.painter().text(
                    thumb.center(),
                    Align2::CENTER_CENTER,
                    item.kind.label(),
                    FontId::proportional(12.0),
                    t.text_faint,
                );
            }
        }
        let p = ui.painter();
        let cap = pos2(rect.min.x + t.space_sm, rect.max.y - caption_h + t.space_xs);
        p.text(
            cap,
            Align2::LEFT_TOP,
            elide(&item.title, 28),
            FontId::proportional(12.0),
            t.text,
        );
        p.text(
            cap + vec2(0.0, 16.0),
            Align2::LEFT_TOP,
            item.captured_at.get(..10).unwrap_or(&item.captured_at),
            FontId::proportional(10.5),
            t.text_dim,
        );
        if item.favorite {
            p.text(
                pos2(rect.max.x - t.space_sm, cap.y),
                Align2::RIGHT_TOP,
                "★",
                FontId::proportional(12.0),
                t.dirty,
            );
        }
        self.interact(resp, item, lib, toggle, intents);
    }

    fn row(
        &self,
        ui: &mut Ui,
        item: &TrayItem,
        lib: &mut LibraryState,
        toggle: bool,
        intents: &mut Vec<UiIntent>,
    ) {
        let selected = lib.selected.contains(&item.doc_id);
        let text = format!(
            "{}   ·   {}   ·   {}   {}",
            item.title,
            item.kind.label(),
            item.captured_at,
            item.tags
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let resp = ui.add(egui::Button::selectable(selected, text).sense(Sense::click()));
        self.interact(resp, item, lib, toggle, intents);
    }

    fn interact(
        &self,
        resp: egui::Response,
        item: &TrayItem,
        lib: &mut LibraryState,
        toggle: bool,
        intents: &mut Vec<UiIntent>,
    ) {
        if resp.double_clicked() {
            intents.push(UiIntent::ActivateDocument {
                doc_id: item.doc_id.clone(),
            });
            lib.visible = false;
        } else if resp.clicked() {
            click(&mut lib.selected, &item.doc_id, toggle);
        }
        resp.on_hover_text(item.app_name.as_str())
            .context_menu(|ui| {
                if ui.button("Open in editor").clicked() {
                    intents.push(UiIntent::ActivateDocument {
                        doc_id: item.doc_id.clone(),
                    });
                    lib.visible = false;
                    ui.close();
                }
                if ui.button("Delete").clicked() {
                    let ids = if lib.selected.contains(&item.doc_id) {
                        lib.selected.clone()
                    } else {
                        vec![item.doc_id.clone()]
                    };
                    intents.push(UiIntent::DeleteDocuments { doc_ids: ids });
                    ui.close();
                }
            });
    }
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

pub fn date_label(d: &DateFilter) -> &'static str {
    match d {
        DateFilter::Any => "Any date",
        DateFilter::Today => "Today",
        DateFilter::Last7Days => "Last 7 days",
        DateFilter::Last30Days => "Last 30 days",
        DateFilter::Range { .. } => "Custom range",
    }
}

pub fn sort_label(s: SortField) -> &'static str {
    match s {
        SortField::Date => "Date",
        SortField::Title => "Title",
        SortField::App => "App",
        SortField::Website => "Website",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, title: &str, at: &str, tags: &[&str]) -> TrayItem {
        TrayItem {
            doc_id: id.into(),
            title: title.into(),
            captured_at: at.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            app_name: "Firefox".into(),
            ..TrayItem::default()
        }
    }

    fn items() -> Vec<TrayItem> {
        vec![
            item("a", "Login form", "2026-10-19T09:00:00", &["bug", "web"]),
            item("b", "Settings dialog", "2026-10-15T12:00:00", &["docs"]),
            item("c", "Old chart", "2026-08-01T08:00:00", &["web"]),
        ]
    }

    fn ids(v: Vec<&TrayItem>) -> Vec<&str> {
        v.into_iter().map(|i| i.doc_id.as_str()).collect()
    }

    #[test]
    fn app_and_simplified_filters() {
        let mut items = items();
        items[1].app_name = "Explorer".into();
        items[2].simplified = true;
        assert_eq!(all_apps(&items), ["Explorer", "Firefox"]);
        let mut lib = LibraryState::default();
        lib.filters.app = Some("Firefox".into());
        let mut got = ids(library_items(&items, &lib, "2026-10-19"));
        got.sort();
        assert_eq!(got, ["a", "c"]);
        lib.filters.simplified_only = true;
        assert_eq!(ids(library_items(&items, &lib, "2026-10-19")), ["c"]);
    }

    #[test]
    fn search_tags_and_dates_narrow_the_grid() {
        let items = items();
        let mut lib = LibraryState::default();
        assert_eq!(
            ids(library_items(&items, &lib, "2026-10-19")),
            ["a", "b", "c"]
        );
        lib.query = "WEB".into();
        assert_eq!(ids(library_items(&items, &lib, "2026-10-19")), ["a", "c"]);
        lib.query.clear();
        lib.filters.tags = vec!["web".into(), "bug".into()];
        assert_eq!(ids(library_items(&items, &lib, "2026-10-19")), ["a"]);
        lib.filters.tags.clear();
        lib.filters.date = DateFilter::Last7Days;
        assert_eq!(ids(library_items(&items, &lib, "2026-10-19")), ["a", "b"]);
        lib.filters.date = DateFilter::Today;
        assert_eq!(ids(library_items(&items, &lib, "2026-10-19")), ["a"]);
        lib.filters.date = DateFilter::Range {
            from: "2026-08-01".into(),
            to: "2026-10-15".into(),
        };
        assert_eq!(ids(library_items(&items, &lib, "2026-10-19")), ["b", "c"]);
    }

    #[test]
    fn sorting_by_title_and_date() {
        let items = items();
        let mut lib = LibraryState::default();
        lib.sort.field = SortField::Title;
        lib.sort.descending = false;
        assert_eq!(
            ids(library_items(&items, &lib, "2026-10-19")),
            ["a", "c", "b"]
        );
        assert_eq!(all_tags(&items), ["bug", "docs", "web"]);
    }

    #[test]
    fn dates_count_days_across_months_and_years() {
        assert_eq!(day_number("1970-01-01"), Some(0));
        assert_eq!(
            day_number("2026-03-01")
                .zip(day_number("2026-02-28"))
                .map(|(a, b)| a - b),
            Some(1)
        );
        assert_eq!(
            day_number("2025-01-01T00:00")
                .zip(day_number("2024-12-31"))
                .map(|(a, b)| a - b),
            Some(1)
        );
        assert_eq!(day_number("garbage"), None);
    }

    #[test]
    fn clicks_select_and_toggle() {
        let mut sel = Vec::new();
        click(&mut sel, "a", false);
        click(&mut sel, "b", true);
        assert_eq!(sel, ["a", "b"]);
        click(&mut sel, "a", true);
        assert_eq!(sel, ["b"]);
        click(&mut sel, "c", false);
        assert_eq!(sel, ["c"]);
        assert_eq!(elide("abcdef", 4), "abc…");
    }
}
