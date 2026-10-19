//! The floating Capture HUD: a compact, borderless, always-on-top window
//! with the capture tabs (All-in-One, Image, Video), the selection mode,
//! a delay, the cursor toggle and the orange Capture button.
//!
//! The window has no title bar: the header strip drags it
//! (`ViewportCommand::StartDrag`) and carries Editor, Settings and Close.
//! Settings live in [`CaptureHud`] (per tab, as Snagit keeps them); the
//! Capture button and the shell buttons are [`UiIntent`]s for the app.

use egui::{
    Align, Color32, Context, Layout, RichText, Sense, Ui, Vec2, ViewportBuilder, ViewportCommand,
    ViewportId, pos2, vec2,
};

use crate::UiIntent;
use crate::state::{CaptureHud, CaptureTab, ScrollMode, SelectionType};
use crate::theme::{Tokens, stroke};
use capturecraft_format::Axis;

pub fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("capture_hud")
}

/// Window size in points, expanded and collapsed.
pub const HUD_SIZE: Vec2 = vec2(380.0, 148.0);
pub const HUD_SIZE_COLLAPSED: Vec2 = vec2(380.0, 44.0);

/// Delays the HUD offers, seconds.
pub const DELAYS: [u32; 4] = [0, 3, 5, 10];

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
            // Grip: drags the borderless window.
            let (grip, resp) = ui.allocate_exact_size(vec2(18.0, 22.0), Sense::drag());
            for i in 0..3 {
                let y = grip.min.y + 6.0 + i as f32 * 5.0;
                ui.painter()
                    .hline(grip.x_range().shrink(3.0), y, stroke(1.5, t.text_faint));
            }
            if resp.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            for tab in CaptureTab::ALL {
                ui.selectable_value(&mut hud.tab, tab, tab.label());
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
        ui.horizontal(|ui| {
            let s = hud.current_mut();
            for mode in MODES {
                let on = same_mode(&s.selection, &mode);
                if ui.selectable_label(on, mode_label(&mode)).clicked() && !on {
                    s.selection = mode;
                }
            }
        });
        ui.add_space(t.space_sm);
        ui.horizontal(|ui| {
            let s = hud.current_mut();
            egui::ComboBox::from_id_salt("hud.delay")
                .width(72.0)
                .selected_text(delay_label(s.delay_secs))
                .show_ui(ui, |ui| {
                    for d in DELAYS {
                        ui.selectable_value(&mut s.delay_secs, d, delay_label(d));
                    }
                });
            ui.checkbox(&mut s.capture_cursor, "Cursor");
            ui.checkbox(&mut s.copy_to_clipboard, "Copy");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let capture = egui::Button::new(
                    RichText::new("● Capture")
                        .color(t.accent_capture_text)
                        .strong()
                        .size(15.0),
                )
                .fill(t.accent_capture)
                .corner_radius(Tokens::corner(t.radius_pill))
                .min_size(vec2(112.0, 34.0));
                if ui
                    .add(capture)
                    .on_hover_text(format!("Capture ({})", hud.hotkey))
                    .clicked()
                {
                    out.push(capture_intent(hud));
                }
            });
        });
    }
}

pub fn delay_label(secs: u32) -> String {
    if secs == 0 {
        "No delay".into()
    } else {
        format!("{secs} s delay")
    }
}

#[cfg(test)]
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
    }

    #[test]
    fn viewport_is_borderless_and_on_top() {
        let mut hud = CaptureHud::default();
        let b = viewport_builder(&hud);
        assert_eq!(b.decorations, Some(false));
        assert_eq!(b.transparent, Some(true));
        assert_eq!(b.inner_size, Some(HUD_SIZE));
        assert_eq!(b.window_level, Some(egui::WindowLevel::AlwaysOnTop));
        hud.window.collapsed = true;
        hud.window.position = Some([10.0, 20.0]);
        let b = viewport_builder(&hud);
        assert_eq!(b.inner_size, Some(HUD_SIZE_COLLAPSED));
        assert_eq!(b.position, Some(pos2(10.0, 20.0)));
    }
}
