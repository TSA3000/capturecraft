//! Zone 2: the right drawer, "Tool Properties" or "Effects" (the utility
//! bar's tabs pick which).
//!
//! Tool Properties: the Selection tool shows its mode tiles, Snap to
//! Object and the Background Fill toggle (Transparent clears the hole a
//! moved or deleted selection leaves, Auto Fill inpaints it with the UI
//! analyzer). Every other tool shows Quick Styles above, the Property
//! Inspector below.
//!
//! Effects: tiles for the document's canvas effects (border, drop shadow,
//! rounded shadow frame; the rest are marked as not available yet), the
//! frame's settings and the canvas options.
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
//!
//! The Frame section edits the document's rounded shadow frame (margin,
//! corners, a solid, linear or radial backdrop, shadow and border) and
//! commits `canvas.setBackdrop` on release, the same way.

use capturecraft_engine::DocState;
use capturecraft_format::{
    Axis, CanvasBackdrop, Color, DropShadow, Fill, GradientStop, HeadStyle, Numbering, Stroke,
};
use egui::{
    Align2, Color32, ComboBox, FontId, Id, Pos2, Rect as ERect, Sense, Shape, Slider, StrokeKind,
    Ui, Vec2, pos2, vec2,
};
use photocraft_geom::{Point, Size};
use serde_json::{Value, json};

