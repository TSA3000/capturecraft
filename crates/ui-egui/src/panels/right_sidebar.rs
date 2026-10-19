//! Zone 2: Quick Styles above, the Property Inspector below.
//!
//! The grid shows the active tool's presets as live vector thumbnails
//! (two or three columns by sidebar width). Clicking a card selects it as
//! the style of the next annotation and, with annotations selected, applies
//! it to them (`annotation.setStyle`). Cut Out and Simplify have no style
//! presets; their cards choose the seam and the palette instead.
//!
//! The inspector edits a draft of the style (`SidebarState::draft`):
//! colours with an eyedropper, line width, arrowheads, the shadow direction
//! dial, corner radius, plus tool options (tail bend, step numbering, tear
//! roughness, Simplify palette). A change is applied to the selection once
//! the pointer is released, so a slider drag is one undo step; `+ Add Quick
//! Style` saves the draft as a new card.

use capturecraft_engine::DocState;
use capturecraft_format::{Axis, Color, DropShadow, Fill, HeadStyle, Numbering, Stroke};
use egui::{
    Align2, Color32, ComboBox, FontId, Id, Pos2, Rect as ERect, Sense, Shape, Slider, StrokeKind,
    Ui, Vec2, pos2, vec2,
};
use serde_json::{Value, json};

use crate::UiIntent;
use crate::state::{ColorTarget, EditorState, ExpandBackdrop, QuickStyle, StylePreset, ToolId};
use crate::theme::{Tokens, stroke};

const CARD_MIN_W: f32 = 76.0;
const CARD_H: f32 = 64.0;

/// Simplify palettes (`smart.simplify` names).
pub const PALETTES: [(&str, &str); 7] = [
    ("match", "Match capture"),
    ("auto", "Auto roles"),
    ("light", "Light"),
    ("dark", "Dark"),
    ("high_contrast", "High contrast"),
    ("blueprint", "Blueprint"),
    ("mono", "Mono"),
];

pub const HEADS: [(HeadStyle, &str); 7] = [
    (HeadStyle::None, "None"),
    (HeadStyle::Triangle, "Triangle"),
    (HeadStyle::OpenV, "Open"),
    (HeadStyle::Line, "Line"),
    (HeadStyle::Circle, "Circle"),
    (HeadStyle::Square, "Square"),
    (HeadStyle::Diamond, "Diamond"),
];

pub const NUMBERING: [(Numbering, &str); 5] = [
    (Numbering::Decimal, "1, 2, 3"),
    (Numbering::UpperAlpha, "A, B, C"),
    (Numbering::LowerAlpha, "a, b, c"),
    (Numbering::UpperRoman, "I, II, III"),
    (Numbering::LowerRoman, "i, ii, iii"),
];

/// Which inspector controls a tool has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fields {
    pub fill: bool,
    pub stroke: bool,
    pub text: bool,
    pub width: bool,
    pub heads: bool,
    pub shadow: bool,
    pub radius: bool,
}

impl Fields {
    pub fn of(tool: ToolId) -> Self {
        let f = Fields::default;
        match tool {
            ToolId::Arrow => Fields {
                stroke: true,
                width: true,
                heads: true,
                shadow: true,
                ..f()
            },
            ToolId::Callout => Fields {
                fill: true,
                stroke: true,
                text: true,
                width: true,
                shadow: true,
                radius: true,
                ..f()
            },
            ToolId::Shape => Fields {
                fill: true,
                stroke: true,
                width: true,
                shadow: true,
                radius: true,
                ..f()
            },
            ToolId::Step => Fields {
                fill: true,
                stroke: true,
                text: true,
                shadow: true,
                ..f()
            },
            ToolId::Text => Fields {
                text: true,
                shadow: true,
                ..f()
            },
            ToolId::Pen => Fields {
                stroke: true,
                width: true,
                ..f()
            },
            ToolId::Highlighter => Fields { fill: true, ..f() },
            ToolId::Magnify => Fields {
                stroke: true,
                width: true,
                ..f()
            },
            _ => f(),
        }
    }

    pub fn any(self) -> bool {
        self != Fields::default()
    }
}

