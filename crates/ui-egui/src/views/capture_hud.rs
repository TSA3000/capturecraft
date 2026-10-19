//! The floating Capture HUD: a compact, dark, borderless, always-on-top
//! window.
//!
//! ```text
//! ┌ ≡ CaptureCraft Capture ─────────────────────────────── ▾ 🗗 ⚙ ✖ ┐
//! │ All-in-One │ Selection  Region  Window  Fullscreen  Scrolling │         │
//! │ Image      │ Effects    None  Border  Shadow  Torn  Frame     │ ●Capture│
//! │ Video      │ Share      Editor  Clipboard  File               │ PrtSc   │
//! │            │ (Cursor) (No delay ▾) (Mic) (System audio)       │         │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! The header strip drags the window (`ViewportCommand::StartDrag`).
//! Settings live in [`CaptureHud`] (per tab, as Snagit keeps them); the
//! Capture button and the shell buttons are [`UiIntent`]s for the app,
//! which applies the Effects ([`effects_backdrop`]) and the Share
//! destination once the capture is open.

use capturecraft_format::{Axis, CanvasBackdrop, Color, DropShadow, Fill, Stroke};
use egui::{
    Align, Color32, Context, Layout, RichText, Sense, Ui, Vec2, ViewportBuilder, ViewportCommand,
    ViewportId, pos2, vec2,
};

use crate::UiIntent;
use crate::state::{
    CaptureEffect, CaptureHud, CaptureTab, ImageFormat, ScrollMode, SelectionType, ShareDestination,
};
use crate::theme::{Tokens, stroke};

pub fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("capture_hud")
}

/// Window size in points, expanded and collapsed.
pub const HUD_SIZE: Vec2 = vec2(720.0, 220.0);
pub const HUD_SIZE_COLLAPSED: Vec2 = vec2(720.0, 44.0);

/// Delays the HUD offers, seconds.
pub const DELAYS: [u32; 6] = [0, 1, 2, 3, 5, 10];

/// The selection modes on the HUD, in order.
pub const MODES: [SelectionType; 4] = [
    SelectionType::Region,
    SelectionType::Window,
    SelectionType::Fullscreen,
    SelectionType::Scrolling {
        axis: Axis::Vertical,
        mode: ScrollMode::Auto,
    },
];

pub fn mode_label(s: &SelectionType) -> &'static str {
    match s {
        SelectionType::Region => "⛶ Region",
        SelectionType::Window => "🗔 Window",
        SelectionType::Fullscreen => "🖵 Fullscreen",
        SelectionType::Scrolling { .. } => "⇕ Scrolling",
        SelectionType::Menu => "☰ Menu",
    }
}

/// Same mode, ignoring the scrolling sub-options.
pub fn same_mode(a: &SelectionType, b: &SelectionType) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

// ---------------------------------------------------------------------------
// Effects

/// The Effects row's chips, with the settings a new one starts from.
pub fn effect_presets() -> [CaptureEffect; 4] {
    [
        CaptureEffect::Border {
            color: Color::rgb(0x33, 0x41, 0x55),
            width: 2.0,
        },
        CaptureEffect::Shadow {
            shadow: DropShadow {
                color: Color::rgba(0x0F, 0x17, 0x2A, 0x59),
                offset_x: 0.0,
                offset_y: 6.0,
                blur: 18.0,
                spread: 0.0,
            },
        },
        CaptureEffect::TornEdge {
            sides: [false, false, true, false],
            amplitude_px: 8.0,
        },
        CaptureEffect::RoundedFrame,
    ]
}

pub fn effect_label(e: &CaptureEffect) -> &'static str {
    match e {
        CaptureEffect::Border { .. } => "Border",
        CaptureEffect::Shadow { .. } => "Drop Shadow",
        CaptureEffect::TornEdge { .. } => "Torn Edge",
        CaptureEffect::RoundedFrame => "Rounded Frame",
    }
}

fn same_effect(a: &CaptureEffect, b: &CaptureEffect) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Turns `preset`'s kind on (with the preset's settings) or off.
pub fn toggle_effect(effects: &mut Vec<CaptureEffect>, preset: CaptureEffect) {
    match effects.iter().position(|e| same_effect(e, &preset)) {
        Some(i) => {
            effects.remove(i);
        }
        None => effects.push(preset),
    }
}

