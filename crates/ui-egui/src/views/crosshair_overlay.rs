//! The full-screen crosshair overlay over the frozen desktop.
//!
//! One borderless, always-on-top viewport (`crosshair_overlay`) spans the
//! bounding box of every display and shows the desktop as it was when the
//! hotkey fired ([`FrozenDesktop`]). Hovering highlights the window,
//! control or monitor `FrozenDesktop::hit_test` reports, with a scrim over
//! everything else; a click locks that target, a drag selects a custom
//! region with a live `W × H px` badge. A 15 × 15 nearest-neighbour pixel
//! loupe follows the cursor and flips quadrant near screen edges. Once a
//! selection is locked, a floating action bar under it offers Capture
//! Image, Record Video, Panorama, Redo Selection and Cancel.
//!
//! The view never captures anything: it returns an [`OverlayAction`] and
//! the app crops (`FrozenDesktop::crop`) or starts a recording. The gesture
//! logic is the pure [`step`] function, unit-tested without a display.
//!
//! HiDPI: positions on screen are [`LogicalPt`]s, desktop pixels are
//! [`PhysicalPx`]s (see [`crate::dpi`]); the mapping is rebuilt every frame
//! from the overlay viewport's own `pixels_per_point`, and the size badges
//! count the frozen frames' physical pixels. One egui viewport has one
//! scale, so on a desktop mixing scales the overlay is exact on the
//! monitor whose scale egui reports and the badges stay exact everywhere.

use capturecraft_capture::{FrozenDesktop, HitKind, HitTarget, PointI32};
use egui::{
    Align2, Color32, ColorImage, Context, CursorIcon, FontId, Key, Pos2, Rect as ERect, Sense,
    StrokeKind, TextureHandle, TextureOptions, Vec2, ViewportBuilder, ViewportId, pos2, vec2,
};
use photocraft_geom::Rect;

use crate::dpi::{LogicalPt, PhysicalPx, dimension_badge, physical_size, sane_ppp};
use crate::state::{
    ColorCode, CrosshairOverlayState, CrosshairPrefs, HoverTarget, OverlayPhase, SelectionSource,
};
use crate::theme::{Tokens, stroke};

pub fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("crosshair_overlay")
}

/// Selections smaller than this (either side, desktop units) are treated as
/// a stray click-drag and dropped.
pub const MIN_SELECTION: u32 = 3;

/// What the user chose.
#[derive(Clone, Debug, PartialEq)]
pub enum OverlayAction {
    CaptureImage {
        rect: Rect,
        source: SelectionSource,
    },
    RecordVideo {
        rect: Rect,
        source: SelectionSource,
    },
    /// Scrolling capture of the region.
    Panorama {
        rect: Rect,
        source: SelectionSource,
    },
    Cancel,
}

// ---------------------------------------------------------------------------
// Coordinates

/// Maps global desktop units onto the overlay viewport's points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DesktopMapping {
    /// `FrozenDesktop::bounds`.
    pub desktop: Rect,
    pub points_per_unit: f32,
    /// macOS reports the desktop in points; Windows and X11 in physical
    /// pixels, which egui divides by `pixels_per_point`.
    pub units_are_points: bool,
}

impl DesktopMapping {
    pub fn new(desktop: Rect, units_are_points: bool, pixels_per_point: f32) -> Self {
        Self {
            desktop,
            points_per_unit: if units_are_points {
                1.0
            } else {
                1.0 / sane_ppp(pixels_per_point)
            },
            units_are_points,
        }
    }

    /// The same desktop at another scale (the overlay viewport's own
    /// `pixels_per_point`, read every frame).
    pub fn with_pixels_per_point(self, pixels_per_point: f32) -> Self {
        Self::new(self.desktop, self.units_are_points, pixels_per_point)
    }

    /// Desktop units per point: the scale of the unit ↔ point conversion.
    fn units_per_point(&self) -> f32 {
        1.0 / self.points_per_unit.max(f32::EPSILON)
    }

