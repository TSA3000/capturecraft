//! The studio chrome's widgets, drawn from [`crate::icons`] and the theme:
//! the toolbar's icon-over-label buttons, the drawer's mode tiles, the
//! utility bar's pills and segmented tabs, and the Share split button.
//!
//! Every widget is placed at an explicit rect (the toolbar centres its
//! tool strip itself) and returns the egui [`Response`]; callers decide
//! what a click does.

use egui::{
    Align2, Color32, FontId, Id, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

use crate::icons::{self, Icon, IconStyle};
use crate::state::ToolId;
use crate::theme::Tokens;

/// Toolbar buttons: a 48 × 48 cell, a 20 px icon over an 11 px label.
pub const TOOL_BUTTON: Vec2 = Vec2::splat(48.0);
pub const TOOL_ICON: f32 = 20.0;
pub const TOOL_LABEL: f32 = 11.0;
/// Gap between toolbar buttons.
pub const TOOL_GAP: f32 = 2.0;
/// Drawer mode and preset tiles.
pub const TILE: Vec2 = Vec2::splat(48.0);

/// The icon of an editor tool.
pub fn tool_icon(t: ToolId) -> Icon {
    match t {
        ToolId::Arrow => Icon::Arrow,
        ToolId::Text => Icon::Text,
        ToolId::Callout => Icon::Callout,
        ToolId::Shape => Icon::Shape,
        ToolId::Stamp => Icon::Stamp,
        ToolId::Fill => Icon::Fill,
        ToolId::Move => Icon::Move,
        ToolId::Selection => Icon::Selection,
        ToolId::Step => Icon::Step,
        ToolId::Blur => Icon::Blur,
        ToolId::Highlighter => Icon::Highlight,
        ToolId::Pen => Icon::Pen,
        ToolId::Eraser => Icon::Eraser,
        ToolId::Magnify => Icon::Magnify,
        ToolId::Spotlight => Icon::Spotlight,
        ToolId::CutOut => Icon::CutOut,
        ToolId::SmartMove => Icon::SmartMove,
        ToolId::Simplify => Icon::Simplify,
        ToolId::Crop => Icon::Crop,
        ToolId::GrabText => Icon::GrabText,
    }
}

/// Icon colours for a control in this state.
pub fn icon_style(t: &Tokens, active: bool, enabled: bool) -> IconStyle {
    let fg = match (enabled, active) {
        (false, _) => t.text_faint,
        (true, true) => t.text,
        (true, false) => t.text_dim,
    };
    IconStyle {
        fg,
        accent: if enabled { t.record } else { t.text_faint },
    }
}

/// Background and border of a control: active > hovered > at rest.
fn paint_state(ui: &Ui, rect: Rect, resp: &Response, active: bool, t: &Tokens) {
    let p = ui.painter();
    let r = Tokens::corner(t.radius);
    if active {
        p.rect_filled(rect, r, t.tool_active_fill);
        p.rect_stroke(
            rect,
            r,
            Stroke::new(1.0, t.tool_active_border),
            StrokeKind::Inside,
        );
    } else if resp.hovered() && resp.enabled() {
        p.rect_filled(rect, r, t.card_hover);
    }
}

/// An icon-over-label toolbar button at `rect`. `chevron` adds the small
/// ▾ of a button that opens a menu.
pub struct ToolButton<'a> {
    pub id: Id,
    pub icon: Icon,
    pub label: &'a str,
    pub active: bool,
    pub enabled: bool,
    pub chevron: bool,
}

impl ToolButton<'_> {
    pub fn show(self, ui: &mut Ui, rect: Rect, t: &Tokens) -> Response {
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let resp = ui.interact(rect, self.id, sense);
        paint_state(ui, rect, &resp, self.active, t);
        let style = icon_style(t, self.active || resp.hovered(), self.enabled);
        let icon = Rect::from_center_size(
            pos2(rect.center().x, rect.min.y + 6.0 + TOOL_ICON / 2.0),
            Vec2::splat(TOOL_ICON),
        );
        icons::paint(ui.painter(), icon, self.icon, style);
        let label_y = rect.max.y - 5.0 - TOOL_LABEL / 2.0;
        let text_color = if self.enabled { style.fg } else { t.text_faint };
        let galley = ui.painter().layout_no_wrap(
            self.label.to_string(),
            FontId::proportional(TOOL_LABEL),
            text_color,
        );
        let mut w = galley.size().x;
        if self.chevron {
            w += 8.0;
        }
        let x0 = rect.center().x - w / 2.0;
        ui.painter().galley(
            pos2(x0, label_y - galley.size().y / 2.0),
            galley,
            text_color,
        );
        if self.chevron {
            let c = Rect::from_center_size(pos2(x0 + w - 3.0, label_y), Vec2::splat(7.0));
            icons::paint(ui.painter(), c, Icon::ChevronDown, style);
        }
        resp
    }
}