/// 3 × 3 shadow direction: `(-1..=1, -1..=1)`, `(0, 0)` = straight below
/// the object (no offset).
pub fn shadow_direction(s: &DropShadow) -> (i8, i8) {
    let sign = |v: f32| {
        if v > 0.5 {
            1
        } else if v < -0.5 {
            -1
        } else {
            0
        }
    };
    (sign(s.offset_x), sign(s.offset_y))
}

pub fn shadow_distance(s: &DropShadow) -> f32 {
    s.offset_x.abs().max(s.offset_y.abs())
}

pub fn set_shadow_offset(s: &mut DropShadow, dir: (i8, i8), distance: f32) {
    s.offset_x = f32::from(dir.0) * distance;
    s.offset_y = f32::from(dir.1) * distance;
}

fn c32(c: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

fn from_c32(c: Color32) -> Color {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Color::rgba(r, g, b, a)
}

pub struct RightSidebar<'a> {
    pub doc: Option<&'a DocState>,
    pub tokens: &'a Tokens,
}

impl RightSidebar<'_> {
    pub fn show(self, ui: &mut Ui, editor: &mut EditorState, intents: &mut Vec<UiIntent>) {
        let t = self.tokens;
        let tool = editor.toolbar.active;
        let pending_id = Id::new("sidebar.style_pending");
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(t.space_md);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Quick Styles").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(tool.label()).color(t.text_dim));
                    });
                });
                ui.add_space(t.space_sm);
                let changed = self.quick_styles(ui, editor, intents);
                ui.add_space(t.space_md);
                ui.separator();
                let edited = self.inspector(ui, editor, intents);
                if edited || changed {
                    ui.ctx().data_mut(|d| d.insert_temp(pending_id, edited));
                }
            });
        // Commit inspector edits to the selection on release.
        let pending = ui
            .ctx()
            .data(|d| d.get_temp::<bool>(pending_id))
            .unwrap_or(false);
        let released = !ui.input(|i| i.pointer.any_down());
        if pending && released {
            ui.ctx().data_mut(|d| d.insert_temp(pending_id, false));
            if let Some(i) = apply_to_selection(editor, tool) {
                intents.push(i);
            }
        }
    }

    /// The grid; true when a card was clicked.
    fn quick_styles(
        &self,
        ui: &mut Ui,
        editor: &mut EditorState,
        intents: &mut Vec<UiIntent>,
    ) -> bool {
        let t = self.tokens;
        let tool = editor.toolbar.active;
        let width = ui.available_width();
        let cols = if width >= 3.0 * CARD_MIN_W + 2.0 * t.space_md {
            3
        } else {
            2
        };
        let card_w = (width - (cols - 1) as f32 * t.space_md) / cols as f32;
        let mut clicked = false;
        let mut grid =
            |ui: &mut Ui, n: usize, card: &mut dyn FnMut(&mut Ui, usize, ERect) -> bool| {
                egui::Grid::new(("sidebar.quick_styles", tool))
                    .spacing(Vec2::splat(t.space_md))
                    .show(ui, |ui| {
                        for i in 0..n {
                            let (rect, _) =
                                ui.allocate_exact_size(vec2(card_w, CARD_H), Sense::hover());
                            if card(ui, i, rect) {
                                clicked = true;
                            }
                            if (i + 1) % cols == 0 {
                                ui.end_row();
                            }
                        }
                    });
            };
        match tool {
            ToolId::CutOut => {
                let torn = editor.tools.cut_out.torn;
                grid(ui, 2, &mut |ui, i, rect| {
                    let r = card_frame(ui, rect, (i == 1) == torn, t, ("cut", i));
                    paint_seam(ui.painter(), r.rect.shrink(t.space_md), i == 1, t);
                    if r.clicked() {
                        editor.tools.cut_out.torn = i == 1;
                    }
                    r.on_hover_text(if i == 1 {
                        "Torn edge with shadow"
                    } else {
                        "Straight seam"
                    })
                    .clicked()
                });
            }
            ToolId::Simplify => {
                let current = editor.tools.simplify.palette.clone();
                let has_state = self.doc.and_then(|d| d.simplify_state()).is_some();
                grid(ui, PALETTES.len(), &mut |ui, i, rect| {
                    let Some((name, label)) = PALETTES.get(i) else {
                        return false;
                    };
                    let r = card_frame(ui, rect, current == *name, t, ("palette", i));
                    ui.painter().text(
                        r.rect.center(),
                        Align2::CENTER_CENTER,
                        *label,
                        FontId::proportional(12.0),
                        t.text,
                    );
                    if r.clicked() {
                        editor.tools.simplify.palette = (*name).into();
                        let id = if has_state {
                            "smart.setPalette"
                        } else {
                            "smart.simplify"
                        };
                        intents.push(UiIntent::command(id, json!({ "palette": name })));
                    }
                    r.clicked()
                });
            }
            _ => {
                let Some(ts) = editor.sidebar.quick_styles.for_tool(tool).cloned() else {
                    ui.label(
                        egui::RichText::new("This tool has no Quick Styles.").color(t.text_faint),
                    );
                    return false;
                };
                let visible: Vec<(usize, QuickStyle)> = ts
                    .styles
                    .iter()
                    .cloned()
                    .enumerate()
                    .filter(|(_, s)| !s.hidden)
                    .collect();
                let draft_active = editor
                    .sidebar
                    .draft
                    .as_ref()
                    .is_some_and(|d| d.tool == tool);
                let mut remove = None;
                grid(ui, visible.len(), &mut |ui, i, rect| {
                    let Some((index, style)) = visible.get(i) else {
                        return false;
                    };
                    let selected = ts.selected == Some(*index) && !draft_active;
                    let r = card_frame(ui, rect, selected, t, ("style", &style.id));
                    paint_thumbnail(
                        ui.painter(),
                        r.rect.shrink(t.space_md),
                        tool,
                        &style.preset,
                        t,
                    );
                    let r = r.on_hover_text(&style.name);
                    r.context_menu(|ui| {
                        let label = if style.builtin { "Hide" } else { "Delete" };
                        if ui.button(label).clicked() {
                            remove = Some((*index, style.builtin));
                            ui.close();
                        }
                    });
                    if r.clicked() {
                        if let Some(ts) = editor
                            .sidebar
                            .quick_styles
                            .tools
                            .iter_mut()
                            .find(|x| x.tool == tool)
                        {
                            ts.selected = Some(*index);
                        }
                        editor.sidebar.draft = None;
                        if let Some(i) = apply_to_selection(editor, tool) {
                            intents.push(i);
                        }
                    }
                    r.clicked()
                });
                if let Some((index, builtin)) = remove
                    && let Some(ts) = editor
                        .sidebar
                        .quick_styles
                        .tools
                        .iter_mut()
                        .find(|x| x.tool == tool)
                {
                    if builtin {
                        if let Some(s) = ts.styles.get_mut(index) {
                            s.hidden = true;
                        }
                    } else if index < ts.styles.len() {
                        ts.styles.remove(index);
                    }
                    ts.selected = ts
                        .selected
                        .filter(|s| *s != index)
                        .map(|s| if s > index && !builtin { s - 1 } else { s });
                }
                if editor.sidebar.draft_is_new(tool) {
                    ui.add_space(t.space_sm);
                    let add = egui::Button::new(
                        egui::RichText::new("+ Add Quick Style").color(t.accent_text),
                    )
                    .fill(t.accent)
                    .corner_radius(Tokens::corner(t.radius_pill));
                    if ui.add(add).clicked() {
                        let n = editor
                            .sidebar
                            .quick_styles
                            .for_tool(tool)
                            .map_or(0, |ts| ts.styles.iter().filter(|s| !s.builtin).count());
                        editor.sidebar.save_draft(&format!("Custom {}", n + 1));
                    }
                }
            }
        }
        clicked
    }

    /// The Property Inspector; true when a style value changed.
    fn inspector(
        &self,
        ui: &mut Ui,
        editor: &mut EditorState,
        intents: &mut Vec<UiIntent>,
    ) -> bool {
        let t = self.tokens;
        let tool = editor.toolbar.active;
        let fields = Fields::of(tool);
        let mut style = editor.sidebar.effective_style(tool).unwrap_or_default();
        let before = style.clone();
        let mut eyedropper = None;

        if fields.any() {
            section(ui, "Style", |ui| {
                if fields.fill {
                    color_row(ui, "Fill", &mut style, ColorTarget::Fill, &mut eyedropper);
                }
                if fields.stroke {
                    color_row(
                        ui,
                        "Outline",
                        &mut style,
                        ColorTarget::Stroke,
                        &mut eyedropper,
                    );
                }
                if fields.text {
                    color_row(ui, "Text", &mut style, ColorTarget::Text, &mut eyedropper);
                }
                ui.add(Slider::new(&mut style.opacity, 0.0..=1.0).text("Opacity"));
            });
        }
        if fields.width || fields.heads {
            section(ui, "Line", |ui| {
                if fields.width {
                    let s = style.stroke.get_or_insert_with(Stroke::default);
                    ui.add(
                        Slider::new(&mut s.width, 0.5..=32.0)
                            .text("Width")
                            .suffix(" px"),
                    );
                }
                if fields.heads {
                    head_combo(ui, "Start", &mut style.arrow_start, HeadStyle::None);
                    head_combo(ui, "End", &mut style.arrow_end, HeadStyle::Triangle);
                }
            });
        }
        if fields.shadow {
            section(ui, "Shadow", |ui| self.shadow_dial(ui, &mut style));
        }
        if fields.radius {
            section(ui, "Shape", |ui| {
                let mut r = style.corner_radius.unwrap_or(0.0);
                if ui
                    .add(Slider::new(&mut r, 0.0..=64.0).text("Corner radius"))
                    .changed()
                {
                    style.corner_radius = Some(r);
                }
            });
        }

        // Tool options: not style, never applied to the selection.
        let o = &mut editor.tools;
        match tool {
            ToolId::Callout => section(ui, "Tail", |ui| {
                ui.add(Slider::new(&mut o.callout.tail_curve, -1.0..=1.0).text("Bend"));
                ui.add(Slider::new(&mut o.callout.tail_width, 4.0..=96.0).text("Base width"));
            }),
            ToolId::Step => section(ui, "Numbering", |ui| {
                ComboBox::from_id_salt("sidebar.numbering")
                    .selected_text(
                        NUMBERING
                            .iter()
                            .find(|(n, _)| *n == o.step.numbering)
                            .map_or("", |(_, l)| *l),
                    )
                    .show_ui(ui, |ui| {
                        for (n, label) in NUMBERING {
                            ui.selectable_value(&mut o.step.numbering, n, label);
                        }
                    });
                ui.horizontal(|ui| {
                    ui.label("Sequence");
                    ui.text_edit_singleline(&mut o.step.sequence);
                });
                if ui.button("Renumber").clicked() {
                    intents.push(UiIntent::command(
                        "annotation.renumberSteps",
                        json!({ "sequence": o.step.sequence }),
                    ));
                }
            }),
            ToolId::CutOut => section(ui, "Cut Out", |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut o.cut_out.axis, Axis::Horizontal, "Horizontal");
                    ui.selectable_value(&mut o.cut_out.axis, Axis::Vertical, "Vertical");
                });
                ui.checkbox(&mut o.cut_out.torn, "Torn edge");
                ui.add_enabled_ui(o.cut_out.torn, |ui| {
                    ui.add(Slider::new(&mut o.cut_out.roughness, 0.0..=1.0).text("Roughness"));
                    ui.add(
                        Slider::new(&mut o.cut_out.amplitude_px, 1.0..=48.0)
                            .text("Depth")
                            .suffix(" px"),
                    );
                });
            }),
            ToolId::Simplify => section(ui, "Simplify", |ui| {
                if let Some(p) = self
                    .doc
                    .and_then(|d| d.simplify_state())
                    .and_then(|s| s.palette.as_ref())
                {
                    ui.horizontal(|ui| {
                        for (c, name) in [
                            (p.background, "Background"),
                            (p.surface, "Surface"),
                            (p.accent, "Accent"),
                            (p.muted_text, "Text"),
                            (p.border, "Border"),
                            (p.on_accent, "On accent"),
                        ] {
                            let (r, resp) =
                                ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                            ui.painter().rect(
                                r,
                                Tokens::corner(t.radius_sm),
                                c32(c),
                                stroke(1.0, t.card_border),
                                StrokeKind::Inside,
                            );
                            resp.on_hover_text(name);
                        }
                    });
                }
                if ui.button("Simplify capture").clicked() {
                    intents.push(UiIntent::command(
                        "smart.simplify",
                        json!({ "palette": o.simplify.palette }),
                    ));
                }
            }),
            _ => {}
        }

        section(ui, "Canvas", |ui| {
            let c = &mut editor.canvas;
            ComboBox::from_label("New margin")
                .selected_text(c.expand_backdrop.label())
                .show_ui(ui, |ui| {
                    for b in ExpandBackdrop::ALL {
                        ui.selectable_value(&mut c.expand_backdrop, b, b.label());
                    }
                });
            ui.checkbox(&mut c.checkerboard, "Transparency checkerboard");
        });

        if let Some(target) = eyedropper {
            editor.canvas.eyedropper = Some(target);
        }
        let edited = style != before;
        if edited {
            editor.sidebar.edit_draft(tool, |p| *p = style);
        }
        edited
    }

    fn shadow_dial(&self, ui: &mut Ui, style: &mut StylePreset) {
        let t = self.tokens;
        let mut on = style.shadow.is_some();
        ui.checkbox(&mut on, "Drop shadow");
        if !on {
            style.shadow = None;
            return;
        }
        let s = style.shadow.get_or_insert_with(DropShadow::default);
        let dir = shadow_direction(s);
        let mut distance = shadow_distance(s).max(if dir == (0, 0) { 0.0 } else { 1.0 });
        ui.horizontal(|ui| {
            let mut new_dir = dir;
            egui::Grid::new("sidebar.shadow_dial")
                .spacing(Vec2::splat(t.space_xs))
                .show(ui, |ui| {
                    for dy in -1i8..=1 {
                        for dx in -1i8..=1 {
                            let (r, resp) =
                                ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                            let active = (dx, dy) == dir;
                            let p = ui.painter();
                            p.rect(
                                r,
                                Tokens::corner(t.radius_sm),
                                if active { t.accent } else { t.field },
                                stroke(
                                    1.0,
                                    if resp.hovered() {
                                        t.accent
                                    } else {
                                        t.field_border
                                    },
                                ),
                                StrokeKind::Inside,
                            );
                            if (dx, dy) != (0, 0) {
                                let v = vec2(f32::from(dx), f32::from(dy)).normalized() * 5.0;
                                p.arrow(
                                    r.center() - v,
                                    v * 1.6,
                                    stroke(1.2, if active { t.accent_text } else { t.text_dim }),
                                );
                            } else {
                                p.circle_filled(
                                    r.center(),
                                    2.5,
                                    if active { t.accent_text } else { t.text_dim },
                                );
                            }
                            if resp.on_hover_text(direction_name(dx, dy)).clicked() {
                                new_dir = (dx, dy);
                            }
                        }
                        ui.end_row();
                    }
                });
            ui.vertical(|ui| {
                ui.add_enabled(
                    new_dir != (0, 0),
                    Slider::new(&mut distance, 0.0..=32.0).text("Distance"),
                );
                ui.add(Slider::new(&mut s.blur, 0.0..=64.0).text("Blur"));
                let mut alpha = f32::from(s.color.a) / 255.0;
                if ui
                    .add(Slider::new(&mut alpha, 0.0..=1.0).text("Opacity"))
                    .changed()
                {
                    s.color.a = (alpha * 255.0).round() as u8;
                }
            });
            let d = if new_dir == (0, 0) {
                0.0
            } else {
                distance.max(1.0)
            };
            set_shadow_offset(s, new_dir, d);
        });
    }
}

