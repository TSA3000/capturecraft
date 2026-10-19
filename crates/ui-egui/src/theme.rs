//! Design tokens. Panels read every colour, radius, spacing and elevation
//! from [`Tokens`]; nothing below `theme.rs` writes a hex value.
//!
//! Surfaces, text, accent and radii come from PhotoCraft's
//! [`photocraft_ui_egui::theme::Tokens`], so CaptureCraft offers the same
//! themes and [`Tokens::apply`] styles egui through PhotoCraft's
//! `theme::apply`. The tokens PhotoCraft has no use for (the orange-red
//! capture accent, gizmos, the crosshair overlay, the checkerboard,
//! spacing) are defined here. Selection and gizmo colours stay readable on
//! arbitrary screenshot pixels, so they do not follow the surface colours.
//!
//! Studio (the default) is the editor's dark studio look: a darker title
//! and menu strip, one panel colour for the toolbar, drawer and bars with
//! crisp 1 px borders, a deep surround that makes the capture stand out,
//! blue for the active tool and the primary Share button, and a crimson
//! record dot on Capture. The other themes derive the same roles from
//! PhotoCraft's tokens.

use egui::epaint::Shadow;
use egui::{Color32, CornerRadius, Stroke};
use photocraft_ui_egui::theme as pc;

/// The theme list is PhotoCraft's (Studio, Pro, Adwaita, Breeze, …).
pub use photocraft_ui_egui::theme::ThemeKind;

/// `SidebarState::theme` stores [`ThemeKind::id`]; unknown names fall back
/// to Studio (Dark).
pub fn kind_from_name(name: &str) -> ThemeKind {
    ThemeKind::from_name(name).unwrap_or(ThemeKind::Studio)
}

/// Colour, shape and spacing tokens for one theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub kind: ThemeKind,
    // Surfaces
    /// Title and menu strip above the toolbar.
    pub title_bar: Color32,
    /// Toolbar ribbon and window chrome.
    pub chrome: Color32,
    /// 1 px borders between the toolbar, drawer, bars and canvas.
    pub chrome_border: Color32,
    /// Sidebar, tray and library background.
    pub panel: Color32,
    /// Around the canvas.
    pub canvas_surround: Color32,
    pub card: Color32,
    /// Behind a drawer group's tabs (PhotoCraft's dock strip).
    pub tab_strip: Color32,
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
    /// Behind the active tool, tile or tab.
    pub tool_active_fill: Color32,
    /// Around the active tool, tile or tab.
    pub tool_active_border: Color32,
    /// The primary Share / Save split button.
    pub share_fill: Color32,
    pub share_fill_hover: Color32,
    pub share_text: Color32,
    /// The Capture icon's record dot.
    pub record: Color32,
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
    /// The rotation knob of a text gizmo.
    pub rotate_handle: Color32,
    /// Outline of a text gizmo's round handles.
    pub text_handle_stroke: Color32,
    /// Arrow bend handle.
    pub bend_handle: Color32,
    /// The eight canvas resize squares on the image border.
    pub canvas_handle_fill: Color32,
    pub canvas_handle_stroke: Color32,
    /// Edge of a canvas resize square, points.
    pub canvas_handle_size: f32,
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

/// WCAG contrast ratio of two opaque colours.
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

