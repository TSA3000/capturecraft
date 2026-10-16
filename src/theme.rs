//! Design tokens, ported from PhotoCraft's `ui-egui/src/theme.rs` (v0.9.0).
//!
//! Only the four themes a capture tool needs are carried over, with the same
//! names and the same colour values, so CaptureCraft sits next to PhotoCraft
//! rather than near it. Widgets read [`Tokens`] instead of hard-coding
//! colours, exactly as their rule requires.

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Visuals};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemeKind {
    /// Photoshop-style darkest brightness: flat charcoal panels, blue accent.
    Pro,
    /// Photoshop's default brightness: #535353 panels.
    #[default]
    ProMedium,
    /// Near-black surfaces, rounded cards, soft violet accent.
    Studio,
    /// The Studio system on light surfaces.
    StudioLight,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 4] = [
        ThemeKind::Pro,
        ThemeKind::ProMedium,
        ThemeKind::Studio,
        ThemeKind::StudioLight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::Pro => "Pro (Dark)",
            ThemeKind::ProMedium => "Pro (Medium Gray)",
            ThemeKind::Studio => "Studio (Dark)",
            ThemeKind::StudioLight => "Studio (Light)",
        }
    }

    /// Photoshop-grammar layout: flat panels, square-ish corners.
    pub fn is_pro(self) -> bool {
        matches!(self, ThemeKind::Pro | ThemeKind::ProMedium)
    }

    pub fn dark(self) -> bool {
        self != ThemeKind::StudioLight
    }
}

/// Colour and shape tokens for one theme.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub kind: ThemeKind,
    /// Window chrome (title bar, toolbars).
    pub chrome: Color32,
    /// Canvas surround.
    pub canvas: Color32,
    pub card: Color32,
    pub card_border: Color32,
    pub field: Color32,
    pub field_border: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub accent_border: Color32,
    pub accent_text: Color32,
    pub separator: Color32,
    pub shadow: Color32,
    pub primary_bg: Color32,
    pub primary_text: Color32,
    pub danger: Color32,
    pub warning: Color32,
    pub caption_close: Color32,
    /// Overlay drawn over a window while a modal dialog owns the input.
    pub scrim: Color32,
    pub radius_sm: f32,
    pub radius: f32,
    pub radius_lg: f32,
    pub pro: bool,
}