/// A rounded square tile of the drawer: an icon, a tooltip-sized label
/// under it when there is room.
pub fn tile(ui: &mut Ui, id: Id, icon: Icon, active: bool, enabled: bool, t: &Tokens) -> Response {
    let (rect, _) = ui.allocate_exact_size(TILE, Sense::hover());
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(rect, id, sense);
    let p = ui.painter();
    let r = Tokens::corner(t.radius);
    p.rect_filled(rect, r, if active { t.tool_active_fill } else { t.card });
    p.rect_stroke(
        rect,
        r,
        if active {
            Stroke::new(1.0, t.tool_active_border)
        } else if resp.hovered() && enabled {
            Stroke::new(1.0, t.text_faint)
        } else {
            Stroke::new(1.0, t.card_border)
        },
        StrokeKind::Inside,
    );
    let style = icon_style(t, active || resp.hovered(), enabled);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(26.0)),
        icon,
        style,
    );
    resp
}

/// A small pill of the utility bar: optional icon, text, optional chevron.
pub fn pill(
    ui: &mut Ui,
    id: Id,
    icon: Option<Icon>,
    text: &str,
    active: bool,
    chevron: bool,
    t: &Tokens,
) -> Response {
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, t.text);
    let icon_w = if icon.is_some() { 14.0 + 6.0 } else { 0.0 };
    let chev_w = if chevron { 4.0 + 8.0 } else { 0.0 };
    let size = vec2(20.0 + icon_w + galley.size().x + chev_w, 24.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    let p = ui.painter();
    let r = Tokens::corner(t.radius_pill);
    let fill = if active {
        t.tool_active_fill
    } else if resp.hovered() {
        t.card_hover
    } else {
        t.card
    };
    p.rect_filled(rect, r, fill);
    p.rect_stroke(
        rect,
        r,
        Stroke::new(
            1.0,
            if active {
                t.tool_active_border
            } else {
                t.chrome_border
            },
        ),
        StrokeKind::Inside,
    );
    let style = icon_style(t, true, true);
    let mut x = rect.min.x + 10.0;
    if let Some(i) = icon {
        let ir = Rect::from_center_size(pos2(x + 7.0, rect.center().y), Vec2::splat(14.0));
        icons::paint(p, ir, i, style);
        x += icon_w;
    }
    let gy = rect.center().y - galley.size().y / 2.0;
    let gw = galley.size().x;
    p.galley(pos2(x, gy), galley, t.text);
    if chevron {
        let c = Rect::from_center_size(pos2(x + gw + 4.0 + 4.0, rect.center().y), Vec2::splat(8.0));
        icons::paint(p, c, Icon::ChevronDown, style);
    }
    resp
}

/// A segmented pair of tabs; returns the clicked index.
pub fn segmented(
    ui: &mut Ui,
    id: Id,
    items: &[(Option<Icon>, &str)],
    selected: usize,
    t: &Tokens,
) -> Option<usize> {
    let font = FontId::proportional(12.0);
    let widths: Vec<f32> = items
        .iter()
        .map(|(icon, text)| {
            let g = ui
                .painter()
                .layout_no_wrap(text.to_string(), font.clone(), t.text);
            g.size().x + 20.0 + if icon.is_some() { 20.0 } else { 0.0 }
        })
        .collect();
    let total: f32 = widths.iter().sum();
    let (rect, _) = ui.allocate_exact_size(vec2(total, 24.0), Sense::hover());
    let p = ui.painter();
    let r = Tokens::corner(t.radius);
    p.rect_filled(rect, r, t.card);
    let mut clicked = None;
    let mut x = rect.min.x;
    for (i, ((icon, text), w)) in items.iter().zip(&widths).enumerate() {
        let cell = Rect::from_min_size(pos2(x, rect.min.y), vec2(*w, rect.height()));
        let resp = ui.interact(cell, id.with(i), Sense::click());
        let on = i == selected;
        if on {
            p.rect_filled(cell.shrink(1.0), r, t.tool_active_fill);
            p.rect_stroke(
                cell.shrink(1.0),
                r,
                Stroke::new(1.0, t.tool_active_border),
                StrokeKind::Inside,
            );
        } else if resp.hovered() {
            p.rect_filled(cell.shrink(1.0), r, t.card_hover);
        }
        let style = icon_style(t, on || resp.hovered(), true);
        let mut cx = cell.min.x + 10.0;
        if let Some(ic) = icon {
            icons::paint(
                p,
                Rect::from_center_size(pos2(cx + 7.0, cell.center().y), Vec2::splat(14.0)),
                *ic,
                style,
            );
            cx += 20.0;
        }
        p.text(
            pos2(cx, cell.center().y),
            Align2::LEFT_CENTER,
            *text,
            font.clone(),
            style.fg,
        );
        if resp.clicked() {
            clicked = Some(i);
        }
        x += w;
    }
    p.rect_stroke(
        rect,
        r,
        Stroke::new(1.0, t.chrome_border),
        StrokeKind::Inside,
    );
    clicked
}