use crate::UiIntent;
use crate::icons::{self, Icon};
use crate::panels::layers_panel::LayersPanel;
use crate::state::{
    BlurOptions, ColorTarget, EditorState, ExpandBackdrop, InspectorMode, MagnifyOptions,
    PixelateOptions, QuickStyle, SelectionFill, SelectionMode, ShapeKind, StylePreset, ToolId,
};
use crate::theme::{Tokens, stroke};
use crate::widgets::{self, icon_style, tile};

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
        let mode = editor.sidebar.mode;
        // Size sliders and combos to the drawer: PhotoCraft's defaults
        // (150 / 120 px) make a labelled row wider than it, and egui then
        // pushes the whole right panel past the window edge.
        let width = ui.available_width();
        let spacing = ui.spacing_mut();
        spacing.slider_width = (width - 130.0).clamp(60.0, 150.0);
        spacing.combo_width = (width - 120.0).clamp(80.0, 120.0);
        ui.set_max_width(width);
        let (title, help) = match mode {
            InspectorMode::Effects => (
                "Effects",
                "Effects change the whole canvas: a border, a drop shadow or a rounded shadow frame. Every change is one undo step.",
            ),
            InspectorMode::Properties => ("Tool Properties", tool_help(tool)),
            InspectorMode::Layers => (
                "Layers",
                "Every annotation, topmost first. The eye hides it, the padlock locks it; drag a row to restack, double-click to rename.",
            ),
            InspectorMode::History => (
                "History",
                "Every edit of this capture. Click a row to go back (or forward) to that point.",
            ),
        };
        header(ui, title, help, t);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match mode {
                InspectorMode::Layers => {
                    ui.add_space(t.space_sm);
                    LayersPanel {
                        doc: self.doc,
                        tokens: t,
                    }
                    .layers(ui, editor, intents);
                }
                InspectorMode::History => {
                    ui.add_space(t.space_sm);
                    LayersPanel {
                        doc: self.doc,
                        tokens: t,
                    }
                    .history(ui, intents);
                }
                InspectorMode::Effects => {
                    ui.add_space(t.space_sm);
                    self.effects(ui, intents);
                    self.frame_section(ui, intents);
                    canvas_section(ui, editor);
                }
                InspectorMode::Properties if tool == ToolId::Selection => {
                    ui.add_space(t.space_sm);
                    self.selection(ui, editor, intents);
                }
                InspectorMode::Properties => {
                    ui.add_space(t.space_sm);
                    if tool.command().is_none() {
                        ui.label(
                            egui::RichText::new(format!(
                                "Drawing with {} on the canvas is not available yet; its styles are kept for when it is.",
                                tool.label()
                            ))
                            .color(t.text_faint)
                            .small(),
                        );
                        ui.add_space(t.space_sm);
                    }
                    if self.tool_options(ui, editor, intents) {
                        return;
                    }
                    let heading = match tool {
                        ToolId::Blur | ToolId::Pixelate => "Strength",
                        _ => "Quick Styles",
                    };
                    ui.label(egui::RichText::new(heading).strong());
                    ui.add_space(t.space_sm);
                    let changed = self.quick_styles(ui, editor, intents);
                    ui.add_space(t.space_md);
                    ui.separator();
                    let edited = self.inspector(ui, editor, intents);
                    if edited || changed {
                        ui.ctx().data_mut(|d| d.insert_temp(pending_id, edited));
                    }
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

    /// Options above the Quick Styles; true for tools that have nothing
    /// else (Eraser, Spotlight, Crop).
    fn tool_options(
        &self,
        ui: &mut Ui,
        editor: &mut EditorState,
        intents: &mut Vec<UiIntent>,
    ) -> bool {
        let t = self.tokens;
        let tool = editor.toolbar.active;
        match tool {
            ToolId::Shape => {
                let o = &mut editor.tools.shape;
                let kinds = [ShapeKind::Rectangle, ShapeKind::Ellipse, ShapeKind::Polygon];
                let current = kinds.iter().position(|k| *k == o.kind).unwrap_or(0);
                if let Some(i) = widgets::segmented(
                    ui,
                    Id::new("sidebar.shape_kind"),
                    &[(None, "Rectangle"), (None, "Ellipse"), (None, "Polygon")],
                    current,
                    t,
                ) {
                    o.kind = kinds.get(i).copied().unwrap_or_default();
                    editor.canvas.polygon.clear();
                }
                if o.kind == ShapeKind::Polygon {
                    ui.label(
                        egui::RichText::new(
                            "Click each corner; double-click or Enter closes it, Esc cancels.",
                        )
                        .color(t.text_faint)
                        .small(),
                    );
                }
                ui.add_space(t.space_md);
                false
            }
            ToolId::Magnify => {
                let o = &mut editor.tools.magnify;
                ui.label(egui::RichText::new("Zoom").strong());
                let zooms = MagnifyOptions::ZOOMS;
                let labels: Vec<String> = zooms.iter().map(|z| format!("{z}×")).collect();
                let items: Vec<(Option<Icon>, &str)> =
                    labels.iter().map(|l| (None, l.as_str())).collect();
                let current = zooms.iter().position(|z| *z == o.zoom).unwrap_or(1);
                if let Some(i) =
                    widgets::segmented(ui, Id::new("sidebar.magnify_zoom"), &items, current, t)
                {
                    o.zoom = zooms.get(i).copied().unwrap_or(2.0);
                }
                ui.add(Slider::new(&mut o.radius, 20.0..=300.0).text("Lens radius"));
                ui.add_space(t.space_md);
                false
            }
            ToolId::Eraser => {
                let o = &mut editor.tools.eraser;
                section(ui, "Brush", |ui| {
                    ui.add(
                        Slider::new(&mut o.radius, 1.0..=200.0)
                            .text("Size")
                            .suffix(" px"),
                    );
                    ui.add(Slider::new(&mut o.hardness, 0.0..=1.0).text("Hardness"));
                });
                ui.label(
                    egui::RichText::new("Drag over the capture to erase it to transparency. Annotations are not affected.")
                        .color(t.text_faint)
                        .small(),
                );
                true
            }
            ToolId::Spotlight => {
                let o = &mut editor.tools.spotlight;
                let current = usize::from(o.ellipse);
                if let Some(i) = widgets::segmented(
                    ui,
                    Id::new("sidebar.spotlight_kind"),
                    &[(None, "Rectangle"), (None, "Ellipse")],
                    current,
                    t,
                ) {
                    o.ellipse = i == 1;
                }
                ui.add(Slider::new(&mut o.dim_opacity, 0.1..=0.95).text("Dim"));
                true
            }
            ToolId::Crop => {
                let doc = self.doc;
                let frame = editor.canvas.crop;
                if let (Some(r), Some(d)) = (frame, doc) {
                    ui.label(format!("{} × {} px", r.w.round(), r.h.round()));
                    ui.add_space(t.space_sm);
                    ui.horizontal(|ui| {
                        if ui.button("Apply").clicked()
                            && let Some(i) = super::canvas_tools::crop_intent(&d.manifest, r)
                        {
                            intents.push(i);
                            editor.canvas.crop = None;
                        }
                        if ui.button("Reset").clicked() {
                            editor.canvas.crop = None;
                        }
                    });
                }
                ui.label(
                    egui::RichText::new("Drag the frame's handles or draw a new frame; Enter or double-click applies, Esc resets. Alt resizes from the centre.")
                        .color(t.text_faint)
                        .small(),
                );
                true
            }
            _ => false,
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
            ToolId::Blur => {
                let current = editor.tools.blur.sigma;
                grid(ui, BlurOptions::PRESETS.len(), &mut |ui, i, rect| {
                    let Some((label, sigma)) = BlurOptions::PRESETS.get(i).copied() else {
                        return false;
                    };
                    let r = card_frame(ui, rect, current == sigma, t, ("blur", i));
                    paint_strength(ui.painter(), r.rect.shrink(t.space_md), None, i, label, t);
                    if r.clicked() {
                        editor.tools.blur.sigma = sigma;
                    }
                    r.on_hover_text(format!("Gaussian blur, radius {sigma} px"))
                        .clicked()
                });
            }
            ToolId::Pixelate => {
                let current = editor.tools.pixelate.block_size_px;
                grid(ui, PixelateOptions::PRESETS.len(), &mut |ui, i, rect| {
                    let Some((label, block)) = PixelateOptions::PRESETS.get(i).copied() else {
                        return false;
                    };
                    let r = card_frame(ui, rect, current == block, t, ("pixelate", i));
                    paint_strength(
                        ui.painter(),
                        r.rect.shrink(t.space_md),
                        Some(block),
                        i,
                        label,
                        t,
                    );
                    if r.clicked() {
                        editor.tools.pixelate.block_size_px = block;
                    }
                    r.on_hover_text(format!("{block} px blocks")).clicked()
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
                let mut favorite = None;
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
                    if style.favorite {
                        let star =
                            ERect::from_min_size(r.rect.min + vec2(4.0, 4.0), Vec2::splat(12.0));
                        icons::paint(
                            ui.painter(),
                            star,
                            Icon::Favorites,
                            icon_style(t, true, true),
                        );
                    }
                    r.context_menu(|ui| {
                        let fav = if style.favorite {
                            "Remove from Favorites"
                        } else {
                            "Add to Favorites"
                        };
                        if ui.button(fav).clicked() {
                            favorite = Some(*index);
                            ui.close();
                        }
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
                if let Some(index) = favorite {
                    editor.sidebar.quick_styles.toggle_favorite(tool, index);
                }
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

        if let Some(target) = eyedropper {
            editor.canvas.eyedropper = Some(target);
        }
        let edited = style != before;
        if edited {
            editor.sidebar.edit_draft(tool, |p| *p = style);
        }
        edited
    }

    /// The Selection tool: mode tiles, Snap to Object, Background Fill and
    /// the marquee's actions.
    fn selection(&self, ui: &mut Ui, editor: &mut EditorState, intents: &mut Vec<UiIntent>) {
        let t = self.tokens;
        let o = &mut editor.tools.selection;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = t.space_md;
            for m in SelectionMode::ALL {
                let icon = match m {
                    SelectionMode::Rectangle => Icon::RectangleSelect,
                    SelectionMode::Ellipse => Icon::EllipseSelect,
                    SelectionMode::Polygon => Icon::PolygonSelect,
                    SelectionMode::Lasso => Icon::LassoSelect,
                };
                let r = tile(
                    ui,
                    Id::new(("sidebar.select_mode", m.label())),
                    icon,
                    o.mode == m,
                    m.available(),
                    t,
                );
                let r = if m.available() {
                    r.on_hover_text(m.label())
                } else {
                    r.on_hover_text(format!("{}: not available yet", m.label()))
                };
                if r.clicked() {
                    o.mode = m;
                }
            }
        });
        ui.add_space(t.space_md);
        ui.add_enabled(
            false,
            egui::Checkbox::new(&mut o.snap_to_object, "Snap to Object"),
        )
        .on_disabled_hover_text("Snapping to detected UI elements is not available yet");
        ui.add_space(t.space_md);
        ui.label(egui::RichText::new("Background Fill").strong());
        ui.add_space(t.space_xs);
        let fills = [
            (None, SelectionFill::Transparent.label()),
            (None, SelectionFill::AutoFill.label()),
        ];
        let current = usize::from(o.background == SelectionFill::AutoFill);
        if let Some(i) = widgets::segmented(ui, Id::new("sidebar.select_fill"), &fills, current, t)
        {
            o.background = if i == 0 {
                SelectionFill::Transparent
            } else {
                SelectionFill::AutoFill
            };
        }
        ui.label(
            egui::RichText::new(match o.background {
                SelectionFill::Transparent => {
                    "Moving or deleting the selection leaves a transparent hole."
                }
                SelectionFill::AutoFill => {
                    "Moving or deleting the selection fills the hole from its surroundings."
                }
            })
            .color(t.text_faint)
            .small(),
        );
        ui.add_space(t.space_md);
        ui.separator();
        let marquee = editor.canvas.marquee;
        let base = marquee
            .zip(self.doc)
            .and_then(|(r, d)| capturecraft_engine::commands::canvas_rect_to_base(&d.manifest, r));
        match base {
            Some(b) => {
                ui.label(format!("Selected {} × {} px", b.width(), b.height()));
                ui.horizontal(|ui| {
                    if ui.button("Delete").on_hover_text("Delete key").clicked()
                        && let Some(d) = self.doc
                        && let Some(i) = crate::panels::canvas_viewport::delete_selection_intent(
                            editor,
                            &d.manifest,
                        )
                    {
                        intents.push(i);
                        editor.canvas.marquee = None;
                    }
                    if ui.button("Deselect").on_hover_text("Esc").clicked() {
                        editor.canvas.marquee = None;
                    }
                });
            }
            None => {
                ui.label(
                    egui::RichText::new(
                        "Drag on the capture to select a region, then drag inside it to move it.",
                    )
                    .color(t.text_dim),
                );
            }
        }
    }

    /// The effect tiles; a click sets the canvas backdrop.
    fn effects(&self, ui: &mut Ui, intents: &mut Vec<UiIntent>) {
        let t = self.tokens;
        let current = self.doc.map(|d| &d.manifest.canvas.backdrop);
        let active = current.map(Effect::of);
        let enabled = self.doc.is_some();
        let w = ui.available_width();
        let cols = ((w + t.space_md) / (widgets::TILE.x + t.space_md))
            .floor()
            .max(1.0) as usize;
        egui::Grid::new("sidebar.effects")
            .spacing(Vec2::splat(t.space_md))
            .show(ui, |ui| {
                for (i, e) in Effect::ALL.into_iter().enumerate() {
                    ui.vertical(|ui| {
                        let on = active == Some(e);
                        let available = e.backdrop().is_some() || e == Effect::None;
                        let r = tile(
                            ui,
                            Id::new(("sidebar.effect", i)),
                            e.icon(),
                            on,
                            enabled && available,
                            t,
                        );
                        let r = if available {
                            r.on_hover_text(e.label())
                        } else {
                            r.on_hover_text(format!("{}: not available yet", e.label()))
                        };
                        ui.label(egui::RichText::new(e.short()).size(10.0).color(t.text_dim));
                        if r.clicked() && !on {
                            let b = e.backdrop().unwrap_or(CanvasBackdrop::None);
                            intents.push(crate::panels::gizmo::backdrop_intent(&b));
                        }
                    });
                    if (i + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
        ui.add_space(t.space_md);
    }

    /// The rounded shadow frame; commits on release.
    fn frame_section(&self, ui: &mut Ui, intents: &mut Vec<UiIntent>) {
        let Some(doc) = self.doc else {
            return;
        };
        let current = &doc.manifest.canvas.backdrop;
        let canvas = doc.manifest.canvas.size;
        let id = Id::new("sidebar.frame_draft");
        let mut draft = ui
            .ctx()
            .data(|d| d.get_temp::<CanvasBackdrop>(id))
            .unwrap_or_else(|| current.clone());
        let before = draft.clone();
        section(ui, "Frame", |ui| {
            let mut on = !matches!(draft, CanvasBackdrop::None);
            if ui.checkbox(&mut on, "Rounded shadow frame").changed() {
                draft = if on {
                    CanvasBackdrop::rounded_shadow_frame()
                } else {
                    CanvasBackdrop::None
                };
            }
            let size = draft.frame_size(canvas);
            let CanvasBackdrop::RoundedShadowFrame {
                margin_px,
                corner_radius_px,
                backdrop_fill,
                shadow,
                border,
            } = &mut draft
            else {
                return;
            };
            ui.add(
                Slider::new(margin_px, 0.0..=256.0)
                    .text("Margin")
                    .suffix(" px"),
            );
            ui.add(
                Slider::new(corner_radius_px, 0.0..=64.0)
                    .text("Corners")
                    .suffix(" px"),
            );
            let (mut kind, mut a, mut b) = fill_parts(backdrop_fill);
            let parts = (kind, a, b);
            ui.horizontal(|ui| {
                ui.label("Backdrop");
                for k in BackdropKind::ALL {
                    ui.selectable_value(&mut kind, k, k.label());
                }
            });
            ui.horizontal(|ui| {
                ui.label("Colours");
                for (c, show) in [(&mut a, true), (&mut b, kind != BackdropKind::Solid)] {
                    if show {
                        let mut v = c32(*c);
                        if egui::color_picker::color_edit_button_srgba(
                            ui,
                            &mut v,
                            egui::color_picker::Alpha::OnlyBlend,
                        )
                        .changed()
                        {
                            *c = from_c32(v);
                        }
                    }
                }
            });
            if (kind, a, b) != parts || kind != BackdropKind::Solid {
                let fresh = backdrop_fill_of(kind, a, b, size);
                // Gradients follow the frame size; keep points set elsewhere
                // unless something here changed.
                if (kind, a, b) != parts || margin_changed(&before, *margin_px) {
                    *backdrop_fill = fresh;
                }
            }
            let mut has_shadow = shadow.is_some();
            if ui.checkbox(&mut has_shadow, "Shadow").changed() {
                *shadow = if has_shadow {
                    match CanvasBackdrop::rounded_shadow_frame() {
                        CanvasBackdrop::RoundedShadowFrame { shadow, .. } => shadow,
                        CanvasBackdrop::None => Some(DropShadow::default()),
                    }
                } else {
                    None
                };
            }
            let mut has_border = border.is_some();
            if ui.checkbox(&mut has_border, "Border").changed() {
                *border = has_border.then(|| Stroke {
                    color: Color::rgba(0, 0, 0, 0x33),
                    width: 1.0,
                    ..Stroke::default()
                });
            }
        });
        let pending = draft != before
            || ui
                .ctx()
                .data(|d| d.get_temp::<CanvasBackdrop>(id))
                .is_some();
        if draft != before {
            ui.ctx().data_mut(|d| d.insert_temp(id, draft.clone()));
        }
        if pending && !ui.input(|i| i.pointer.any_down()) {
            ui.ctx().data_mut(|d| d.remove::<CanvasBackdrop>(id));
            if draft != *current {
                intents.push(crate::panels::gizmo::backdrop_intent(&draft));
            }
        }
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
                fit_sliders(ui);
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

/// How the frame's backdrop is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackdropKind {
    Solid,
    Linear,
    Radial,
}

impl BackdropKind {
    pub const ALL: [BackdropKind; 3] = [
        BackdropKind::Solid,
        BackdropKind::Linear,
        BackdropKind::Radial,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BackdropKind::Solid => "Solid",
            BackdropKind::Linear => "Linear",
            BackdropKind::Radial => "Radial",
        }
    }
}

/// Kind and first/last colours of a backdrop fill.
pub fn fill_parts(f: &Fill) -> (BackdropKind, Color, Color) {
    let ends = |stops: &[GradientStop]| {
        (
            stops.first().map_or(Color::WHITE, |s| s.color),
            stops.last().map_or(Color::WHITE, |s| s.color),
        )
    };
    match f {
        Fill::None => (BackdropKind::Solid, Color::WHITE, Color::WHITE),
        Fill::Solid { color } => (BackdropKind::Solid, *color, *color),
        Fill::Linear { stops, .. } => {
            let (a, b) = ends(stops);
            (BackdropKind::Linear, a, b)
        }
        Fill::Radial { stops, .. } => {
            let (a, b) = ends(stops);
            (BackdropKind::Radial, a, b)
        }
    }
}

/// A backdrop fill over a `frame`-sized image (frame space): a diagonal
/// linear gradient corner to corner, or a radial one from the centre to
/// the farthest corner.
pub fn backdrop_fill_of(kind: BackdropKind, a: Color, b: Color, frame: Size) -> Fill {
    let (w, h) = (f64::from(frame.width), f64::from(frame.height));
    let stops = vec![
        GradientStop {
            offset: 0.0,
            color: a,
        },
        GradientStop {
            offset: 1.0,
            color: b,
        },
    ];
    match kind {
        BackdropKind::Solid => Fill::Solid { color: a },
        BackdropKind::Linear => Fill::Linear {
            from: Point::new(0.0, 0.0),
            to: Point::new(w, h),
            stops,
        },
        BackdropKind::Radial => Fill::Radial {
            center: Point::new(w / 2.0, h / 2.0),
            radius: (w.hypot(h) / 2.0) as f32,
            stops,
        },
    }
}

fn margin_changed(before: &CanvasBackdrop, now: f32) -> bool {
    match before {
        CanvasBackdrop::RoundedShadowFrame { margin_px, .. } => *margin_px != now,
        CanvasBackdrop::None => true,
    }
}

fn section(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(title)
        .default_open(true)
        .show(ui, |ui| {
            fit_sliders(ui);
            body(ui);
        });
}

/// Slider width that leaves room for the value box and a short label in
/// what is left of the row.
fn fit_sliders(ui: &mut Ui) {
    let w = ui.available_width();
    ui.spacing_mut().slider_width = (w - 150.0).clamp(40.0, 150.0);
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

/// The drawer's title row with the round `?` help icon.
fn header(ui: &mut Ui, title: &str, help: &str, t: &Tokens) {
    ui.add_space(t.space_md);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong().size(14.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
            icons::paint(
                ui.painter(),
                r,
                Icon::Help,
                icon_style(t, resp.hovered(), true),
            );
            resp.on_hover_text(help);
        });
    });
    ui.add_space(t.space_sm);
    let r = ui.available_rect_before_wrap();
    ui.painter().line_segment(
        [pos2(r.min.x, r.min.y), pos2(r.max.x, r.min.y)],
        stroke(1.0, t.chrome_border),
    );
    ui.add_space(t.space_sm);
}

/// What the `?` says about a tool.
pub fn tool_help(tool: ToolId) -> &'static str {
    match tool {
        ToolId::Selection => {
            "Drag to select part of the capture, drag inside the selection to move it, Delete to remove it."
        }
        ToolId::Arrow => "Drag from the tail to the tip. Drag the green handle to bend it.",
        ToolId::Callout => {
            "Click or drag to place a callout; drag the yellow diamond to aim its tail."
        }
        ToolId::Step => "Click to drop the next number of the sequence.",
        ToolId::Move => {
            "Click to select annotations, drag to move them; handles resize and rotate."
        }
        ToolId::CutOut => "Drag across the capture to remove a strip and close the gap.",
        ToolId::Simplify => "Replace text and detail with clean shapes in the chosen palette.",
        ToolId::Shape => "Drag a rectangle or ellipse, or click the corners of a polygon.",
        ToolId::Highlighter => "Drag over text to mark it with a translucent highlighter band.",
        ToolId::Blur => "Drag over private details to blur them; pick the strength below.",
        ToolId::Pixelate => {
            "Drag over private details to pixelate them; pick the block size below."
        }
        ToolId::Text => {
            "Click to type, or drag a box; Enter finishes (Shift+Enter for a new line). Double-click a text to edit it."
        }
        ToolId::Pen => "Drag to draw a smooth freehand line in the selected style.",
        ToolId::Eraser => "Drag over the capture to erase pixels.",
        ToolId::Magnify => {
            "Click to drop a magnifier, or drag out its size; move its source ring afterwards."
        }
        ToolId::Spotlight => "Drag around what matters; everything else is dimmed.",
        ToolId::Crop => "Adjust the frame, then press Enter to crop.",
        _ => "Pick a Quick Style, then adjust it below. + Add Quick Style saves it.",
    }
}

/// New-margin fill and the transparency grid.
fn canvas_section(ui: &mut Ui, editor: &mut EditorState) {
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
}

/// A canvas effect tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    Border,
    DropShadow,
    RoundedFrame,
    TornEdge,
    Perspective,
    Spotlight,
    ColorFilter,
}

impl Effect {
    pub const ALL: [Effect; 8] = [
        Effect::None,
        Effect::Border,
        Effect::DropShadow,
        Effect::RoundedFrame,
        Effect::TornEdge,
        Effect::Perspective,
        Effect::Spotlight,
        Effect::ColorFilter,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Effect::None => "No effect",
            Effect::Border => "Border",
            Effect::DropShadow => "Drop Shadow",
            Effect::RoundedFrame => "Rounded Drop-Shadow Frame",
            Effect::TornEdge => "Torn Edge",
            Effect::Perspective => "Perspective / Shear",
            Effect::Spotlight => "Spotlight",
            Effect::ColorFilter => "Color Filter",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Effect::None => "None",
            Effect::RoundedFrame => "Frame",
            Effect::DropShadow => "Shadow",
            Effect::Perspective => "Perspective",
            Effect::ColorFilter => "Filter",
            e => e.label(),
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Effect::None => Icon::Close,
            Effect::Border => Icon::Border,
            Effect::DropShadow => Icon::DropShadow,
            Effect::RoundedFrame => Icon::Frame,
            Effect::TornEdge => Icon::TornEdge,
            Effect::Perspective => Icon::Perspective,
            Effect::Spotlight => Icon::Spotlight,
            Effect::ColorFilter => Icon::ColorFilter,
        }
    }

    /// The backdrop this effect sets; `None` for the effects the engine
    /// cannot draw yet (and for [`Effect::None`], which clears it).
    pub fn backdrop(self) -> Option<CanvasBackdrop> {
        let frame =
            |margin_px, corner_radius_px, shadow, border| CanvasBackdrop::RoundedShadowFrame {
                margin_px,
                corner_radius_px,
                backdrop_fill: Fill::None,
                shadow,
                border,
            };
        match self {
            Effect::Border => Some(frame(
                0.0,
                0.0,
                None,
                Some(Stroke {
                    color: Color::rgb(0x33, 0x37, 0x3E),
                    width: 4.0,
                    ..Stroke::default()
                }),
            )),
            Effect::DropShadow => Some(frame(24.0, 0.0, Some(DropShadow::default()), None)),
            Effect::RoundedFrame => Some(CanvasBackdrop::rounded_shadow_frame()),
            _ => None,
        }
    }

    /// Which tile a backdrop shows as active.
    pub fn of(b: &CanvasBackdrop) -> Effect {
        match b {
            CanvasBackdrop::None => Effect::None,
            b => Effect::ALL
                .into_iter()
                .find(|e| e.backdrop().as_ref() == Some(b))
                .unwrap_or(Effect::RoundedFrame),
        }
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

/// Blur / Pixelate strength card: a mosaic of `block` (Pixelate) or a
/// stack of fading rings (Blur, `level` 0..=2), with the preset's name.
fn paint_strength(
    p: &egui::Painter,
    r: ERect,
    block: Option<u32>,
    level: usize,
    label: &str,
    t: &Tokens,
) {
    let art = ERect::from_min_max(r.min, pos2(r.max.x, r.max.y - 14.0));
    match block {
        Some(b) => {
            let cell = (b as f32 / 2.0).clamp(3.0, art.height() / 2.0);
            let (cols, rows) = (
                (art.width() / cell).floor() as i32,
                (art.height() / cell).floor() as i32,
            );
            for i in 0..cols {
                for j in 0..rows {
                    let shade = if (i * 7 + j * 3) % 3 == 0 {
                        t.field_border
                    } else if (i + j) % 2 == 0 {
                        t.card_hover
                    } else {
                        t.text_faint
                    };
                    let c = ERect::from_min_size(
                        art.min + vec2(i as f32 * cell, j as f32 * cell),
                        Vec2::splat(cell),
                    );
                    p.rect_filled(c, 0.0, shade);
                }
            }
        }
        None => {
            let rad = art.height() * 0.3;
            for k in 0..=(level + 1) * 2 {
                let a = 0.5 / (k as f32 + 1.0);
                p.circle_filled(
                    art.center(),
                    rad + k as f32 * 2.0,
                    t.text_dim.gamma_multiply(a),
                );
            }
        }
    }
    p.text(
        pos2(r.center().x, r.max.y),
        Align2::CENTER_BOTTOM,
        label,
        FontId::proportional(11.0),
        t.text_dim,
    );
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
    fn backdrop_fills_span_the_frame() {
        let frame = Size::new(200, 100);
        let (red, blue) = (Color::rgb(255, 0, 0), Color::rgb(0, 0, 255));
        let f = backdrop_fill_of(BackdropKind::Linear, red, blue, frame);
        assert_eq!(fill_parts(&f), (BackdropKind::Linear, red, blue));
        let Fill::Linear { to, .. } = f else {
            unreachable!()
        };
        assert_eq!((to.x, to.y), (200.0, 100.0));
        let f = backdrop_fill_of(BackdropKind::Radial, red, blue, frame);
        let Fill::Radial { center, radius, .. } = &f else {
            unreachable!()
        };
        assert_eq!((center.x, center.y), (100.0, 50.0));
        assert!((f64::from(*radius) - 200f64.hypot(100.0) / 2.0).abs() < 1e-3);
        assert_eq!(
            fill_parts(&backdrop_fill_of(BackdropKind::Solid, red, blue, frame)),
            (BackdropKind::Solid, red, red)
        );
        // The engine's format accepts what the panel builds.
        let mut m = capturecraft_format::Manifest::new(Size::new(10, 10));
        if let CanvasBackdrop::RoundedShadowFrame { backdrop_fill, .. } =
            CanvasBackdrop::rounded_shadow_frame()
        {
            assert!(matches!(backdrop_fill, Fill::Solid { .. }));
        }
        m.canvas.backdrop = CanvasBackdrop::RoundedShadowFrame {
            margin_px: 20.0,
            corner_radius_px: 8.0,
            backdrop_fill: f,
            shadow: None,
            border: None,
        };
        assert!(m.validate().is_ok());
    }

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
            assert!(Fields::of(tool).any(), "{tool:?}");
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