/// The canvas backdrop that renders `effects`, `None` when there is
/// nothing to draw. A drop shadow gets a transparent margin wide enough
/// for its blur; a border alone is a frame without margin. Torn edges are
/// not a backdrop (see [`unsupported_effects`]).
pub fn effects_backdrop(effects: &[CaptureEffect]) -> Option<CanvasBackdrop> {
    let mut margin = 0.0f32;
    let mut radius = 0.0f32;
    let mut fill = Fill::Solid {
        color: Color::TRANSPARENT,
    };
    let mut drop = None;
    let mut edge = None;
    let mut any = false;
    for e in effects {
        match e {
            CaptureEffect::RoundedFrame => {
                if let CanvasBackdrop::RoundedShadowFrame {
                    margin_px,
                    corner_radius_px,
                    backdrop_fill,
                    shadow,
                    ..
                } = CanvasBackdrop::rounded_shadow_frame()
                {
                    margin = margin.max(margin_px);
                    radius = corner_radius_px;
                    fill = backdrop_fill;
                    drop = drop.or(shadow);
                }
                any = true;
            }
            CaptureEffect::Shadow { shadow } => {
                let reach = shadow.blur.max(0.0)
                    + shadow.spread.max(0.0)
                    + shadow.offset_x.abs().max(shadow.offset_y.abs());
                margin = margin.max(reach.ceil());
                drop = Some(*shadow);
                any = true;
            }
            CaptureEffect::Border { color, width } => {
                edge = Some(Stroke {
                    color: *color,
                    width: *width,
                    ..Stroke::default()
                });
                any = true;
            }
            CaptureEffect::TornEdge { .. } => {}
        }
    }
    any.then_some(CanvasBackdrop::RoundedShadowFrame {
        margin_px: margin,
        corner_radius_px: radius,
        backdrop_fill: fill,
        shadow: drop,
        border: edge,
    })
}

/// Effects the app cannot apply yet, for a notice.
pub fn unsupported_effects(effects: &[CaptureEffect]) -> Vec<&'static str> {
    effects
        .iter()
        .filter(|e| matches!(e, CaptureEffect::TornEdge { .. }))
        .map(effect_label)
        .collect()
}

// ---------------------------------------------------------------------------
// Share

/// The Share row's destinations, in order.
pub fn share_presets() -> [ShareDestination; 3] {
    [
        ShareDestination::Editor,
        ShareDestination::Clipboard,
        ShareDestination::File {
            // Empty: the app's captures folder.
            directory: String::new(),
            name_pattern: "Capture %Y-%m-%d %H%M%S".into(),
            format: ImageFormat::Png,
        },
    ]
}

pub fn share_label(s: &ShareDestination) -> &'static str {
    match s {
        ShareDestination::Editor => "Editor",
        ShareDestination::Clipboard => "Clipboard",
        ShareDestination::File { .. } => "File",
    }
}

fn same_share(a: &ShareDestination, b: &ShareDestination) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

// ---------------------------------------------------------------------------
// The window

/// The viewport the HUD wants: borderless, transparent corners, fixed size.
pub fn viewport_builder(hud: &CaptureHud) -> ViewportBuilder {
    let size = if hud.window.collapsed {
        HUD_SIZE_COLLAPSED
    } else {
        HUD_SIZE
    };
    let mut b = ViewportBuilder::default()
        .with_title("CaptureCraft Capture")
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_taskbar(false)
        .with_inner_size(size);
    if hud.window.always_on_top {
        b = b.with_always_on_top();
    }
    if let Some([x, y]) = hud.window.position {
        b = b.with_position(pos2(x, y));
    }
    b
}

/// What the Capture button starts.
pub fn capture_intent(hud: &CaptureHud) -> UiIntent {
    let s = hud.current();
    UiIntent::StartCapture {
        tab: hud.tab,
        selection: s.selection,
        delay_secs: s.delay_secs,
    }
}

pub fn delay_label(secs: u32) -> String {
    if secs == 0 {
        "No delay".into()
    } else {
        format!("{secs} s delay")
    }
}

/// Width of the left mode rail and of the right action area, points.
const RAIL_W: f32 = 104.0;
const ACTION_W: f32 = 148.0;
/// Width of a row's caption.
const CAPTION_W: f32 = 64.0;

pub struct CaptureHudView<'a> {
    pub tokens: &'a Tokens,
}