    /// `p` relative to the desktop's top-left.
    fn relative(&self, p: [i32; 2]) -> PhysicalPx {
        let d = |v: i32, o: i32| {
            (i64::from(v) - i64::from(o)).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
        };
        PhysicalPx::new(d(p[0], self.desktop.x0), d(p[1], self.desktop.y0))
    }

    /// Top-left corner of desktop unit `p`, in viewport points.
    pub fn to_screen(&self, p: [i32; 2]) -> Pos2 {
        self.relative(p)
            .to_logical(self.units_per_point())
            .to_pos2()
    }

    /// Centre of desktop unit `p`, in viewport points.
    pub fn center_to_screen(&self, p: [i32; 2]) -> Pos2 {
        self.relative(p)
            .center_to_logical(self.units_per_point())
            .to_pos2()
    }

    pub fn rect_to_screen(&self, r: Rect) -> ERect {
        ERect::from_min_max(self.to_screen([r.x0, r.y0]), self.to_screen([r.x1, r.y1]))
    }

    /// The desktop unit under viewport point `pos`.
    pub fn to_global(&self, pos: Pos2) -> [i32; 2] {
        let p = LogicalPt::from_pos2(pos).to_physical(self.units_per_point());
        let f = |v: i32, o: i32| {
            (i64::from(v) + i64::from(o)).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
        };
        [f(p.x, self.desktop.x0), f(p.y, self.desktop.y0)]
    }

    /// Where and how large the overlay window opens, in points.
    pub fn viewport(&self) -> ViewportBuilder {
        let s = self.points_per_unit;
        ViewportBuilder::default()
            .with_title("CaptureCraft crosshair")
            .with_position(pos2(self.desktop.x0 as f32 * s, self.desktop.y0 as f32 * s))
            .with_inner_size(vec2(
                self.desktop.width() as f32 * s,
                self.desktop.height() as f32 * s,
            ))
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(true)
    }
}

// ---------------------------------------------------------------------------
// Gesture logic

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayEvent {
    Hover([i32; 2]),
    Click([i32; 2]),
    DragStart([i32; 2]),
    DragTo([i32; 2]),
    DragEnd([i32; 2]),
    /// Enter: capture the locked selection.
    Confirm,
    /// "Redo Selection": back to hovering.
    Redo,
    /// Esc.
    Cancel,
}

/// The hover target for a hit, `None` outside every display.
pub fn hover_target(hit: &HitTarget) -> Option<HoverTarget> {
    if hit.rect.is_empty() {
        return None;
    }
    let window = hit.window.as_ref();
    let app = window.map(|w| match (w.app_name.as_str(), w.title.as_str()) {
        ("", t) => t.to_string(),
        (a, "") => a.to_string(),
        (a, t) => format!("{a} — {t}"),
    });
    let (source, label) = match hit.kind {
        HitKind::Nothing => return None,
        HitKind::Display => (
            SelectionSource::Display {
                id: hit.display_id.unwrap_or_default(),
            },
            format!("Display {}", hit.display_id.unwrap_or_default()),
        ),
        HitKind::Window => (
            SelectionSource::Window {
                id: window.map_or(0, |w| w.id),
                element: false,
            },
            app.unwrap_or_default(),
        ),
        HitKind::Element => {
            let role = hit
                .element
                .as_ref()
                .map(|e| match e.name.as_str() {
                    "" => format!("{:?}", e.role),
                    n => format!("{:?} “{n}”", e.role),
                })
                .unwrap_or_default();
            (
                SelectionSource::Window {
                    id: window.map_or(0, |w| w.id),
                    element: true,
                },
                match app {
                    Some(a) => format!("{role} · {a}"),
                    None => role,
                },
            )
        }
    };
    Some(HoverTarget {
        rect: hit.rect,
        source,
        label,
    })
}