fn direction_name(dx: i8, dy: i8) -> &'static str {
    match (dx, dy) {
        (-1, -1) => "Up left",
        (0, -1) => "Up",
        (1, -1) => "Up right",
        (-1, 0) => "Left",
        (0, 0) => "Centered",
        (1, 0) => "Right",
        (-1, 1) => "Down left",
        (0, 1) => "Down",
        _ => "Down right",
    }
}

/// `annotation.setStyle` with the tool's current style, for the selection.
pub fn apply_to_selection(editor: &EditorState, tool: ToolId) -> Option<UiIntent> {
    if editor.canvas.selection.is_empty() {
        return None;
    }
    let style = editor.sidebar.effective_style(tool)?;
    let mut p = style.style_params();
    let ids: Vec<u64> = editor.canvas.selection.iter().map(|n| n.0).collect();
    p.insert("ids".into(), json!(ids));
    Some(UiIntent::command("annotation.setStyle", Value::Object(p)))
}

fn section(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(title)
        .default_open(true)
        .show(ui, body);
}

fn color_row(
    ui: &mut Ui,
    label: &str,
    style: &mut StylePreset,
    target: ColorTarget,
    eyedropper: &mut Option<ColorTarget>,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("💧")
                .on_hover_text("Pick a colour from the capture")
                .clicked()
            {
                *eyedropper = Some(target);
            }
            let current = match target {
                ColorTarget::Fill => style.fill_color(),
                ColorTarget::Stroke => style.stroke.as_ref().map(|s| s.color),
                ColorTarget::Text => style.text_color,
            };
            let mut c = current.map_or(Color32::TRANSPARENT, c32);
            if egui::color_picker::color_edit_button_srgba(
                ui,
                &mut c,
                egui::color_picker::Alpha::OnlyBlend,
            )
            .changed()
            {
                let color = from_c32(c);
                match target {
                    ColorTarget::Fill => style.fill = Fill::Solid { color },
                    ColorTarget::Stroke => {
                        style.stroke.get_or_insert_with(Stroke::default).color = color
                    }
                    ColorTarget::Text => style.text_color = Some(color),
                }
            }
        });
    });
}