impl CaptureHudView<'_> {
    /// Opens (or keeps open) the HUD viewport for this frame.
    pub fn show_viewport(&self, ctx: &Context, hud: &mut CaptureHud) -> Vec<UiIntent> {
        ctx.show_viewport_immediate(viewport_id(), viewport_builder(hud), |ui, _| {
            let mut out = self.show(ui, hud);
            if ui.input(|i| i.viewport().close_requested()) {
                out.push(UiIntent::CloseHud);
            }
            out
        })
    }

    /// Draws the HUD into `root` (the HUD viewport's root `Ui`).
    pub fn show(&self, root: &mut Ui, hud: &mut CaptureHud) -> Vec<UiIntent> {
        let t = self.tokens;
        let mut out = Vec::new();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::TRANSPARENT))
            .show(root, |ui| {
                egui::Frame::new()
                    .fill(t.panel)
                    .stroke(stroke(1.0, t.card_border))
                    .corner_radius(Tokens::corner(t.radius_lg))
                    .inner_margin(egui::Margin::same(t.space_md as i8))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        self.header(ui, hud, &mut out);
                        if !hud.window.collapsed {
                            ui.add_space(t.space_sm);
                            self.body(ui, hud, &mut out);
                        }
                    });
            });
        if root.input(|i| i.key_pressed(egui::Key::Escape)) {
            out.push(UiIntent::CloseHud);
        }
        out
    }

    fn header(&self, ui: &mut Ui, hud: &mut CaptureHud, out: &mut Vec<UiIntent>) {
        let t = self.tokens;
        ui.horizontal(|ui| {
            // Grip and title: drag the borderless window.
            let (grip, resp) = ui.allocate_exact_size(vec2(18.0, 22.0), Sense::drag());
            for i in 0..3 {
                let y = grip.min.y + 6.0 + i as f32 * 5.0;
                ui.painter()
                    .hline(grip.x_range().shrink(3.0), y, stroke(1.5, t.text_faint));
            }
            let title = ui.add(
                egui::Label::new(RichText::new("CaptureCraft Capture").strong())
                    .sense(Sense::drag()),
            );
            if resp.drag_started() || title.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("✖").on_hover_text("Close").clicked() {
                    out.push(UiIntent::CloseHud);
                }
                if ui.small_button("⚙").on_hover_text("Settings").clicked() {
                    out.push(UiIntent::OpenSettings);
                }
                if ui
                    .small_button("🗗")
                    .on_hover_text("Open the editor")
                    .clicked()
                {
                    out.push(UiIntent::ShowEditor);
                }
                let arrow = if hud.window.collapsed { "▸" } else { "▾" };
                if ui
                    .small_button(arrow)
                    .on_hover_text("Collapse/expand")
                    .clicked()
                {
                    hud.window.collapsed = !hud.window.collapsed;
                    let size = if hud.window.collapsed {
                        HUD_SIZE_COLLAPSED
                    } else {
                        HUD_SIZE
                    };
                    ui.ctx().send_viewport_cmd(ViewportCommand::InnerSize(size));
                }
            });
        });
    }

    fn body(&self, ui: &mut Ui, hud: &mut CaptureHud, out: &mut Vec<UiIntent>) {
        let t = self.tokens;
        let h = ui.available_height();
        ui.horizontal_top(|ui| {
            // --- Left: mode rail ---------------------------------------------
            ui.allocate_ui_with_layout(
                vec2(RAIL_W, h),
                Layout::top_down_justified(Align::Min),
                |ui| {
                    for tab in CaptureTab::ALL {
                        let text = RichText::new(tab.label()).size(14.0);
                        ui.selectable_value(&mut hud.tab, tab, text);
                    }
                },
            );
            ui.separator();
            // --- Centre: configuration rows ----------------------------------
            let centre_w = (ui.available_width() - ACTION_W - t.space_md).max(200.0);
            ui.allocate_ui_with_layout(vec2(centre_w, h), Layout::top_down(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = t.space_sm;
                self.rows(ui, hud);
            });
            ui.separator();
            // --- Right: the Capture button -----------------------------------
            ui.allocate_ui_with_layout(vec2(ACTION_W, h), Layout::top_down(Align::Center), |ui| {
                ui.add_space(t.space_lg);
                let label = if hud.tab == CaptureTab::Video {
                    "● Record"
                } else {
                    "● Capture"
                };
                let capture = egui::Button::new(
                    RichText::new(label)
                        .color(t.accent_capture_text)
                        .strong()
                        .size(18.0),
                )
                .fill(t.accent_capture)
                .corner_radius(Tokens::corner(t.radius_pill))
                .min_size(vec2(ACTION_W - t.space_md, 48.0));
                if ui.add(capture).clicked() {
                    out.push(capture_intent(hud));
                }
                ui.add_space(t.space_sm);
                egui::Frame::new()
                    .fill(t.badge_bg)
                    .corner_radius(Tokens::corner(t.radius_sm))
                    .inner_margin(egui::Margin::symmetric(t.space_sm as i8, t.space_xs as i8))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(&hud.hotkey)
                                .monospace()
                                .size(11.0)
                                .color(t.badge_text),
                        );
                    })
                    .response
                    .on_hover_text("Capture hotkey");
            });
        });
    }

    fn rows(&self, ui: &mut Ui, hud: &mut CaptureHud) {
        let video = hud.tab == CaptureTab::Video;
        // The audio toggles live beside the tab settings `s` borrows.
        let (mut mic, mut audio) = (hud.video.record_microphone, hud.video.record_system_audio);
        let s = hud.current_mut();
        row(ui, "Selection", |ui| {
            for mode in MODES {
                let on = same_mode(&s.selection, &mode);
                if ui.selectable_label(on, mode_label(&mode)).clicked() && !on {
                    s.selection = mode;
                }
            }
        });
        row(ui, "Effects", |ui| {
            if ui.selectable_label(s.effects.is_empty(), "None").clicked() {
                s.effects.clear();
            }
            for preset in effect_presets() {
                let on = s.effects.iter().any(|e| same_effect(e, &preset));
                let label = effect_label(&preset);
                let resp = ui.selectable_label(on, label);
                let resp = if matches!(preset, CaptureEffect::TornEdge { .. }) {
                    resp.on_hover_text("Not applied yet: the capture opens without it")
                } else {
                    resp
                };
                if resp.clicked() {
                    toggle_effect(&mut s.effects, preset);
                }
            }
        });
        row(ui, "Share", |ui| {
            for dest in share_presets() {
                let on = same_share(&s.share, &dest);
                let label = share_label(&dest);
                if ui.selectable_label(on, label).clicked() && !on {
                    s.share = dest;
                }
            }
            ui.checkbox(&mut s.copy_to_clipboard, "Also copy");
        });
        ui.horizontal(|ui| {
            ui.add_space(CAPTION_W + ui.spacing().item_spacing.x);
            ui.toggle_value(&mut s.capture_cursor, "🖱 Cursor")
                .on_hover_text("Capture the cursor as its own layer");
            egui::ComboBox::from_id_salt("hud.delay")
                .width(84.0)
                .selected_text(format!("⏱ {}", delay_label(s.delay_secs)))
                .show_ui(ui, |ui| {
                    for d in DELAYS {
                        ui.selectable_value(&mut s.delay_secs, d, delay_label(d));
                    }
                });
            ui.add_enabled_ui(video, |ui| {
                ui.toggle_value(&mut mic, "🎤 Mic")
                    .on_disabled_hover_text("Video tab only");
                ui.toggle_value(&mut audio, "🔊 System audio")
                    .on_disabled_hover_text("Video tab only");
            });
        });
        hud.video.record_microphone = mic;
        hud.video.record_system_audio = audio;
    }
}