/// Advances the overlay by one input event. `hit` is asked for the
/// target under the cursor only while hovering with snapping on.
pub fn step(
    state: &mut CrosshairOverlayState,
    event: OverlayEvent,
    snap: bool,
    hit: impl FnOnce([i32; 2]) -> Option<HoverTarget>,
) -> Option<OverlayAction> {
    match (event, &mut state.phase) {
        (OverlayEvent::Cancel, _) => return Some(OverlayAction::Cancel),
        (OverlayEvent::Hover(p), phase) => {
            state.cursor = Some(p);
            if matches!(phase, OverlayPhase::Hovering) {
                state.hover = if snap { hit(p) } else { None };
            }
        }
        (OverlayEvent::Click(p), OverlayPhase::Hovering) => {
            state.cursor = Some(p);
            if let Some(t) = state.hover.take() {
                state.phase = OverlayPhase::Selected {
                    rect: t.rect,
                    source: t.source,
                };
            }
        }
        (OverlayEvent::DragStart(p), OverlayPhase::Hovering) => {
            state.hover = None;
            state.phase = OverlayPhase::Dragging { from: p, to: p };
        }
        (OverlayEvent::DragTo(p), OverlayPhase::Dragging { to, .. }) => {
            *to = p;
            state.cursor = Some(p);
        }
        (OverlayEvent::DragEnd(p), OverlayPhase::Dragging { from, .. }) => {
            let rect = OverlayPhase::drag_rect(*from, p);
            state.phase = if rect.width() < MIN_SELECTION || rect.height() < MIN_SELECTION {
                OverlayPhase::Hovering
            } else {
                OverlayPhase::Selected {
                    rect,
                    source: SelectionSource::Region,
                }
            };
        }
        (OverlayEvent::Confirm, OverlayPhase::Selected { rect, source }) => {
            return Some(OverlayAction::CaptureImage {
                rect: *rect,
                source: source.clone(),
            });
        }
        (OverlayEvent::Redo, _) => {
            state.phase = OverlayPhase::Hovering;
            state.hover = None;
        }
        _ => {}
    }
    None
}

// ---------------------------------------------------------------------------
// Placement

/// Loupe box beside the cursor: bottom-right by default, flipped left
/// and/or up when it would leave `screen`, then clamped inside it.
pub fn loupe_rect(cursor: Pos2, size: Vec2, screen: ERect, gap: f32) -> ERect {
    let x = if cursor.x + gap + size.x <= screen.max.x {
        cursor.x + gap
    } else {
        cursor.x - gap - size.x
    };
    let y = if cursor.y + gap + size.y <= screen.max.y {
        cursor.y + gap
    } else {
        cursor.y - gap - size.y
    };
    clamp_into(ERect::from_min_size(pos2(x, y), size), screen)
}

/// Action bar under the selection, else above it, else inside its
/// bottom edge; centred on it and kept on screen.
pub fn action_bar_rect(selection: ERect, size: Vec2, screen: ERect, gap: f32) -> ERect {
    let x = selection.center().x - size.x / 2.0;
    let y = if selection.max.y + gap + size.y <= screen.max.y {
        selection.max.y + gap
    } else if selection.min.y - gap - size.y >= screen.min.y {
        selection.min.y - gap - size.y
    } else {
        selection.max.y - gap - size.y
    };
    clamp_into(ERect::from_min_size(pos2(x, y), size), screen)
}

fn clamp_into(r: ERect, screen: ERect) -> ERect {
    let dx = (screen.min.x - r.min.x).max(0.0) + (screen.max.x - r.max.x).min(0.0);
    let dy = (screen.min.y - r.min.y).max(0.0) + (screen.max.y - r.max.y).min(0.0);
    r.translate(vec2(dx, dy))
}

/// `#RRGGBB` or `rgb(r, g, b)` of a pixel.
pub fn color_code(px: [u8; 4], code: ColorCode) -> String {
    let [r, g, b, _] = px;
    match code {
        ColorCode::Hex => format!("#{r:02X}{g:02X}{b:02X}"),
        ColorCode::Rgb => format!("rgb({r}, {g}, {b})"),
    }
}

// ---------------------------------------------------------------------------
// Drawing

/// The frozen displays as egui textures (nearest filtering: the loupe and
/// a 1:1 overlay must not blur).
pub struct DesktopTextures {
    displays: Vec<(Rect, TextureHandle)>,
}