impl Tokens {
    pub fn for_kind(kind: ThemeKind) -> Tokens {
        let pro = Tokens {
            kind: ThemeKind::Pro,
            chrome: Color32::from_rgb(50, 50, 50),
            canvas: Color32::from_rgb(40, 40, 40),
            card: Color32::from_rgb(50, 50, 50),
            card_border: Color32::from_rgb(30, 30, 30),
            field: Color32::from_rgb(36, 36, 36),
            field_border: Color32::from_rgb(74, 74, 74),
            hover: Color32::from_rgb(66, 66, 66),
            pressed: Color32::from_rgb(78, 78, 78),
            text: Color32::from_rgb(222, 222, 222),
            text_dim: Color32::from_rgb(178, 178, 178),
            text_faint: Color32::from_rgb(128, 128, 128),
            accent: Color32::from_rgb(55, 142, 240),
            accent_soft: Color32::from_rgb(78, 78, 78),
            accent_border: Color32::from_rgb(55, 142, 240),
            accent_text: Color32::WHITE,
            separator: Color32::from_rgb(30, 30, 30),
            shadow: Color32::from_black_alpha(150),
            primary_bg: Color32::from_rgb(55, 142, 240),
            primary_text: Color32::WHITE,
            danger: Color32::from_rgb(236, 91, 98),
            warning: Color32::from_rgb(232, 176, 70),
            caption_close: Color32::from_rgb(196, 43, 28),
            scrim: Color32::from_black_alpha(110),
            radius_sm: 3.0,
            radius: 4.0,
            radius_lg: 6.0,
            pro: true,
        };
        let studio = Tokens {
            kind: ThemeKind::Studio,
            chrome: Color32::from_rgb(20, 20, 21),
            canvas: Color32::from_rgb(14, 14, 15),
            card: Color32::from_rgb(26, 26, 28),
            card_border: Color32::from_rgb(40, 40, 44),
            field: Color32::from_rgb(35, 35, 38),
            field_border: Color32::from_rgb(52, 52, 57),
            hover: Color32::from_rgb(44, 44, 48),
            pressed: Color32::from_rgb(56, 56, 62),
            text: Color32::from_rgb(236, 236, 240),
            text_dim: Color32::from_rgb(150, 150, 158),
            text_faint: Color32::from_rgb(96, 96, 104),
            accent: Color32::from_rgb(139, 124, 246),
            accent_soft: Color32::from_rgba_unmultiplied(139, 124, 246, 46),
            accent_border: Color32::from_rgba_unmultiplied(160, 148, 255, 110),
            accent_text: Color32::from_rgb(214, 208, 255),
            separator: Color32::from_rgb(38, 38, 42),
            shadow: Color32::from_black_alpha(140),
            primary_bg: Color32::from_rgb(246, 246, 248),
            primary_text: Color32::from_rgb(12, 12, 14),
            danger: Color32::from_rgb(240, 96, 96),
            warning: Color32::from_rgb(240, 190, 90),
            caption_close: Color32::from_rgb(196, 43, 28),
            scrim: Color32::from_black_alpha(130),
            radius_sm: 6.0,
            radius: 8.0,
            radius_lg: 12.0,
            pro: false,
        };
        match kind {
            ThemeKind::Pro => pro,
            ThemeKind::ProMedium => Tokens {
                kind,
                chrome: Color32::from_rgb(83, 83, 83),
                card: Color32::from_rgb(83, 83, 83),
                card_border: Color32::from_rgb(66, 66, 66),
                field: Color32::from_rgb(69, 69, 69),
                field_border: Color32::from_rgb(104, 104, 104),
                hover: Color32::from_rgb(98, 98, 98),
                pressed: Color32::from_rgb(112, 112, 112),
                text: Color32::from_rgb(238, 238, 238),
                text_dim: Color32::from_rgb(212, 212, 212),
                text_faint: Color32::from_rgb(160, 160, 160),
                accent_soft: Color32::from_rgb(110, 110, 110),
                separator: Color32::from_rgb(62, 62, 62),
                ..pro
            },
            ThemeKind::Studio => studio,
            ThemeKind::StudioLight => Tokens {
                kind,
                chrome: Color32::from_rgb(246, 246, 248),
                canvas: Color32::from_rgb(226, 226, 230),
                card: Color32::from_rgb(252, 252, 253),
                card_border: Color32::from_rgb(222, 222, 228),
                field: Color32::from_rgb(242, 242, 245),
                field_border: Color32::from_rgb(214, 214, 220),
                hover: Color32::from_rgb(232, 232, 237),
                pressed: Color32::from_rgb(220, 220, 226),
                text: Color32::from_rgb(24, 24, 28),
                text_dim: Color32::from_rgb(96, 96, 106),
                text_faint: Color32::from_rgb(150, 150, 160),
                accent: Color32::from_rgb(108, 92, 231),
                accent_soft: Color32::from_rgba_unmultiplied(108, 92, 231, 36),
                accent_border: Color32::from_rgba_unmultiplied(108, 92, 231, 120),
                accent_text: Color32::from_rgb(80, 64, 200),
                separator: Color32::from_rgb(226, 226, 232),
                shadow: Color32::from_black_alpha(50),
                primary_bg: Color32::from_rgb(20, 20, 24),
                primary_text: Color32::from_rgb(250, 250, 252),
                scrim: Color32::from_black_alpha(70),
                danger: Color32::from_rgb(210, 60, 60),
                warning: Color32::from_rgb(190, 130, 20),
                ..studio
            },
        }
    }

    pub fn dark(&self) -> bool {
        self.kind.dark()
    }
}

/// Tokens of the theme currently applied to `ctx`.
pub fn tokens(ctx: &egui::Context) -> Tokens {
    ctx.data(|d| d.get_temp::<Tokens>(egui::Id::new("capturecraft-theme")))
        .unwrap_or_else(|| Tokens::for_kind(ThemeKind::default()))
}