fn head_combo(ui: &mut Ui, label: &str, head: &mut Option<HeadStyle>, default: HeadStyle) {
    let mut h = head.unwrap_or(default);
    ComboBox::from_label(label)
        .selected_text(HEADS.iter().find(|(s, _)| *s == h).map_or("", |(_, l)| *l))
        .show_ui(ui, |ui| {
            for (s, l) in HEADS {
                ui.selectable_value(&mut h, s, l);
            }
        });
    if Some(h) != *head && (head.is_some() || h != default) {
        *head = Some(h);
    }
}

/// A card: elevation, fill and the selected ring; returns its response.
fn card_frame(
    ui: &mut Ui,
    rect: ERect,
    selected: bool,
    t: &Tokens,
    salt: impl std::hash::Hash + std::fmt::Debug,
) -> egui::Response {
    let resp = ui.interact(rect, Id::new(("sidebar.card", salt)), Sense::click());
    let hot = resp.hovered();
    let corner = Tokens::corner(t.radius);
    let p = ui.painter();
    let elevation = if hot {
        t.card_elevation_hover
    } else {
        t.card_elevation
    };
    p.add(elevation.as_shape(rect, corner));
    p.rect(
        rect,
        corner,
        if hot { t.card_hover } else { t.card },
        if selected {
            stroke(2.0, t.card_selected_border)
        } else {
            stroke(1.0, t.card_border)
        },
        StrokeKind::Inside,
    );
    resp
}