impl DesktopTextures {
    pub fn load(ctx: &Context, desktop: &FrozenDesktop) -> Self {
        let displays = desktop
            .displays()
            .iter()
            .map(|d| {
                let f = &d.frame;
                let image = ColorImage::from_rgba_unmultiplied(
                    [f.width() as usize, f.height() as usize],
                    f.data(),
                );
                let tex = ctx.load_texture(
                    format!("crosshair.display.{}", d.info.id),
                    image,
                    TextureOptions::NEAREST,
                );
                (d.info.rect, tex)
            })
            .collect();
        Self { displays }
    }
}

pub struct CrosshairOverlay<'a> {
    pub desktop: &'a FrozenDesktop,
    pub textures: &'a DesktopTextures,
    pub mapping: DesktopMapping,
    pub prefs: &'a CrosshairPrefs,
    pub tokens: &'a Tokens,
}

impl CrosshairOverlay<'_> {
    /// Opens (or keeps open) the overlay viewport for this frame.
    pub fn show_viewport(
        &self,
        ctx: &Context,
        state: &mut CrosshairOverlayState,
    ) -> Option<OverlayAction> {
        ctx.show_viewport_immediate(viewport_id(), self.mapping.viewport(), |ui, _| {
            let action = self.show(ui, state);
            if ui.input(|i| i.viewport().close_requested()) {
                return Some(OverlayAction::Cancel);
            }
            action
        })
    }

    /// Draws the overlay into `root` (the overlay viewport's root `Ui`).
    pub fn show(
        &self,
        root: &mut egui::Ui,
        state: &mut CrosshairOverlayState,
    ) -> Option<OverlayAction> {
        let ctx = &root.ctx().clone();
        let t = self.tokens;
        let mut action = None;
        let snap = self.prefs.snap_to_windows;
        let desktop = self.desktop;
        let hit = |p: [i32; 2]| hover_target(&desktop.hit_test(PointI32::new(p[0], p[1])));
        let displays = display_scales(desktop);
        let m = self.mapping.with_pixels_per_point(ctx.pixels_per_point());

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(root, |ui| {
                let screen = ui.max_rect();
                let resp = ui.interact(screen, ui.id().with("crosshair"), Sense::click_and_drag());

                // Input → events.
                let mut events = Vec::new();
                if let Some(pos) = resp.hover_pos() {
                    events.push(OverlayEvent::Hover(m.to_global(pos)));
                }
                if resp.drag_started()
                    && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
                {
                    events.push(OverlayEvent::DragStart(m.to_global(origin)));
                }
                if let Some(pos) = resp.interact_pointer_pos() {
                    let g = m.to_global(pos);
                    if resp.dragged() {
                        events.push(OverlayEvent::DragTo(g));
                    }
                    if resp.drag_stopped() {
                        events.push(OverlayEvent::DragEnd(g));
                    }
                    if resp.clicked() {
                        events.push(OverlayEvent::Click(g));
                    }
                }
                ctx.input(|i| {
                    if i.key_pressed(Key::Escape) {
                        events.push(OverlayEvent::Cancel);
                    }
                    if i.key_pressed(Key::Enter) {
                        events.push(OverlayEvent::Confirm);
                    }
                });
                for e in events {
                    if let Some(a) = step(state, e, snap, hit) {
                        action = Some(a);
                    }
                }

                let painter = ui.painter_at(screen);
                for (rect, tex) in &self.textures.displays {
                    painter.image(
                        tex.id(),
                        m.rect_to_screen(*rect),
                        ERect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }

                // Target (locked/dragged selection, else the hover target):
                // scrim outside, outline and label.
                let selection = state.phase.selection();
                let target = selection.or(state.hover.as_ref().map(|h| h.rect));
                let target_rect = target.map(|r| m.rect_to_screen(r).intersect(screen));
                draw_scrim(&painter, screen, target_rect, t.scrim);
                if let Some(r) = target_rect {
                    painter.rect_stroke(
                        r.expand(1.0),
                        0.0,
                        stroke(1.0, t.gizmo_outline),
                        StrokeKind::Outside,
                    );
                    painter.rect_stroke(r, 0.0, stroke(2.0, t.hover_target), StrokeKind::Inside);
                }
                let label = match (&state.phase, &state.hover) {
                    (OverlayPhase::Hovering, Some(h)) => Some((h.rect, h.label.clone())),
                    _ => None,
                };
                if let Some((rect, text)) = label {
                    let (w, h) = physical_size(rect, &displays);
                    let text = format!("{text}   {}", dimension_badge(w, h));
                    let r = m.rect_to_screen(rect);
                    let at = pos2(r.min.x, (r.min.y - t.space_sm).max(screen.min.y + 20.0));
                    badge(&painter, at, Align2::LEFT_BOTTOM, &text, t);
                }

                let cursor = state.cursor;
                let selected = matches!(state.phase, OverlayPhase::Selected { .. });
                if let Some(c) = cursor
                    && !selected
                {
                    let p = m.center_to_screen(c);
                    if self.prefs.crosshair_lines {
                        let s = stroke(1.0, t.crosshair);
                        painter.hline(screen.x_range(), p.y, s);
                        painter.vline(p.x, screen.y_range(), s);
                    }
                    if let (OverlayPhase::Dragging { .. }, Some(sel), true) =
                        (&state.phase, selection, self.prefs.show_dimensions)
                    {
                        let (w, h) = physical_size(sel, &displays);
                        let text = dimension_badge(w, h);
                        badge(
                            &painter,
                            p + vec2(14.0, -14.0),
                            Align2::LEFT_BOTTOM,
                            &text,
                            t,
                        );
                    }
                    if self.prefs.loupe {
                        self.draw_loupe(&painter, screen, p, c);
                    }
                    ctx.set_cursor_icon(CursorIcon::Crosshair);
                }

                if let OverlayPhase::Selected { rect, source } = &state.phase {
                    let (rect, source) = (*rect, source.clone());
                    if let Some(a) =
                        self.action_bar(ctx, screen, m.rect_to_screen(rect), rect, source)
                    {
                        match a {
                            BarChoice::Done(a) => action = Some(a),
                            BarChoice::Redo => {
                                step(state, OverlayEvent::Redo, snap, |_| None);
                            }
                        }
                    }
                }
            });
        action
    }

    fn draw_loupe(&self, painter: &egui::Painter, screen: ERect, cursor_pt: Pos2, at: [i32; 2]) {
        let t = self.tokens;
        let loupe = self
            .desktop
            .loupe(PointI32::new(at[0], at[1]), self.prefs.loupe_radius);
        let cells = loupe.size.max(1) as usize;
        let cell = f32::from(self.prefs.loupe_zoom.max(1));
        let side = cells as f32 * cell;
        let info_h = 22.0;
        let size = vec2(side, side + info_h);
        let r = loupe_rect(cursor_pt, size, screen, 20.0);
        let corner = Tokens::corner(t.radius_sm);
        painter.add(t.popup_elevation.as_shape(r, corner));
        painter.rect_filled(r, corner, t.badge_bg);
        let grid = ERect::from_min_size(r.min, Vec2::splat(side));
        for (i, px) in loupe.pixels.iter().enumerate() {
            let (x, y) = ((i % cells) as f32, (i / cells) as f32);
            let c = ERect::from_min_size(grid.min + vec2(x * cell, y * cell), Vec2::splat(cell));
            painter.rect_filled(
                c,
                0.0,
                Color32::from_rgba_unmultiplied(px[0], px[1], px[2], px[3]),
            );
        }
        if cell >= 4.0 {
            let s = stroke(1.0, t.loupe_grid);
            for i in 1..cells {
                let o = i as f32 * cell;
                painter.vline(grid.min.x + o, grid.y_range(), s);
                painter.hline(grid.x_range(), grid.min.y + o, s);
            }
        }
        let mid = (cells / 2) as f32 * cell;
        let center = ERect::from_min_size(grid.min + vec2(mid, mid), Vec2::splat(cell));
        painter.rect_stroke(
            center,
            0.0,
            stroke(1.5, t.loupe_reticle),
            StrokeKind::Outside,
        );
        painter.rect_stroke(r, corner, stroke(1.0, t.loupe_border), StrokeKind::Inside);

        let info = ERect::from_min_max(pos2(r.min.x, grid.max.y), r.max).shrink(t.space_sm);
        let font = FontId::monospace(11.0);
        painter.text(
            info.left_center(),
            Align2::LEFT_CENTER,
            format!("({}, {})", at[0], at[1]),
            font.clone(),
            t.badge_text,
        );
        if let Some(px) = loupe.center {
            let code = color_code(px, self.prefs.color_code);
            let text_r = painter.text(
                info.right_center(),
                Align2::RIGHT_CENTER,
                code,
                font,
                t.badge_text,
            );
            let sw = ERect::from_center_size(
                pos2(text_r.min.x - t.space_md, info.center().y),
                Vec2::splat(10.0),
            );
            painter.rect_filled(sw, 2.0, Color32::from_rgb(px[0], px[1], px[2]));
            painter.rect_stroke(sw, 2.0, stroke(1.0, t.loupe_border), StrokeKind::Outside);
        }
    }

    fn action_bar(
        &self,
        ctx: &Context,
        screen: ERect,
        sel_screen: ERect,
        rect: Rect,
        source: SelectionSource,
    ) -> Option<BarChoice> {
        let t = self.tokens;
        let id = egui::Id::new("crosshair.action_bar");
        // Measured last frame; a fair guess on the first.
        let size = ctx
            .data(|d| d.get_temp::<Vec2>(id))
            .unwrap_or(vec2(600.0, 40.0));
        let at = action_bar_rect(sel_screen, size, screen, t.space_md).min;
        let mut choice = None;
        let area = egui::Area::new(id)
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(t.panel)
                    .stroke(stroke(1.0, t.card_border))
                    .corner_radius(Tokens::corner(t.radius_pill))
                    .shadow(t.popup_elevation)
                    .inner_margin(egui::Margin::symmetric(t.space_md as i8, t.space_sm as i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let capture = egui::Button::new(
                                egui::RichText::new("📷 Capture Image")
                                    .color(t.accent_capture_text)
                                    .strong(),
                            )
                            .fill(t.accent_capture)
                            .corner_radius(Tokens::corner(t.radius_pill));
                            if ui.add(capture).on_hover_text("Enter").clicked() {
                                choice = Some(BarChoice::Done(OverlayAction::CaptureImage {
                                    rect,
                                    source: source.clone(),
                                }));
                            }
                            if ui.button("🎥 Record Video").clicked() {
                                choice = Some(BarChoice::Done(OverlayAction::RecordVideo {
                                    rect,
                                    source: source.clone(),
                                }));
                            }
                            if ui
                                .button("⇕ Panorama")
                                .on_hover_text("Scrolling capture")
                                .clicked()
                            {
                                choice = Some(BarChoice::Done(OverlayAction::Panorama {
                                    rect,
                                    source: source.clone(),
                                }));
                            }
                            ui.separator();
                            if ui.button("↺ Redo Selection").clicked() {
                                choice = Some(BarChoice::Redo);
                            }
                            if ui.button("✖ Cancel (Esc)").clicked() {
                                choice = Some(BarChoice::Done(OverlayAction::Cancel));
                            }
                        });
                    });
            });
        ctx.data_mut(|d| d.insert_temp(id, area.response.rect.size()));
        choice
    }
}

