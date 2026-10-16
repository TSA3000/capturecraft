//! Preferences dialog: a section list beside a page of settings, the shape
//! PhotoCraft's Edit › Preferences uses (`ui-egui/src/prefs_ui.rs`, whose
//! sections come from `engine::prefs::SECTIONS`). The sections here are the
//! ones a capture tool has, and the controls match Snagit's Preferences
//! windows: Capture, Save, Interface, Hotkeys, Advanced.

use crate::autostart;
use crate::config::{Config, SelectMode, config_path};
use crate::theme::{self, ThemeKind};
use eframe::egui::{self, RichText};

/// Section ids and titles, like PhotoCraft's `SECTIONS`.
pub const SECTIONS: [(&str, &str); 5] = [
    ("capture", "Capture"),
    ("save", "Save"),
    ("interface", "Interface"),
    ("hotkeys", "Hotkeys"),
    ("advanced", "Advanced"),
];

/// Hotkey presets offered in Preferences, shared with the window's tests.
pub const HOTKEYS: &[&str] = &[
    "PrintScreen",
    "ctrl+PrintScreen",
    "alt+PrintScreen",
    "ctrl+shift+s",
    "ctrl+shift+2",
];

/// Open Preferences, its selected section, and whether it has unsaved edits.
#[derive(Default)]
pub struct Prefs {
    pub open: bool,
    section: usize,
    /// Edited copy; applied to disk when a control changes.
    draft: Option<Config>,
}

impl Prefs {
    pub fn open(&mut self, config: &Config) {
        self.open = true;
        self.draft = Some(config.clone());
    }

    /// Draws Preferences as a modal popup inside the window it is called
    /// from: a dimmed backdrop, input blocked behind it, Esc to close.
    /// Returns the saved config when something changed.
    pub fn show(&mut self, ctx: &egui::Context, config: &Config) -> Option<Config> {
        if !self.open {
            return None;
        }
        let mut draft = self.draft.take().unwrap_or_else(|| config.clone());
        let mut changed = false;
        let mut section = self.section;

        let response = egui::Modal::new(egui::Id::new("capturecraft-prefs"))
            .backdrop_color(theme::tokens(ctx).scrim)
            .show(ctx, |ui| {
                // Fit the window we are in: it grows to MODAL_SIZE over the
                // next frame, and the popup must never be clipped meanwhile.
                let room = ctx.content_rect().size();
                let width = (room.x - 48.0).clamp(MIN_MODAL.x, MODAL_SIZE.x);
                let height = (room.y - 96.0).clamp(MIN_MODAL.y, MODAL_SIZE.y);
                ui.set_width(width);
                ui.set_height(height);
                ui.horizontal(|ui| {
                    ui.heading("Preferences");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            ui.close();
                        }
                    });
                });
                ui.separator();
                ui.add_space(4.0);

                ui.horizontal_top(|ui| {
                    // Section list on the left, page on the right.
                    ui.vertical(|ui| {
                        ui.set_min_width(120.0);
                        ui.set_max_width(120.0);
                        for (i, (_, title)) in SECTIONS.iter().enumerate() {
                            if ui.selectable_label(section == i, *title).clicked() {
                                section = i;
                            }
                        }
                    });
                    ui.separator();
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let id = SECTIONS
                                .get(section)
                                .map(|(id, _)| *id)
                                .unwrap_or("capture");
                            changed = match id {
                                "capture" => capture_page(ui, &mut draft),
                                "save" => save_page(ui, &mut draft),
                                "interface" => interface_page(ui, &mut draft),
                                "hotkeys" => hotkeys_page(ui, &mut draft),
                                _ => advanced_page(ui, &mut draft),
                            };
                        });
                });
            });

        // Esc, the Close button, or a click on the backdrop.
        let close = response.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape));

        self.section = section;
        if close {
            self.open = false;
            self.draft = None;
        } else {
            self.draft = Some(draft.clone());
        }

        let result = changed.then_some(draft);
        if let Some(saved) = &result
            && let Err(e) = saved.save_at(&config_path())
        {
            eprintln!("failed to save preferences: {e:#}");
        }
        result
    }
}

