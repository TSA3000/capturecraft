//! Design tokens. Panels read every colour, radius, spacing and elevation
//! from [`Tokens`]; nothing below `theme.rs` writes a hex value.
//!
//! Two themes: **Studio Dark**, the high-contrast near-black editor with the
//! orange-red capture accent, and **Light**, the same system on light
//! surfaces. Selection and gizmo colours stay readable on arbitrary
//! screenshot pixels in both, so they do not follow the surface colours.

use egui::epaint::Shadow;
use egui::{Color32, CornerRadius, Stroke, Visuals};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeKind {
    #[default]
    StudioDark,
    Light,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 2] = [ThemeKind::StudioDark, ThemeKind::Light];

    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::StudioDark => "Studio (Dark)",
            ThemeKind::Light => "Light",
        }
    }

    /// The name `SidebarState::theme` stores; unknown names fall back to dark.
    pub fn from_name(name: &str) -> Self {
        match name {
            "light" | "studio_light" => ThemeKind::Light,
            _ => ThemeKind::StudioDark,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ThemeKind::StudioDark => "dark",
            ThemeKind::Light => "light",
        }
    }

    pub fn dark(self) -> bool {
        self == ThemeKind::StudioDark
    }
}

/// Colour, shape and spacing tokens for one theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub kind: ThemeKind,
    // Surfaces
    /// Toolbar ribbon and window chrome.
    pub chrome: Color32,
    /// Sidebar, tray and library background.
    pub panel: Color32,
    /// Around the canvas.
    pub canvas_surround: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub card_border: Color32,
    pub card_selected_border: Color32,
    pub field: Color32,
    pub field_border: Color32,
    pub separator: Color32,
    // Text
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    // Accents
    /// Selected tool, focused field, toggles.
    pub accent: Color32,
    pub accent_soft: Color32,
    pub accent_text: Color32,
    /// The Capture button and the capture HUD's brand colour.
    pub accent_capture: Color32,
    pub accent_capture_hover: Color32,
    pub accent_capture_text: Color32,
    pub danger: Color32,
    /// Unsaved-changes dot in the Recent Tray.
    pub dirty: Color32,
    // Canvas
    pub checker_light: Color32,
    pub checker_dark: Color32,
    /// Edge of one checkerboard square, in points.
    pub checker_size: f32,
    /// Outline drawn around the canvas bounds.
    pub canvas_edge: Color32,
    // Gizmos (on top of document pixels)
    pub gizmo_stroke: Color32,
    pub gizmo_outline: Color32,
    pub handle_fill: Color32,
    pub handle_stroke: Color32,
    /// Callout tail diamonds.
    pub tail_handle: Color32,
    /// Arrow bend handle.
    pub bend_handle: Color32,
    pub expand_handle_fill: Color32,
    pub expand_handle_stroke: Color32,
    pub expand_preview: Color32,
    /// Strip a Cut Out would remove.
    pub cutout_band: Color32,
    pub cutout_seam: Color32,
    // Crosshair overlay
    /// Box around the hovered window, control or monitor.
    pub hover_target: Color32,
    /// Dims everything outside the target or selection.
    pub scrim: Color32,
    pub crosshair: Color32,
    pub badge_bg: Color32,
    pub badge_text: Color32,
    pub loupe_border: Color32,
    pub loupe_grid: Color32,
    pub loupe_reticle: Color32,
    // Shape
    pub radius_sm: f32,
    pub radius: f32,
    pub radius_lg: f32,
    /// Pills: capture button, toggles, badges.
    pub radius_pill: f32,
    pub space_xs: f32,
    pub space_sm: f32,
    pub space_md: f32,
    pub space_lg: f32,
    /// Selection handle edge, points.
    pub handle_size: f32,
    /// Long side / short side of a canvas expansion pill, points.
    pub expand_handle_len: f32,
    pub expand_handle_thickness: f32,
    /// Gap between the canvas edge and its expansion handles.
    pub expand_handle_gap: f32,
    /// Quick Style cards at rest and hovered.
    pub card_elevation: Shadow,
    pub card_elevation_hover: Shadow,
    /// Floating bars and popovers (overlay action bar, loupe).
    pub popup_elevation: Shadow,
}