/// Vector preview of `preset` as `tool` would draw it.
pub fn paint_thumbnail(
    p: &egui::Painter,
    r: ERect,
    tool: ToolId,
    preset: &StylePreset,
    t: &Tokens,
) {
    let a = preset.opacity.clamp(0.0, 1.0);
    let col = |c: Color| c32(c).gamma_multiply(a);
    let line = preset
        .stroke
        .as_ref()
        .map(|s| col(s.color))
        .unwrap_or(t.text_dim);
    let width = preset
        .stroke
        .as_ref()
        .map_or(2.0, |s| (s.width * 0.5).clamp(1.0, 5.0));
    let fill = preset.fill_color().map(col);
    let text = preset.text_color.map(col).unwrap_or(t.text);
    let shadow = preset.shadow.map(|s| {
        (
            vec2(s.offset_x, s.offset_y).clamp(Vec2::splat(-3.0), Vec2::splat(3.0)),
            c32(s.color),
        )
    });
    let corner = preset.corner_radius.map_or(0.0, |r| (r * 0.5).min(12.0));
    match tool {
        ToolId::Arrow => {
            let (a0, a1) = (
                r.left_bottom() + vec2(6.0, -8.0),
                r.right_top() + vec2(-8.0, 8.0),
            );
            if let Some((o, c)) = shadow {
                p.arrow(a0 + o, a1 - a0, stroke(width, c));
            }
            p.arrow(a0, a1 - a0, stroke(width, line));
            if preset.arrow_start.is_some_and(|h| h != HeadStyle::None) {
                p.circle_filled(a0, width + 1.0, line);
            }
        }
        ToolId::Callout => {
            let body = ERect::from_min_max(r.min, pos2(r.max.x, r.max.y - 12.0));
            let tail = vec![
                pos2(body.min.x + body.width() * 0.25, body.max.y - 1.0),
                pos2(body.min.x + body.width() * 0.45, body.max.y - 1.0),
                pos2(body.min.x + body.width() * 0.18, r.max.y),
            ];
            if let Some((o, c)) = shadow {
                p.rect_filled(body.translate(o), Tokens::corner(corner), c);
            }
            let f = fill.unwrap_or(Color32::TRANSPARENT);
            p.add(Shape::convex_polygon(tail, f, stroke(width, line)));
            p.rect(
                body,
                Tokens::corner(corner),
                f,
                stroke(width, line),
                StrokeKind::Inside,
            );
            p.text(
                body.center(),
                Align2::CENTER_CENTER,
                "Aa",
                FontId::proportional(13.0),
                text,
            );
        }
        ToolId::Step => {
            let c = r.center();
            let rad = r.height().min(r.width()) * 0.38;
            if let Some((o, sc)) = shadow {
                p.circle_filled(c + o, rad, sc);
            }
            p.circle(
                c,
                rad,
                fill.unwrap_or(t.accent),
                stroke(width.min(2.0), line),
            );
            p.text(
                c,
                Align2::CENTER_CENTER,
                "1",
                FontId::proportional(rad * 1.1),
                text,
            );
        }
        ToolId::Text => {
            if let Some((o, c)) = shadow {
                p.text(
                    r.center() + o,
                    Align2::CENTER_CENTER,
                    "Aa",
                    FontId::proportional(24.0),
                    c,
                );
            }
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                "Aa",
                FontId::proportional(24.0),
                text,
            );
        }
        ToolId::Shape | ToolId::Magnify => {
            let b = r.shrink(4.0);
            if let Some((o, c)) = shadow {
                p.rect_filled(b.translate(o), Tokens::corner(corner), c);
            }
            if tool == ToolId::Magnify {
                let rad = b.height() / 2.0;
                p.circle(b.center(), rad, t.card_hover, stroke(width, line));
                p.text(
                    b.center(),
                    Align2::CENTER_CENTER,
                    "+",
                    FontId::proportional(rad),
                    t.text_dim,
                );
            } else {
                p.rect(
                    b,
                    Tokens::corner(corner),
                    fill.unwrap_or(Color32::TRANSPARENT),
                    stroke(width, line),
                    StrokeKind::Inside,
                );
            }
        }
        ToolId::Pen => {
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let u = i as f32 / 16.0;
                    pos2(
                        r.min.x + u * r.width(),
                        r.center().y + (u * std::f32::consts::TAU).sin() * r.height() * 0.3,
                    )
                })
                .collect();
            p.add(Shape::line(pts, stroke(width, line)));
        }
        ToolId::Highlighter => {
            let band = ERect::from_center_size(r.center(), vec2(r.width(), r.height() * 0.4));
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                "text",
                FontId::proportional(14.0),
                t.text_dim,
            );
            p.rect_filled(band, 0.0, fill.unwrap_or(line));
        }
        ToolId::Blur => {
            let n = 6;
            let cell = r.width() / n as f32;
            for i in 0..n {
                for j in 0..3 {
                    let shade = if (i + j) % 2 == 0 {
                        t.card_hover
                    } else {
                        t.field_border
                    };
                    let c = ERect::from_min_size(
                        pos2(
                            r.min.x + i as f32 * cell,
                            r.center().y + (j as f32 - 1.5) * cell,
                        ),
                        Vec2::splat(cell),
                    );
                    p.rect_filled(c, 0.0, shade);
                }
            }
            p.rect_stroke(r, 0.0, stroke(width, line), StrokeKind::Inside);
        }
        _ => {
            p.rect_stroke(
                r,
                Tokens::corner(corner),
                stroke(width, line),
                StrokeKind::Inside,
            );
        }
    }
}