/// One configuration row: a caption, then the row's chips.
fn row(ui: &mut Ui, caption: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            vec2(CAPTION_W, ui.spacing().interact_size.y),
            egui::Label::new(RichText::new(caption).weak()),
        );
        add(ui);
    });
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn capture_uses_the_current_tab() {
        let mut hud = CaptureHud {
            tab: CaptureTab::Image,
            ..CaptureHud::default()
        };
        hud.current_mut().selection = SelectionType::Window;
        hud.current_mut().delay_secs = 3;
        assert_eq!(
            capture_intent(&hud),
            UiIntent::StartCapture {
                tab: CaptureTab::Image,
                selection: SelectionType::Window,
                delay_secs: 3,
            }
        );
        // Other tabs keep their own settings.
        hud.tab = CaptureTab::AllInOne;
        assert_eq!(hud.current().selection, SelectionType::Region);
    }

    #[test]
    fn modes_compare_without_scroll_options() {
        assert!(same_mode(
            &SelectionType::Scrolling {
                axis: Axis::Horizontal,
                mode: ScrollMode::Manual
            },
            &MODES[3]
        ));
        assert!(!same_mode(&SelectionType::Region, &SelectionType::Window));
        assert_eq!(delay_label(0), "No delay");
        assert_eq!(delay_label(5), "5 s delay");
        assert_eq!(DELAYS.first(), Some(&0));
        assert_eq!(DELAYS.last(), Some(&10));
    }

    #[test]
    fn viewport_is_borderless_and_on_top() {
        let mut hud = CaptureHud::default();
        let b = viewport_builder(&hud);
        assert_eq!(b.decorations, Some(false));
        assert_eq!(b.transparent, Some(true));
        assert_eq!(b.resizable, Some(false));
        assert_eq!(b.inner_size, Some(vec2(720.0, 220.0)));
        assert_eq!(b.window_level, Some(egui::WindowLevel::AlwaysOnTop));
        assert_eq!(b.title.as_deref(), Some("CaptureCraft Capture"));
        hud.window.collapsed = true;
        hud.window.position = Some([10.0, 20.0]);
        let b = viewport_builder(&hud);
        assert_eq!(b.inner_size, Some(HUD_SIZE_COLLAPSED));
        assert_eq!(b.position, Some(pos2(10.0, 20.0)));
    }

    #[test]
    fn effects_toggle_by_kind() {
        let mut fx = Vec::new();
        let [border, shadow, _, frame] = effect_presets();
        toggle_effect(&mut fx, border);
        toggle_effect(&mut fx, frame);
        assert_eq!(fx.len(), 2);
        // A tuned border is still the Border chip.
        fx[0] = CaptureEffect::Border {
            color: Color::BLACK,
            width: 9.0,
        };
        toggle_effect(&mut fx, border);
        assert_eq!(fx, [CaptureEffect::RoundedFrame]);
        toggle_effect(&mut fx, shadow);
        assert_eq!(fx.len(), 2);
    }

    #[test]
    fn effects_become_one_backdrop() {
        assert_eq!(effects_backdrop(&[]), None);
        let [border, shadow, torn, frame] = effect_presets();
        assert_eq!(effects_backdrop(std::slice::from_ref(&torn)), None);
        assert_eq!(unsupported_effects(&[border, torn]), ["Torn Edge"]);

        // Rounded Frame alone is the editor's Frame toggle.
        assert_eq!(
            effects_backdrop(&[frame]),
            Some(CanvasBackdrop::rounded_shadow_frame())
        );
        // A border alone: no margin, no shadow, square corners.
        let Some(CanvasBackdrop::RoundedShadowFrame {
            margin_px,
            corner_radius_px,
            shadow: None,
            border: Some(b),
            ..
        }) = effects_backdrop(&[border])
        else {
            panic!("border frame expected")
        };
        assert_eq!((margin_px, corner_radius_px, b.width), (0.0, 0.0, 2.0));
        // A shadow gets room for its blur and offset on a clear backdrop.
        let Some(CanvasBackdrop::RoundedShadowFrame {
            margin_px,
            backdrop_fill,
            shadow: Some(_),
            ..
        }) = effects_backdrop(&[shadow])
        else {
            panic!("shadow frame expected")
        };
        assert_eq!(margin_px, 24.0);
        assert_eq!(
            backdrop_fill,
            Fill::Solid {
                color: Color::TRANSPARENT
            }
        );
        // Everything together keeps the frame's margin and adds the border.
        let Some(CanvasBackdrop::RoundedShadowFrame {
            margin_px,
            corner_radius_px,
            border: Some(_),
            ..
        }) = effects_backdrop(&[border, shadow, frame])
        else {
            panic!()
        };
        assert_eq!((margin_px, corner_radius_px), (48.0, 12.0));
    }

    #[test]
    fn share_presets_cover_editor_clipboard_and_file() {
        let labels = share_presets().map(|s| share_label(&s));
        assert_eq!(labels, ["Editor", "Clipboard", "File"]);
        assert!(matches!(
            &share_presets()[2],
            ShareDestination::File { directory, format: ImageFormat::Png, .. } if directory.is_empty()
        ));
    }
}