/// What a click on a [`split_button`] hit.
#[derive(Clone, Debug)]
pub struct SplitResponse {
    pub main: Response,
    pub chevron: Response,
}

/// The primary Share / Save button at `rect`: an icon and label on the
/// left, a separate dropdown chevron on the right.
pub fn split_button(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    icon: Icon,
    label: &str,
    enabled: bool,
    t: &Tokens,
) -> SplitResponse {
    let chev_w = 22.0;
    let main_r = Rect::from_min_max(rect.min, pos2(rect.max.x - chev_w, rect.max.y));
    let chev_r = Rect::from_min_max(pos2(rect.max.x - chev_w, rect.min.y), rect.max);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let main = ui.interact(main_r, id.with("main"), sense);
    let chevron = ui.interact(chev_r, id.with("chevron"), sense);
    let p = ui.painter();
    let r = Tokens::corner(t.radius);
    let base = if enabled { t.share_fill } else { t.card };
    p.rect_filled(rect, r, base);
    for (part, resp) in [(main_r, &main), (chev_r, &chevron)] {
        if enabled && resp.hovered() {
            p.rect_filled(part, r, t.share_fill_hover);
        }
    }
    let text = if enabled { t.share_text } else { t.text_faint };
    p.line_segment(
        [
            pos2(chev_r.min.x, rect.min.y + 6.0),
            pos2(chev_r.min.x, rect.max.y - 6.0),
        ],
        Stroke::new(1.0, Color32::from_black_alpha(0x40)),
    );
    let style = IconStyle {
        fg: text,
        accent: text,
    };
    icons::paint(
        p,
        Rect::from_center_size(
            pos2(main_r.min.x + 18.0, rect.center().y),
            Vec2::splat(16.0),
        ),
        icon,
        style,
    );
    p.text(
        pos2(main_r.min.x + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        text,
    );
    icons::paint(
        p,
        Rect::from_center_size(chev_r.center(), Vec2::splat(9.0)),
        Icon::ChevronDown,
        style,
    );
    SplitResponse { main, chevron }
}

/// A 1 px vertical divider centred in `rect`.
pub fn vdivider(ui: &Ui, rect: Rect, t: &Tokens) {
    let x = rect.center().x.round() + 0.5;
    ui.painter().line_segment(
        [pos2(x, rect.min.y + 8.0), pos2(x, rect.max.y - 8.0)],
        Stroke::new(1.0, t.chrome_border),
    );
}

/// A full-width 1 px border line along the bottom (or top) of `rect`.
pub fn hborder(ui: &Ui, rect: Rect, bottom: bool, t: &Tokens) {
    let y = if bottom {
        rect.max.y - 0.5
    } else {
        rect.min.y + 0.5
    };
    ui.painter().line_segment(
        [pos2(rect.min.x, y), pos2(rect.max.x, y)],
        Stroke::new(1.0, t.chrome_border),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_distinct_icon() {
        let mut seen = Vec::new();
        for tool in ToolId::ALL {
            let i = tool_icon(tool);
            assert!(!seen.contains(&i), "{tool:?}");
            seen.push(i);
        }
    }

    #[test]
    fn disabled_controls_use_the_faint_colour() {
        let t = Tokens::default();
        assert_eq!(icon_style(&t, true, false).fg, t.text_faint);
        assert_eq!(icon_style(&t, false, true).accent, t.record);
    }
}
