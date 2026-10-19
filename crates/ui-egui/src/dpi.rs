//! Physical pixels versus egui points.
//!
//! Captures, selections and the `W × H px` badges count **physical
//! pixels** ([`PhysicalPx`]): what the frozen desktop frames hold. egui
//! lays out in **logical points** ([`LogicalPt`]), `physical /
//! pixels_per_point`. Keeping the two apart as types means every crossing
//! goes through one conversion, so a 125 % or 150 % monitor (Windows, X11)
//! cannot drift the crosshair off the pixel it reports.
//!
//! A conversion back from points snaps to the pixel *containing* the
//! point, with a small tolerance so the top-left corner of pixel `p`
//! (`p / ppp` points, inexact in binary at fractional scales) maps back to
//! `p` rather than `p - 1`.

use egui::{Pos2, pos2};
use photocraft_geom::Rect;

/// A position in physical pixels (global desktop pixels on Windows and
/// X11, frame pixels everywhere).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PhysicalPx {
    pub x: i32,
    pub y: i32,
}

/// A position in egui logical points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalPt {
    pub x: f32,
    pub y: f32,
}

/// Snapping tolerance of [`LogicalPt::to_physical`], in physical pixels.
const SNAP_EPS: f32 = 1e-3;

/// `ppp` if usable, else 1.
pub fn sane_ppp(ppp: f32) -> f32 {
    if ppp.is_finite() && ppp > 0.0 {
        ppp
    } else {
        1.0
    }
}

impl PhysicalPx {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub fn from_array(p: [i32; 2]) -> Self {
        Self::new(p[0], p[1])
    }

    pub fn to_array(self) -> [i32; 2] {
        [self.x, self.y]
    }

    /// Top-left corner of the pixel, in points.
    pub fn to_logical(self, ppp: f32) -> LogicalPt {
        let s = sane_ppp(ppp);
        LogicalPt::new(self.x as f32 / s, self.y as f32 / s)
    }

    /// Centre of the pixel, in points (where a crosshair through it goes).
    pub fn center_to_logical(self, ppp: f32) -> LogicalPt {
        let s = sane_ppp(ppp);
        LogicalPt::new((self.x as f32 + 0.5) / s, (self.y as f32 + 0.5) / s)
    }
}

impl LogicalPt {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn from_pos2(p: Pos2) -> Self {
        Self::new(p.x, p.y)
    }

    pub fn to_pos2(self) -> Pos2 {
        pos2(self.x, self.y)
    }

    /// The physical pixel containing this point (non-finite → 0).
    pub fn to_physical(self, ppp: f32) -> PhysicalPx {
        let s = sane_ppp(ppp);
        let f = |v: f32| {
            let g = (v * s + SNAP_EPS).floor();
            if g.is_finite() {
                g.clamp(i32::MIN as f32, i32::MAX as f32) as i32
            } else {
                0
            }
        };
        PhysicalPx::new(f(self.x), f(self.y))
    }
}

/// The physical pixel containing the egui position `pt`.
pub fn to_physical(pt: Pos2, ppp: f32) -> PhysicalPx {
    LogicalPt::from_pos2(pt).to_physical(ppp)
}

/// The top-left corner of physical pixel `px` as an egui position.
pub fn to_logical(px: PhysicalPx, ppp: f32) -> Pos2 {
    px.to_logical(ppp).to_pos2()
}

/// `v` points moved to the nearest physical pixel boundary, so a shape
/// drawn there is not smeared over two hardware pixels.
pub fn snap_to_pixel(v: f32, ppp: f32) -> f32 {
    let s = sane_ppp(ppp);
    let g = (v * s).round() / s;
    if g.is_finite() { g } else { 0.0 }
}

/// Edge of one loupe cell in points: `zoom` points rounded to whole
/// physical pixels (at least one), so every cell covers the same number
/// of hardware pixels at 125 % or 150 %.
pub fn loupe_cell_pt(zoom: u8, ppp: f32) -> f32 {
    let s = sane_ppp(ppp);
    (f32::from(zoom.max(1)) * s).round().max(1.0) / s
}

/// `W × H px` of a selection, physical pixels.
pub fn dimension_badge(w: u32, h: u32) -> String {
    format!("{w} × {h} px")
}

/// Physical size of a desktop-unit rectangle, sampled from the frozen
/// display it is (mostly) on. `displays` lists each display's global rect
/// and its frame pixels per desktop unit (1 where the desktop is reported
/// in physical pixels, the scale factor where it is reported in points).
pub fn physical_size(rect: Rect, displays: &[(Rect, f32)]) -> (u32, u32) {
    let k = displays
        .iter()
        .map(|(r, k)| (r.intersect(&rect).area_u64(), *k))
        .filter(|(a, _)| *a > 0)
        .max_by_key(|(a, _)| *a)
        .map_or(1.0, |(_, k)| sane_ppp(k));
    let scale = |v: u32| {
        (f64::from(v) * f64::from(k))
            .round()
            .min(f64::from(u32::MAX)) as u32
    };
    (scale(rect.width()), scale(rect.height()))
}