/// Preferred inner size of the Preferences popup.
pub const MODAL_SIZE: egui::Vec2 = egui::vec2(560.0, 300.0);
/// Smallest it may shrink to while the window is still growing.
const MIN_MODAL: egui::Vec2 = egui::vec2(380.0, 200.0);

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).strong());
    ui.add_space(4.0);
}

fn grid(ui: &mut egui::Ui, id: &str, build: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .min_col_width(120.0)
        .spacing([14.0, 10.0])
        .show(ui, build);
}

/// Slider sized so every page's controls line up.
fn slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
) -> bool {
    ui.spacing_mut().slider_width = 120.0;
    ui.add(egui::Slider::new(value, range).suffix(suffix.to_string()))
        .changed()
}

fn capture_page(ui: &mut egui::Ui, config: &mut Config) -> bool {
    let mut changed = false;
    heading(ui, "Capture");
    grid(ui, "prefs_capture", |ui| {
        ui.label("Selection");
        let before = config.selection;
        egui::ComboBox::from_id_salt("prefs_selection")
            .width(190.0)
            .selected_text(before.label())
            .show_ui(ui, |ui| {
                for mode in SelectMode::ALL {
                    ui.selectable_value(&mut config.selection, mode, mode.label());
                }
            });
        changed |= config.selection != before;
        ui.end_row();

        ui.label("Copy to clipboard");
        changed |= theme::toggle(ui, &mut config.clipboard).changed();
        ui.end_row();

        ui.label("Open in editor");
        changed |= theme::toggle(ui, &mut config.editor).changed();
        ui.end_row();

        ui.label("Zoom delay");
        let mut delay = config.overlay.zoom_delay_ms as f32;
        if slider(ui, &mut delay, 0.0..=2000.0, " ms") {
            config.overlay.zoom_delay_ms = delay as u64;
            changed = true;
        }
        ui.end_row();

        ui.label("Dim while dragging only");
        changed |= theme::toggle(ui, &mut config.overlay.dim_on_drag).changed();
        ui.end_row();

        ui.label("Dim level");
        let mut dim = config.overlay.dim as f32;
        if slider(ui, &mut dim, 16.0..=255.0, "") {
            config.overlay.dim = dim as u32;
            changed = true;
        }
        ui.end_row();
    });
    changed
}

fn save_page(ui: &mut egui::Ui, config: &mut Config) -> bool {
    let mut changed = false;
    heading(ui, "Save");
    grid(ui, "prefs_save", |ui| {
        ui.label("Save to");
        ui.horizontal(|ui| {
            let mut dir = config.output_dir.display().to_string();
            if ui
                .add(egui::TextEdit::singleline(&mut dir).desired_width(230.0))
                .changed()
            {
                config.output_dir = dir.into();
                changed = true;
            }
            if ui.small_button("Open").clicked() {
                let _ = std::fs::create_dir_all(&config.output_dir);
                crate::tray::open_path(&config.output_dir);
            }
        });
        ui.end_row();
    });
    ui.add_space(6.0);
    ui.label(
        RichText::new("Captures are named capture-YYYY-MM-DD_HH-MM-SS.png\nand never overwrite.")
            .small()
            .weak(),
    );
    changed
}

fn interface_page(ui: &mut egui::Ui, config: &mut Config) -> bool {
    let mut changed = false;
    heading(ui, "Interface");
    grid(ui, "prefs_interface", |ui| {
        ui.label("Theme");
        let before = config.theme;
        egui::ComboBox::from_id_salt("prefs_theme")
            .width(190.0)
            .selected_text(before.label())
            .show_ui(ui, |ui| {
                for kind in ThemeKind::ALL {
                    ui.selectable_value(&mut config.theme, kind, kind.label());
                }
            });
        changed |= config.theme != before;
        ui.end_row();

        ui.label("Selection colour");
        let mut rgb = color_to_rgb(theme::tokens(ui.ctx()).accent, &config.overlay.border_color);
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            config.overlay.border_color = format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
            changed = true;
        }
        ui.end_row();

        ui.label("Border width");
        let mut width = config.overlay.border_width as f32;
        if slider(ui, &mut width, 1.0..=8.0, " px") {
            config.overlay.border_width = width as u32;
            changed = true;
        }
        ui.end_row();

        ui.label("Magnifier zoom");
        let mut zoom = config.overlay.zoom as f32;
        if slider(ui, &mut zoom, 2.0..=16.0, "×") {
            config.overlay.zoom = zoom as u32;
            changed = true;
        }
        ui.end_row();
    });
    changed
}