/// A stroke of `width` points (typed, so literals need no suffix).
pub fn stroke(width: f32, color: Color32) -> Stroke {
    Stroke::new(width, color)
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn rgba(hex: u32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, a)
}

const fn shadow(offset: [i8; 2], blur: u8, alpha: u8) -> Shadow {
    Shadow {
        offset,
        blur,
        spread: 0,
        color: Color32::from_black_alpha(alpha),
    }
}

impl Tokens {
    pub const STUDIO_DARK: Tokens = Tokens {
        kind: ThemeKind::StudioDark,
        chrome: rgb(0x161719),
        panel: rgb(0x1E1F22),
        canvas_surround: rgb(0x0F1012),
        card: rgb(0x2A2C30),
        card_hover: rgb(0x34373C),
        card_border: rgb(0x3A3D42),
        card_selected_border: rgb(0xFF6A2B),
        field: rgb(0x26282C),
        field_border: rgb(0x404349),
        separator: rgb(0x2E3034),
        text: rgb(0xF2F3F5),
        text_dim: rgb(0xB4B8BF),
        text_faint: rgb(0x7A7F87),
        accent: rgb(0x3D8BFF),
        accent_soft: rgba(0x3D8BFF, 0x33),
        accent_text: rgb(0xFFFFFF),
        accent_capture: rgb(0xF04E23),
        accent_capture_hover: rgb(0xFF6A3D),
        accent_capture_text: rgb(0xFFFFFF),
        danger: rgb(0xE53E30),
        dirty: rgb(0xFFA62B),
        checker_light: rgb(0x3A3C40),
        checker_dark: rgb(0x2C2E32),
        checker_size: 8.0,
        canvas_edge: rgba(0xFFFFFF, 0x30),
        gizmo_stroke: rgb(0x3D8BFF),
        gizmo_outline: rgba(0x000000, 0xB0),
        handle_fill: rgb(0xFFFFFF),
        handle_stroke: rgb(0x3D8BFF),
        tail_handle: rgb(0xFFD21F),
        bend_handle: rgb(0x2ED1A2),
        expand_handle_fill: rgb(0xF2F3F5),
        expand_handle_stroke: rgb(0x161719),
        expand_preview: rgb(0xFF6A2B),
        cutout_band: rgba(0xF04E23, 0x40),
        cutout_seam: rgb(0xFF6A2B),
        hover_target: rgb(0xFFB31A),
        scrim: rgba(0x000000, 0x80),
        crosshair: rgba(0xFFFFFF, 0xC0),
        badge_bg: rgba(0x101113, 0xE6),
        badge_text: rgb(0xFFFFFF),
        loupe_border: rgb(0xFFFFFF),
        loupe_grid: rgba(0x000000, 0x50),
        loupe_reticle: rgb(0xFFB31A),
        radius_sm: 4.0,
        radius: 8.0,
        radius_lg: 12.0,
        radius_pill: 999.0,
        space_xs: 2.0,
        space_sm: 4.0,
        space_md: 8.0,
        space_lg: 16.0,
        handle_size: 8.0,
        expand_handle_len: 28.0,
        expand_handle_thickness: 8.0,
        expand_handle_gap: 10.0,
        card_elevation: shadow([0, 1], 4, 0x60),
        card_elevation_hover: shadow([0, 3], 10, 0x80),
        popup_elevation: shadow([0, 6], 18, 0x90),
    };