/// White or near-black text, whichever reads better on `fill`.
fn text_on(fill: Color32) -> Color32 {
    let (white, black) = (rgb(0xFFFFFF), rgb(0x111111));
    if contrast(white, fill) >= contrast(black, fill) {
        white
    } else {
        black
    }
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
    pub fn for_kind(kind: ThemeKind) -> Self {
        let b = pc::Tokens::for_kind(kind);
        let dark = b.dark();
        let t = Tokens {
            kind,
            title_bar: b.chrome,
            chrome: b.chrome,
            chrome_border: b.separator,
            panel: b.dock,
            canvas_surround: b.canvas,
            card: b.card,
            tab_strip: b.tab_strip,
            card_hover: b.hover,
            card_border: b.card_border,
            card_selected_border: b.accent_border,
            field: b.field,
            field_border: b.field_border,
            separator: b.separator,
            text: b.text,
            text_dim: b.text_dim,
            text_faint: b.text_faint,
            accent: b.accent,
            accent_soft: b.accent_soft,
            accent_text: b.accent_text,
            tool_active_fill: b.accent_soft,
            tool_active_border: b.accent,
            share_fill: b.accent,
            share_fill_hover: b.accent_border,
            share_text: text_on(b.accent),
            record: rgb(0xE11D48),
            accent_capture: if dark { rgb(0xF04E23) } else { rgb(0xC93A14) },
            accent_capture_hover: if dark { rgb(0xFF6A3D) } else { rgb(0xE0451C) },
            accent_capture_text: rgb(0xFFFFFF),
            danger: b.danger,
            dirty: b.warning,
            checker_light: if dark { rgb(0x3A3C40) } else { rgb(0xFFFFFF) },
            checker_dark: if dark { rgb(0x2C2E32) } else { rgb(0xE6E8EB) },
            checker_size: 8.0,
            canvas_edge: if dark {
                rgba(0xFFFFFF, 0x30)
            } else {
                rgba(0x000000, 0x30)
            },
            gizmo_stroke: rgb(0x3D8BFF),
            gizmo_outline: rgba(0x000000, 0xB0),
            handle_fill: rgb(0xFFFFFF),
            handle_stroke: rgb(0x3D8BFF),
            tail_handle: rgb(0xFFD21F),
            rotate_handle: rgb(0x00FF00),
            text_handle_stroke: rgb(0x000000),
            bend_handle: rgb(0x2ED1A2),
            canvas_handle_fill: if dark { rgb(0x4B5059) } else { rgb(0x6B7280) },
            canvas_handle_stroke: if dark { rgb(0x111317) } else { rgb(0xFFFFFF) },
            canvas_handle_size: 7.0,
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
            radius_sm: b.radius_sm,
            radius: b.radius,
            radius_lg: b.radius_lg,
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
        if kind == ThemeKind::Studio {
            Self::studio(t)
        } else {
            t
        }
    }

    /// The dark studio palette over PhotoCraft's Studio tokens.
    fn studio(t: Tokens) -> Tokens {
        let border = rgb(0x33373E);
        Tokens {
            title_bar: rgb(0x1B1D21),
            chrome: rgb(0x24272C),
            chrome_border: border,
            panel: rgb(0x24272C),
            canvas_surround: rgb(0x181A1E),
            card: rgb(0x2B2F35),
            tab_strip: rgb(0x1E2125),
            card_hover: rgb(0x32363D),
            card_border: border,
            card_selected_border: rgb(0x3B82F6),
            separator: border,
            accent: rgb(0x0078D4),
            accent_soft: rgb(0x1E3A5F),
            tool_active_fill: rgb(0x1E3A5F),
            tool_active_border: rgb(0x3B82F6),
            share_fill: rgb(0x1D70B8),
            share_fill_hover: rgb(0x2563EB),
            share_text: rgb(0xFFFFFF),
            ..t
        }
    }

    pub fn from_name(name: &str) -> Self {
        Self::for_kind(kind_from_name(name))
    }

    pub fn corner(r: f32) -> CornerRadius {
        CornerRadius::same(r.clamp(0.0, 255.0) as u8)
    }

    /// Styles egui with PhotoCraft's `theme::apply` (which also publishes
    /// PhotoCraft's tokens for its widgets) and stores these tokens for
    /// [`Tokens::get`]. The first call on a context also installs
    /// PhotoCraft's fonts: its text styles name font families (`semibold`)
    /// that egui cannot lay out without them.
    pub fn apply(&self, ctx: &egui::Context) {
        let fonts = egui::Id::new("capturecraft-photocraft-fonts");
        if !ctx.data(|d| d.get_temp::<bool>(fonts).unwrap_or(false)) {
            pc::install_fonts(ctx);
            ctx.data_mut(|d| d.insert_temp(fonts, true));
        }
        pc::apply(ctx, self.kind);
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(TOKENS_ID), *self));
    }

    /// Tokens of the theme last applied to `ctx` (Studio before any).
    pub fn get(ctx: &egui::Context) -> Self {
        ctx.data(|d| d.get_temp::<Tokens>(egui::Id::new(TOKENS_ID)))
            .unwrap_or_default()
    }
}

const TOKENS_ID: &str = "capturecraft-theme";

impl Default for Tokens {
    fn default() -> Self {
        Self::for_kind(ThemeKind::Studio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_readable_in_every_theme() {
        for t in ThemeKind::ALL.map(Tokens::for_kind) {
            assert!(contrast(t.text, t.panel) >= 4.5, "{:?}", t.kind);
            assert!(
                contrast(t.accent_capture_text, t.accent_capture) >= 3.0,
                "{:?}",
                t.kind
            );
        }
    }

    #[test]
    fn studio_uses_the_studio_palette() {
        let t = Tokens::for_kind(ThemeKind::Studio);
        assert_eq!(t.title_bar, rgb(0x1B1D21));
        assert_eq!(t.chrome, rgb(0x24272C));
        assert_eq!(t.chrome_border, rgb(0x33373E));
        assert_eq!(t.canvas_surround, rgb(0x181A1E));
        assert_eq!(t.share_fill, rgb(0x1D70B8));
        assert_eq!(t.record, rgb(0xE11D48));
        assert!(contrast(t.text, t.chrome) >= 4.5);
        assert!(contrast(t.text, t.title_bar) >= 4.5);
        for t in ThemeKind::ALL.map(Tokens::for_kind) {
            assert!(contrast(t.share_text, t.share_fill) >= 3.0, "{:?}", t.kind);
        }
    }

    #[test]
    fn names_round_trip() {
        for k in ThemeKind::ALL {
            assert_eq!(kind_from_name(k.id()), k);
        }
        assert_eq!(kind_from_name("nonsense"), ThemeKind::Studio);
    }
}