enum BarChoice {
    Done(OverlayAction),
    Redo,
}

/// Each display's global rect and frame pixels per desktop unit.
pub fn display_scales(desktop: &FrozenDesktop) -> Vec<(Rect, f32)> {
    desktop
        .displays()
        .iter()
        .map(|d| {
            let k = d.frame.width() as f32 / d.info.rect.width().max(1) as f32;
            (d.info.rect, k)
        })
        .collect()
}

/// Dims `screen` except `hole` (all of it without one).
fn draw_scrim(painter: &egui::Painter, screen: ERect, hole: Option<ERect>, color: Color32) {
    let Some(h) = hole.filter(|h| h.is_positive()) else {
        painter.rect_filled(screen, 0.0, color);
        return;
    };
    let bands = [
        ERect::from_min_max(screen.min, pos2(screen.max.x, h.min.y)),
        ERect::from_min_max(pos2(screen.min.x, h.max.y), screen.max),
        ERect::from_min_max(pos2(screen.min.x, h.min.y), pos2(h.min.x, h.max.y)),
        ERect::from_min_max(pos2(h.max.x, h.min.y), pos2(screen.max.x, h.max.y)),
    ];
    for b in bands.into_iter().filter(|b| b.is_positive()) {
        painter.rect_filled(b, 0.0, color);
    }
}