fn hotkeys_page(ui: &mut egui::Ui, config: &mut Config) -> bool {
    let mut changed = false;
    heading(ui, "Hotkeys");
    grid(ui, "prefs_hotkeys", |ui| {
        ui.label("Capture");
        let current = config.hotkeys.first().cloned().unwrap_or_default();
        let mut selected = current.clone();
        egui::ComboBox::from_id_salt("prefs_hotkey")
            .width(190.0)
            .selected_text(&current)
            .show_ui(ui, |ui| {
                for preset in HOTKEYS {
                    ui.selectable_value(&mut selected, preset.to_string(), *preset);
                }
                if !HOTKEYS.contains(&current.as_str()) && !current.is_empty() {
                    ui.selectable_value(&mut selected, current.clone(), &current);
                }
            });
        if selected != current {
            config.hotkeys = crate::app::hotkey_list(&selected);
            changed = true;
        }
        ui.end_row();

        ui.label("Fallback");
        ui.label(
            RichText::new(config.hotkeys.get(1).map(String::as_str).unwrap_or("none"))
                .weak()
                .small(),
        );
        ui.end_row();
    });
    ui.add_space(6.0);
    ui.label(
        RichText::new(
            "If the first hotkey is taken by another program,\nthe fallback is used instead.",
        )
        .small()
        .weak(),
    );
    changed
}

fn advanced_page(ui: &mut egui::Ui, config: &mut Config) -> bool {
    let mut changed = false;
    heading(ui, "Advanced");
    grid(ui, "prefs_advanced", |ui| {
        ui.label("Start tray with app");
        changed |= theme::toggle(ui, &mut config.start_tray).changed();
        ui.end_row();

        ui.label(autostart::label());
        let mut enabled = autostart::is_enabled();
        if theme::toggle(ui, &mut enabled).changed()
            && let Err(e) = autostart::set_enabled(enabled)
        {
            eprintln!("{e:#}");
        }
        ui.end_row();

        ui.label("Capture delay");
        let mut delay = config.overlay.hide_delay_ms as f32;
        if slider(ui, &mut delay, 0.0..=3000.0, " ms") {
            config.overlay.hide_delay_ms = delay as u64;
            changed = true;
        }
        ui.end_row();

        ui.label("Config file");
        ui.horizontal(|ui| {
            if ui
                .button("Edit config.toml")
                .on_hover_text(config_path().display().to_string())
                .clicked()
            {
                crate::tray::open_path(&config_path());
            }
        });
        ui.end_row();
    });
    changed
}

/// Parses `#RRGGBB` for the colour button, falling back to the theme accent.
fn color_to_rgb(accent: egui::Color32, text: &str) -> [u8; 3] {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() == 6
        && let Ok(value) = u32::from_str_radix(hex, 16)
    {
        return [
            ((value >> 16) & 0xFF) as u8,
            ((value >> 8) & 0xFF) as u8,
            (value & 0xFF) as u8,
        ];
    }
    [accent.r(), accent.g(), accent.b()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_are_unique() {
        for (i, (id, _)) in SECTIONS.iter().enumerate() {
            assert!(!SECTIONS.iter().skip(i + 1).any(|(other, _)| other == id));
        }
    }

    #[test]
    fn color_parsing() {
        let fallback = egui::Color32::from_rgb(1, 2, 3);
        assert_eq!(color_to_rgb(fallback, "#FF8800"), [255, 136, 0]);
        assert_eq!(color_to_rgb(fallback, "00ff00"), [0, 255, 0]);
        assert_eq!(color_to_rgb(fallback, "bad"), [1, 2, 3]);
    }
}