trait AreaU64 {
    fn area_u64(&self) -> u64;
}

impl AreaU64 for Rect {
    fn area_u64(&self) -> u64 {
        if self.is_empty() {
            0
        } else {
            u64::from(self.width()) * u64::from(self.height())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALES: [f32; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

    #[test]
    fn physical_to_logical_to_physical_never_drifts() {
        for ppp in SCALES {
            for x in (-4000..4000).step_by(7) {
                let p = PhysicalPx::new(x, x / 3);
                assert_eq!(p.to_logical(ppp).to_physical(ppp), p, "corner {x} @ {ppp}");
                assert_eq!(
                    p.center_to_logical(ppp).to_physical(ppp),
                    p,
                    "centre {x} @ {ppp}"
                );
            }
        }
    }

    #[test]
    fn points_scale_by_pixels_per_point() {
        assert_eq!(
            PhysicalPx::new(300, 150).to_logical(1.5),
            LogicalPt::new(200.0, 100.0)
        );
        assert_eq!(
            LogicalPt::new(10.2, 10.9).to_physical(2.0),
            PhysicalPx::new(20, 21)
        );
        assert_eq!(
            LogicalPt::new(f32::NAN, 3.0).to_physical(f32::INFINITY),
            PhysicalPx::new(0, 3)
        );
    }

    #[test]
    fn free_conversions_match_the_typed_ones() {
        for ppp in SCALES {
            let p = PhysicalPx::new(-37, 1201);
            assert_eq!(to_physical(to_logical(p, ppp), ppp), p, "@ {ppp}");
        }
        assert_eq!(
            to_logical(PhysicalPx::new(300, 150), 1.5),
            pos2(200.0, 100.0)
        );
        assert_eq!(to_physical(pos2(10.2, 10.9), 2.0), PhysicalPx::new(20, 21));
    }

    #[test]
    fn loupe_cells_cover_whole_hardware_pixels() {
        for ppp in SCALES {
            for zoom in [1u8, 7, 10, 12] {
                let cell = loupe_cell_pt(zoom, ppp) * ppp;
                assert!((cell - cell.round()).abs() < 1e-4, "{zoom} @ {ppp}: {cell}");
                assert!(cell >= 1.0);
            }
            let v = snap_to_pixel(10.3, ppp) * ppp;
            assert!((v - v.round()).abs() < 1e-4, "{ppp}");
        }
        // 10 points at 125 % = 12.5 px → 13 px cells.
        assert_eq!(loupe_cell_pt(10, 1.25), 13.0 / 1.25);
        assert_eq!(snap_to_pixel(f32::NAN, 1.0), 0.0);
    }

    #[test]
    fn badges_count_physical_pixels_at_every_scale() {
        // Windows/X11: desktop units are physical pixels; the frames match.
        let sel = Rect::new(100, 100, 400, 300);
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            let display = [(Rect::new(0, 0, 2560, 1440), 1.0)];
            let (w, h) = physical_size(sel, &display);
            assert_eq!(dimension_badge(w, h), "300 × 200 px", "@ {ppp}");
            // The badge sits at the selection corner in points.
            let at = PhysicalPx::new(sel.x1, sel.y1).to_logical(ppp);
            assert_eq!(at.to_physical(ppp), PhysicalPx::new(400, 300));
        }
        // macOS: desktop units are points; frames hold `scale` pixels each.
        for (scale, want) in [
            (1.0, "300 × 200 px"),
            (1.25, "375 × 250 px"),
            (1.5, "450 × 300 px"),
            (2.0, "600 × 400 px"),
        ] {
            let display = [(Rect::new(0, 0, 1440, 900), scale)];
            let (w, h) = physical_size(sel, &display);
            assert_eq!(dimension_badge(w, h), want);
        }
    }

    #[test]
    fn mixed_monitors_use_the_display_holding_most_of_the_selection() {
        let displays = [
            (Rect::new(0, 0, 1000, 1000), 1.0),
            (Rect::new(1000, 0, 2000, 1000), 2.0),
        ];
        assert_eq!(
            physical_size(Rect::new(900, 0, 1300, 100), &displays),
            (800, 200)
        );
        assert_eq!(
            physical_size(Rect::new(5000, 0, 5010, 10), &displays),
            (10, 10)
        );
    }
}