fn badge(painter: &egui::Painter, at: Pos2, align: Align2, text: &str, t: &Tokens) {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(12.0), t.badge_text);
    let pad = vec2(t.space_md, t.space_sm);
    let rect = align.anchor_size(at, galley.size() + 2.0 * pad);
    painter.rect_filled(rect, Tokens::corner(t.radius_pill), t.badge_bg);
    painter.galley(rect.min + pad, galley, t.badge_text);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn target(rect: Rect) -> Option<HoverTarget> {
        Some(HoverTarget {
            rect,
            source: SelectionSource::Window {
                id: 7,
                element: false,
            },
            label: "Files — Home".into(),
        })
    }

    #[test]
    fn mapping_round_trips_on_hidpi_and_negative_desktops() {
        let m = DesktopMapping::new(Rect::new(-1920, 0, 2560, 1440), false, 2.0);
        assert_eq!(m.to_screen([-1920, 0]), Pos2::ZERO);
        assert_eq!(m.to_screen([0, 100]), pos2(960.0, 50.0));
        for p in [[-1920, 0], [0, 0], [17, 999], [2559, 1439]] {
            let s = m.to_screen(p) + vec2(0.25, 0.25);
            assert_eq!(m.to_global(s), p);
        }
        let m = DesktopMapping::new(Rect::new(0, 0, 10, 10), false, f32::NAN);
        assert_eq!(m.points_per_unit, 1.0);
        assert_eq!(m.to_global(pos2(f32::INFINITY, 3.5))[1], 3);
    }

    #[test]
    fn fractional_scales_put_the_crosshair_on_the_reported_pixel() {
        let base = DesktopMapping::new(Rect::new(-1366, -200, 3840, 2160), false, 1.0);
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            let m = base.with_pixels_per_point(ppp);
            for p in [[-1366, -200], [0, 0], [1, 1], [997, 13], [3839, 2159]] {
                assert_eq!(m.to_global(m.to_screen(p)), p, "corner {p:?} @ {ppp}");
                assert_eq!(
                    m.to_global(m.center_to_screen(p)),
                    p,
                    "centre {p:?} @ {ppp}"
                );
            }
        }
        // macOS reports points: the scale does not change the mapping.
        let mac = DesktopMapping::new(Rect::new(0, 0, 1440, 900), true, 2.0);
        assert_eq!(mac.to_screen([100, 50]), pos2(100.0, 50.0));
        assert_eq!(mac.with_pixels_per_point(1.0), mac);
    }

    #[test]
    fn click_locks_the_hover_target_and_enter_captures_it() {
        let mut s = CrosshairOverlayState::default();
        let win = Rect::new(100, 100, 500, 400);
        assert!(
            step(&mut s, OverlayEvent::Hover([120, 130]), true, |_| target(
                win
            ))
            .is_none()
        );
        assert_eq!(s.hover.as_ref().unwrap().rect, win);
        step(&mut s, OverlayEvent::Click([120, 130]), true, |_| None);
        assert_eq!(s.phase.selection(), Some(win));
        // Hovering elsewhere no longer retargets.
        step(&mut s, OverlayEvent::Hover([0, 0]), true, |_| {
            panic!("no hit test")
        });
        let a = step(&mut s, OverlayEvent::Confirm, true, |_| None).unwrap();
        assert_eq!(
            a,
            OverlayAction::CaptureImage {
                rect: win,
                source: SelectionSource::Window {
                    id: 7,
                    element: false
                }
            }
        );
    }

    #[test]
    fn drag_selects_a_region_and_tiny_drags_are_dropped() {
        let mut s = CrosshairOverlayState::default();
        step(&mut s, OverlayEvent::DragStart([50, 60]), true, |_| None);
        step(&mut s, OverlayEvent::DragTo([10, 20]), true, |_| None);
        assert_eq!(s.phase.selection(), Some(Rect::new(10, 20, 51, 61)));
        step(&mut s, OverlayEvent::DragEnd([1329, 739]), true, |_| None);
        let OverlayPhase::Selected { rect, source } = &s.phase else {
            panic!()
        };
        assert_eq!((rect.width(), rect.height()), (1280, 680));
        assert_eq!(*source, SelectionSource::Region);
        step(&mut s, OverlayEvent::Redo, true, |_| None);
        assert_eq!(s.phase, OverlayPhase::Hovering);
        step(&mut s, OverlayEvent::DragStart([5, 5]), true, |_| None);
        step(&mut s, OverlayEvent::DragEnd([6, 9]), true, |_| None);
        assert_eq!(s.phase, OverlayPhase::Hovering);
        assert_eq!(
            step(&mut s, OverlayEvent::Cancel, true, |_| None),
            Some(OverlayAction::Cancel)
        );
    }

    #[test]
    fn snapping_off_never_hit_tests() {
        let mut s = CrosshairOverlayState::default();
        step(&mut s, OverlayEvent::Hover([1, 1]), false, |_| {
            panic!("no hit test")
        });
        assert!(s.hover.is_none());
        // A click with nothing hovered selects nothing.
        step(&mut s, OverlayEvent::Click([1, 1]), false, |_| None);
        assert_eq!(s.phase, OverlayPhase::Hovering);
    }

    #[test]
    fn loupe_flips_away_from_edges() {
        let screen = ERect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
        let size = vec2(150.0, 172.0);
        let r = loupe_rect(pos2(100.0, 100.0), size, screen, 20.0);
        assert_eq!(r.min, pos2(120.0, 120.0));
        let r = loupe_rect(pos2(990.0, 790.0), size, screen, 20.0);
        assert_eq!(r.max, pos2(970.0, 770.0));
        let r = loupe_rect(pos2(990.0, 10.0), size, screen, 20.0);
        assert_eq!((r.max.x, r.min.y), (970.0, 30.0));
        // A loupe larger than the gap on both sides still stays on screen.
        let tiny = ERect::from_min_size(Pos2::ZERO, vec2(160.0, 180.0));
        let r = loupe_rect(pos2(80.0, 90.0), size, tiny, 20.0);
        assert!(tiny.contains_rect(r));
    }

    #[test]
    fn action_bar_goes_below_then_above_then_inside() {
        let screen = ERect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
        let bar = vec2(400.0, 40.0);
        let sel = ERect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 300.0));
        let r = action_bar_rect(sel, bar, screen, 8.0);
        assert_eq!(r.min, pos2(0.0, 308.0), "centred, clamped to the left edge");
        let low = ERect::from_min_max(pos2(400.0, 500.0), pos2(600.0, 790.0));
        assert_eq!(action_bar_rect(low, bar, screen, 8.0).max.y, 492.0);
        let full = screen;
        let r = action_bar_rect(full, bar, screen, 8.0);
        assert_eq!(r.max.y, 792.0);
    }

    #[test]
    fn hover_targets_name_what_they_capture() {
        use capturecraft_capture::{UiRegion, UiRole, WindowRef};
        let hit = HitTarget {
            kind: HitKind::Element,
            rect: Rect::new(0, 0, 10, 10),
            window: Some(WindowRef {
                id: 3,
                app_name: "Firefox".into(),
                title: "Docs".into(),
                ..WindowRef::default()
            }),
            element: Some(UiRegion {
                role: UiRole::Button,
                name: "Save".into(),
                ..UiRegion::default()
            }),
            display_id: Some(1),
        };
        let h = hover_target(&hit).unwrap();
        assert_eq!(h.label, "Button “Save” · Firefox — Docs");
        assert_eq!(
            h.source,
            SelectionSource::Window {
                id: 3,
                element: true
            }
        );
        assert!(hover_target(&HitTarget::default()).is_none());
        assert_eq!(color_code([0xAB, 0x01, 0xFF, 9], ColorCode::Hex), "#AB01FF");
        assert_eq!(color_code([1, 2, 3, 4], ColorCode::Rgb), "rgb(1, 2, 3)");
    }
}