/// Applies a theme to the whole context.
pub fn apply(ctx: &egui::Context, kind: ThemeKind) {
    let t = Tokens::for_kind(kind);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("capturecraft-theme"), t));

    let mut v = if t.dark() {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = t.chrome;
    v.window_fill = t.card;
    v.window_stroke = Stroke::new(1.0_f32, t.card_border);
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.card;
    v.code_bg_color = t.field;
    v.override_text_color = Some(t.text);
    v.hyperlink_color = t.accent;
    v.warn_fg_color = t.warning;
    v.error_fg_color = t.danger;
    v.window_corner_radius = CornerRadius::same(t.radius_lg as u8);
    v.menu_corner_radius = CornerRadius::same(t.radius as u8);
    v.window_shadow = egui::Shadow {
        offset: [0, 10],
        blur: 32,
        spread: 0,
        color: t.shadow,
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: t.shadow,
    };
    v.selection.bg_fill = if t.pro { t.accent } else { t.accent_soft };
    v.selection.stroke = Stroke::new(1.0_f32, t.accent_text);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.striped = false;

    let r = CornerRadius::same(t.radius_sm as u8);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.card;
    w.noninteractive.weak_bg_fill = t.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, t.separator);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, t.text_dim);
    w.noninteractive.corner_radius = r;
    for (wv, bg, stroke) in [
        (&mut w.inactive, t.field, t.field_border),
        (&mut w.hovered, t.hover, t.field_border),
        (&mut w.active, t.pressed, t.accent_border),
        (&mut w.open, t.hover, t.field_border),
    ] {
        wv.bg_fill = bg;
        wv.weak_bg_fill = bg;
        wv.bg_stroke = Stroke::new(1.0_f32, stroke);
        wv.fg_stroke = Stroke::new(1.0_f32, t.text);
        wv.corner_radius = r;
        wv.expansion = 0.0;
    }
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(10.5)),
            (
                TextStyle::Body,
                FontId::proportional(if t.pro { 12.0 } else { 12.5 }),
            ),
            (
                TextStyle::Button,
                FontId::proportional(if t.pro { 12.0 } else { 12.5 }),
            ),
            (TextStyle::Heading, FontId::proportional(15.0)),
            (TextStyle::Monospace, FontId::monospace(12.0)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.spacing.interact_size = egui::vec2(24.0, 24.0);
        s.spacing.slider_width = 150.0;
        s.spacing.combo_width = 120.0;
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.icon_width = 14.0;
        s.visuals.indent_has_left_vline = false;
        s.interaction.tooltip_delay = TOOLTIP_DELAY;
    });

    // egui picks its style branch from the OS appearance; both branches need
    // the chosen palette so the theme does not flip with the system setting.
    let style = ctx.style();
    ctx.set_style_of(egui::Theme::Light, Arc::clone(&style));
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// Dims a window while a modal dialog owns the input, and swallows clicks
/// on it, the way PhotoCraft's modal scrim does.
pub fn scrim(ctx: &egui::Context) -> egui::Response {
    let t = tokens(ctx);
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("capturecraft-scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .interactable(true)
        .show(ctx, |ui| {
            let response = ui.allocate_response(screen.size(), egui::Sense::click_and_drag());
            ui.painter().rect_filled(screen, 0.0, t.scrim);
            response
        })
        .inner
}

/// On/off switch, used instead of a checkbox in both windows.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let t = tokens(ui.ctx());
    let size = egui::vec2(2.0, 1.0) * ui.spacing().interact_size.y;
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool(response.id, *on);
        let radius = 0.5 * rect.height();
        let bg = if *on {
            t.accent
        } else if response.hovered() {
            t.hover
        } else {
            t.field
        };
        ui.painter().rect_filled(rect, radius, bg);
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0_f32, t.field_border),
            egui::StrokeKind::Inside,
        );
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        let knob = if *on { t.accent_text } else { t.text_dim };
        ui.painter()
            .circle_filled(egui::pos2(x, rect.center().y), 0.72 * radius, knob);
    }
    response
}

/// Seconds the pointer rests on a control before its tooltip shows.
pub const TOOLTIP_DELAY: f32 = 0.35;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_has_readable_contrast() {
        for kind in ThemeKind::ALL {
            let t = Tokens::for_kind(kind);
            let lum = |c: Color32| {
                0.2126 * f32::from(c.r()) + 0.7152 * f32::from(c.g()) + 0.0722 * f32::from(c.b())
            };
            let diff = (lum(t.text) - lum(t.chrome)).abs();
            assert!(diff > 80.0, "{} text on chrome: {diff}", kind.label());
            assert_eq!(t.kind, kind);
            assert_eq!(t.dark(), kind != ThemeKind::StudioLight);
        }
    }

    #[test]
    fn every_theme_has_a_visible_scrim() {
        for kind in ThemeKind::ALL {
            let a = Tokens::for_kind(kind).scrim.a();
            assert!((60..=200).contains(&a), "{}: alpha {a}", kind.label());
        }
    }

    #[test]
    fn pro_themes_are_flagged() {
        assert!(Tokens::for_kind(ThemeKind::Pro).pro);
        assert!(Tokens::for_kind(ThemeKind::ProMedium).pro);
        assert!(!Tokens::for_kind(ThemeKind::Studio).pro);
    }
}