/// Cut Out card: two image halves joined straight or along a torn edge.
fn paint_seam(p: &egui::Painter, r: ERect, torn: bool, t: &Tokens) {
    let mid = r.center().y;
    let pts: Vec<Pos2> = (0..=12)
        .map(|i| {
            let x = r.min.x + r.width() * i as f32 / 12.0;
            let dy = if torn {
                if i % 2 == 0 { -3.0 } else { 3.0 }
            } else {
                0.0
            };
            pos2(x, mid + dy)
        })
        .collect();
    let gap = if torn { 4.0 } else { 2.0 };
    for (k, sign) in [(0, -1.0f32), (1, 1.0)] {
        let edge: Vec<Pos2> = pts.iter().map(|q| *q + vec2(0.0, sign * gap)).collect();
        let mut poly = edge.clone();
        if k == 0 {
            poly.push(r.right_top());
            poly.push(r.left_top());
        } else {
            poly.push(r.right_bottom());
            poly.push(r.left_bottom());
        }
        p.add(Shape::Path(egui::epaint::PathShape::closed_line(
            poly,
            stroke(1.0, t.card_border),
        )));
        let band = if k == 0 {
            ERect::from_min_max(r.min, pos2(r.max.x, mid - gap - 3.0))
        } else {
            ERect::from_min_max(pos2(r.min.x, mid + gap + 3.0), r.max)
        };
        p.rect_filled(band, 0.0, t.card_hover);
        if torn && k == 0 {
            p.add(Shape::line(
                edge.iter().map(|q| *q + vec2(1.5, 2.0)).collect(),
                stroke(2.0, Color32::from_black_alpha(90)),
            ));
        }
        p.add(Shape::line(edge, stroke(1.5, t.cutout_seam)));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use capturecraft_format::NodeId;

    #[test]
    fn shadow_dial_round_trips_every_direction() {
        let mut s = DropShadow::default();
        for dx in -1..=1 {
            for dy in -1..=1 {
                set_shadow_offset(&mut s, (dx, dy), 6.0);
                assert_eq!(shadow_direction(&s), (dx, dy));
                if (dx, dy) != (0, 0) {
                    assert_eq!(shadow_distance(&s), 6.0);
                }
            }
        }
    }

    #[test]
    fn every_quick_style_tool_has_inspector_fields() {
        for tool in ToolId::ALL.into_iter().filter(|t| t.has_quick_styles()) {
            if tool != ToolId::Blur {
                assert!(Fields::of(tool).any(), "{tool:?}");
            }
        }
    }

    #[test]
    fn styles_apply_only_to_a_selection() {
        let mut e = EditorState::default();
        e.toolbar.active = ToolId::Callout;
        assert!(apply_to_selection(&e, ToolId::Callout).is_none());
        e.canvas.selection = vec![NodeId(4), NodeId(9)];
        let Some(UiIntent::Command { id, params }) = apply_to_selection(&e, ToolId::Callout) else {
            unreachable!()
        };
        assert_eq!(id, "annotation.setStyle");
        assert_eq!(params["ids"], json!([4, 9]));
        assert!(params.get("corner_radius").is_some());
    }

    /// The preset parameters are accepted by `annotation.setStyle` for
    /// every built-in Quick Style.
    #[test]
    fn built_in_styles_apply_in_the_engine() {
        use capturecraft_engine::{Pixels, Session};
        use capturecraft_format::Manifest;
        use photocraft_geom::Size;
        let mut s = Session::new();
        let size = Size::new(100, 100);
        s.open(
            Manifest::new(size),
            Pixels::new(size, vec![0; 100 * 100 * 4]),
        )
        .unwrap();
        let id = s
            .run("annotation.addStep", &json!({ "at": { "x": 50, "y": 50 } }))
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let mut e = EditorState::default();
        e.canvas.selection = vec![NodeId(id)];
        for ts in &e.sidebar.quick_styles.tools.clone() {
            for i in 0..ts.styles.len() {
                let mut e = e.clone();
                if let Some(x) = e
                    .sidebar
                    .quick_styles
                    .tools
                    .iter_mut()
                    .find(|x| x.tool == ts.tool)
                {
                    x.selected = Some(i);
                }
                let intent = apply_to_selection(&e, ts.tool).unwrap();
                let out = crate::dispatch(&mut s, vec![intent]);
                assert!(
                    matches!(out[0], crate::Dispatched::Ran { .. }),
                    "{:?} {i}: {out:?}",
                    ts.tool
                );
            }
        }
    }
}