    pub const LIGHT: Tokens = Tokens {
        kind: ThemeKind::Light,
        chrome: rgb(0xF4F5F7),
        panel: rgb(0xFFFFFF),
        canvas_surround: rgb(0xE3E5E9),
        card: rgb(0xFFFFFF),
        card_hover: rgb(0xF1F3F6),
        card_border: rgb(0xD9DCE1),
        card_selected_border: rgb(0xE0451C),
        field: rgb(0xFFFFFF),
        field_border: rgb(0xC9CDD3),
        separator: rgb(0xE2E4E8),
        text: rgb(0x17181B),
        text_dim: rgb(0x4B5057),
        text_faint: rgb(0x8A8F97),
        accent: rgb(0x1F6FEB),
        accent_soft: rgba(0x1F6FEB, 0x26),
        accent_text: rgb(0xFFFFFF),
        accent_capture: rgb(0xE0451C),
        accent_capture_hover: rgb(0xF05A30),
        accent_capture_text: rgb(0xFFFFFF),
        danger: rgb(0xD03225),
        dirty: rgb(0xE88A00),
        checker_light: rgb(0xFFFFFF),
        checker_dark: rgb(0xE6E8EB),
        ..Tokens::STUDIO_DARK
    };

    pub fn for_kind(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::StudioDark => Self::STUDIO_DARK,
            ThemeKind::Light => Self::LIGHT,
        }
    }

    pub fn from_name(name: &str) -> Self {
        Self::for_kind(ThemeKind::from_name(name))
    }

    pub fn corner(r: f32) -> CornerRadius {
        CornerRadius::same(r.clamp(0.0, 255.0) as u8)
    }

    /// egui widget visuals built from the tokens.
    pub fn visuals(&self) -> Visuals {
        let mut v = if self.kind.dark() {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        v.panel_fill = self.panel;
        v.window_fill = self.panel;
        v.extreme_bg_color = self.field;
        v.faint_bg_color = self.card;
        v.override_text_color = Some(self.text);
        v.selection.bg_fill = self.accent_soft;
        v.selection.stroke = stroke(1.0, self.accent);
        v.hyperlink_color = self.accent;
        v.window_shadow = self.popup_elevation;
        v.popup_shadow = self.popup_elevation;
        v.window_corner_radius = Self::corner(self.radius_lg);
        v.menu_corner_radius = Self::corner(self.radius);
        v.window_stroke = stroke(1.0, self.card_border);
        let w = &mut v.widgets;
        for (state, fill) in [
            (&mut w.inactive, self.field),
            (&mut w.hovered, self.card_hover),
            (&mut w.active, self.card_hover),
            (&mut w.open, self.card_hover),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.corner_radius = Self::corner(self.radius_sm);
        }
        w.inactive.bg_stroke = stroke(1.0, self.field_border);
        w.hovered.bg_stroke = stroke(1.0, self.accent);
        w.active.bg_stroke = stroke(1.0, self.accent);
        w.noninteractive.bg_fill = self.panel;
        w.noninteractive.bg_stroke = stroke(1.0, self.separator);
        v
    }

    pub fn apply(&self, ctx: &egui::Context) {
        ctx.set_visuals(self.visuals());
        ctx.style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(self.space_md, self.space_sm + self.space_xs);
            s.spacing.button_padding = egui::vec2(self.space_md, self.space_sm);
        });
    }
}

impl Default for Tokens {
    fn default() -> Self {
        Self::STUDIO_DARK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance contrast of two opaque colours.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            let ch = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_is_readable_in_both_themes() {
        for t in ThemeKind::ALL.map(Tokens::for_kind) {
            assert!(contrast(t.text, t.panel) >= 7.0, "{:?}", t.kind);
            assert!(contrast(t.text_dim, t.panel) >= 4.5, "{:?}", t.kind);
            assert!(
                contrast(t.accent_capture_text, t.accent_capture) >= 3.0,
                "{:?}",
                t.kind
            );
        }
    }

    #[test]
    fn names_round_trip() {
        for k in ThemeKind::ALL {
            assert_eq!(ThemeKind::from_name(k.name()), k);
        }
        assert_eq!(ThemeKind::from_name("nonsense"), ThemeKind::StudioDark);
    }
}
