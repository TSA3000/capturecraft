//! Vector icons for the editor chrome.
//!
//! Every icon is drawn from anti-aliased `egui` primitives (segments, paths,
//! circles, rects and Bézier curves); there are no fonts, glyphs or bitmaps.
//! Icons are designed on a 20×20 unit grid with the origin at the top left.
//! [`shapes`] maps that grid onto the target rect (square fit, centred, aspect
//! kept) and scales the stroke with it, so the same icon works at 12 px in a
//! status bar and at 48 px on a tile. Colours come from the caller through
//! [`IconStyle`], which keeps the icons theme-agnostic.

use egui::epaint::{CubicBezierShape, QuadraticBezierShape};
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, vec2};

/// Every icon the UI can paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    // main toolbar
    Library,
    Assets,
    Capture,
    Create,
    Favorites,
    Arrow,
    Text,
    Callout,
    Shape,
    Stamp,
    Fill,
    Move,
    Selection,
    More,
    Undo,
    Redo,
    CopyAll,
    Share,
    Effects,
    Properties,
    // selection modes (Tool Properties tiles)
    RectangleSelect,
    EllipseSelect,
    PolygonSelect,
    LassoSelect,
    // tools behind More
    Step,
    Blur,
    Pixelate,
    Highlight,
    Pen,
    Eraser,
    Magnify,
    Spotlight,
    CutOut,
    SmartMove,
    Simplify,
    Crop,
    // utility bar / misc chrome
    Help,
    Recent,
    Tag,
    Zoom,
    Dimensions,
    Close,
    ChevronDown,
    Pin,
    Check,
    // effects tiles
    Border,
    DropShadow,
    Frame,
    TornEdge,
    Perspective,
    ColorFilter,
}

impl Icon {
    /// Every variant, in declaration order.
    pub const ALL: [Icon; 51] = [
        Icon::Library,
        Icon::Assets,
        Icon::Capture,
        Icon::Create,
        Icon::Favorites,
        Icon::Arrow,
        Icon::Text,
        Icon::Callout,
        Icon::Shape,
        Icon::Stamp,
        Icon::Fill,
        Icon::Move,
        Icon::Selection,
        Icon::More,
        Icon::Undo,
        Icon::Redo,
        Icon::CopyAll,
        Icon::Share,
        Icon::Effects,
        Icon::Properties,
        Icon::RectangleSelect,
        Icon::EllipseSelect,
        Icon::PolygonSelect,
        Icon::LassoSelect,
        Icon::Step,
        Icon::Blur,
        Icon::Pixelate,
        Icon::Highlight,
        Icon::Pen,
        Icon::Eraser,
        Icon::Magnify,
        Icon::Spotlight,
        Icon::CutOut,
        Icon::SmartMove,
        Icon::Simplify,
        Icon::Crop,
        Icon::Help,
        Icon::Recent,
        Icon::Tag,
        Icon::Zoom,
        Icon::Dimensions,
        Icon::Close,
        Icon::ChevronDown,
        Icon::Pin,
        Icon::Check,
        Icon::Border,
        Icon::DropShadow,
        Icon::Frame,
        Icon::TornEdge,
        Icon::Perspective,
        Icon::ColorFilter,
    ];

    /// Human name, e.g. "Rectangle Select".
    pub fn name(self) -> &'static str {
        match self {
            Icon::Library => "Library",
            Icon::Assets => "Assets",
            Icon::Capture => "Capture",
            Icon::Create => "Create",
            Icon::Favorites => "Favorites",
            Icon::Arrow => "Arrow",
            Icon::Text => "Text",
            Icon::Callout => "Callout",
            Icon::Shape => "Shape",
            Icon::Stamp => "Stamp",
            Icon::Fill => "Fill",
            Icon::Move => "Move",
            Icon::Selection => "Selection",
            Icon::More => "More",
            Icon::Undo => "Undo",
            Icon::Redo => "Redo",
            Icon::CopyAll => "Copy All",
            Icon::Share => "Share",
            Icon::Effects => "Effects",
            Icon::Properties => "Properties",
            Icon::RectangleSelect => "Rectangle Select",
            Icon::EllipseSelect => "Ellipse Select",
            Icon::PolygonSelect => "Polygon Select",
            Icon::LassoSelect => "Lasso Select",
            Icon::Step => "Step",
            Icon::Blur => "Blur",
            Icon::Pixelate => "Pixelate",
            Icon::Highlight => "Highlight",
            Icon::Pen => "Pen",
            Icon::Eraser => "Eraser",
            Icon::Magnify => "Magnify",
            Icon::Spotlight => "Spotlight",
            Icon::CutOut => "Cut Out",
            Icon::SmartMove => "Smart Move",
            Icon::Simplify => "Simplify",
            Icon::Crop => "Crop",
            Icon::Help => "Help",
            Icon::Recent => "Recent",
            Icon::Tag => "Tag",
            Icon::Zoom => "Zoom",
            Icon::Dimensions => "Dimensions",
            Icon::Close => "Close",
            Icon::ChevronDown => "Chevron Down",
            Icon::Pin => "Pin",
            Icon::Check => "Check",
            Icon::Border => "Border",
            Icon::DropShadow => "Drop Shadow",
            Icon::Frame => "Frame",
            Icon::TornEdge => "Torn Edge",
            Icon::Perspective => "Perspective",
            Icon::ColorFilter => "Color Filter",
        }
    }
}

/// Colours an icon is drawn with: `fg` for strokes/fills, `accent` for the
/// one coloured part some icons have (the Capture record dot, the Fill
/// bucket's drip, Highlight's marker band).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconStyle {
    pub fg: Color32,
    pub accent: Color32,
}

/// The icon's shapes, designed on a 20×20 grid and scaled/centred to fit `rect`
/// (square fit, aspect kept).
pub fn shapes(icon: Icon, rect: Rect, style: IconStyle) -> Vec<Shape> {
    let mut pen = GridPen::new(rect, style, matches!(icon, Icon::Redo));
    draw(&mut pen, icon);
    pen.out
}

/// Paints `shapes(icon, rect, style)`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, style: IconStyle) {
    painter.extend(shapes(icon, rect, style));
}

/// A point on the 20×20 design grid.
type G = (f32, f32);

/// Grid units of the regular stroke.
const STROKE_UNITS: f32 = 1.5;

/// Collects shapes while mapping grid coordinates into the target rect.
struct GridPen {
    origin: Pos2,
    scale: f32,
    /// Mirror horizontally (x -> 20 - x); used to derive Redo from Undo.
    mirror: bool,
    fg: Color32,
    accent: Color32,
    out: Vec<Shape>,
}

impl GridPen {
    fn new(rect: Rect, style: IconStyle, mirror: bool) -> Self {
        let scale = (rect.width().min(rect.height()) / 20.0).max(0.0);
        Self {
            origin: rect.center() - vec2(10.0, 10.0) * scale,
            scale,
            mirror,
            fg: style.fg,
            accent: style.accent,
            out: Vec::new(),
        }
    }

    fn pt(&self, (x, y): G) -> Pos2 {
        let x = if self.mirror { 20.0 - x } else { x };
        self.origin + vec2(x, y) * self.scale
    }

    fn pts(&self, pts: &[G]) -> Vec<Pos2> {
        pts.iter().map(|&p| self.pt(p)).collect()
    }

    /// Stroke `k` times the regular width, never thinner than one point.
    fn stroke_k(&self, k: f32, color: Color32) -> Stroke {
        Stroke::new((STROKE_UNITS * k * self.scale).max(1.0), color)
    }

    fn stroke(&self) -> Stroke {
        self.stroke_k(1.0, self.fg)
    }

    fn line(&mut self, pts: &[G]) {
        let s = self.stroke();
        self.line_with(pts, s);
    }

    fn line_with(&mut self, pts: &[G], stroke: Stroke) {
        let p = self.pts(pts);
        self.out.push(Shape::line(p, stroke));
    }

    fn seg(&mut self, a: G, b: G) {
        let s = self.stroke();
        self.seg_with(a, b, s);
    }

    fn seg_with(&mut self, a: G, b: G, stroke: Stroke) {
        self.out
            .push(Shape::line_segment([self.pt(a), self.pt(b)], stroke));
    }

    fn closed(&mut self, pts: &[G]) {
        let p = self.pts(pts);
        let s = self.stroke();
        self.out.push(Shape::closed_line(p, s));
    }

    /// Filled convex polygon without outline.
    fn fill(&mut self, pts: &[G], color: Color32) {
        let p = self.pts(pts);
        self.out.push(Shape::convex_polygon(p, color, Stroke::NONE));
    }

    /// Filled convex polygon with the regular outline (rounds off small fills).
    fn fill_outlined(&mut self, pts: &[G], color: Color32) {
        let p = self.pts(pts);
        let s = self.stroke_k(0.6, color);
        self.out.push(Shape::convex_polygon(p, color, s));
    }

    fn ring(&mut self, c: G, r: f32) {
        let s = self.stroke();
        self.ring_with(c, r, s);
    }

    fn ring_with(&mut self, c: G, r: f32, stroke: Stroke) {
        self.out
            .push(Shape::circle_stroke(self.pt(c), r * self.scale, stroke));
    }

    fn disc(&mut self, c: G, r: f32, color: Color32) {
        self.out
            .push(Shape::circle_filled(self.pt(c), r * self.scale, color));
    }

    fn grid_rect(&self, a: G, b: G) -> Rect {
        Rect::from_two_pos(self.pt(a), self.pt(b))
    }

    fn rect(&mut self, a: G, b: G, radius: f32) {
        let r = self.grid_rect(a, b);
        let s = self.stroke();
        self.out.push(Shape::rect_stroke(
            r,
            radius * self.scale,
            s,
            StrokeKind::Middle,
        ));
    }

    fn rect_fill(&mut self, a: G, b: G, radius: f32, color: Color32) {
        let r = self.grid_rect(a, b);
        self.out
            .push(Shape::rect_filled(r, radius * self.scale, color));
    }

    /// Dashed polyline; `closed` joins the last point back to the first.
    fn dashed(&mut self, pts: &[G], closed: bool) {
        let mut p = self.pts(pts);
        if closed && let Some(&first) = p.first() {
            p.push(first);
        }
        let s = self.stroke_k(0.85, self.fg);
        let dash = (2.0 * self.scale).max(1.5);
        let gap = (1.5 * self.scale).max(1.0);
        self.out.extend(Shape::dashed_line(&p, s, dash, gap));
    }

    fn cubic(&mut self, p: [G; 4]) {
        let points = [self.pt(p[0]), self.pt(p[1]), self.pt(p[2]), self.pt(p[3])];
        let s = self.stroke();
        self.out
            .push(Shape::CubicBezier(CubicBezierShape::from_points_stroke(
                points,
                false,
                Color32::TRANSPARENT,
                s,
            )));
    }

    fn quad(&mut self, p: [G; 3]) {
        let points = [self.pt(p[0]), self.pt(p[1]), self.pt(p[2])];
        let s = self.stroke();
        self.out.push(Shape::QuadraticBezier(
            QuadraticBezierShape::from_points_stroke(points, false, Color32::TRANSPARENT, s),
        ));
    }

    /// Open arrow head (two strokes) at `tip`, pointing away from `from`.
    fn chevron(&mut self, tip: G, from: G, len: f32) {
        let pts = head_points(tip, from, len, len);
        self.line(&[pts[1], tip, pts[2]]);
    }

    /// Four-point sparkle built from four convex kites.
    fn sparkle(&mut self, c: G, outer: f32, inner: f32, color: Color32) {
        for k in 0..4 {
            let a = k as f32 * std::f32::consts::FRAC_PI_2;
            let tip = polar(c, outer, a);
            let s1 = polar(c, inner, a + std::f32::consts::FRAC_PI_4);
            let s2 = polar(c, inner, a - std::f32::consts::FRAC_PI_4);
            self.fill(&[tip, s1, c, s2], color);
        }
    }
}

/// Point at `r` units and angle `a` (radians, y down) from `c`.
fn polar(c: G, r: f32, a: f32) -> G {
    (c.0 + r * a.cos(), c.1 + r * a.sin())
}

/// Points along a circular arc, angles in degrees (0 = right, 90 = down).
fn arc(c: G, r: f32, from_deg: f32, to_deg: f32, steps: usize) -> Vec<G> {
    let steps = steps.max(1);
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            polar(c, r, (from_deg + (to_deg - from_deg) * t).to_radians())
        })
        .collect()
}

/// Closed ellipse outline.
fn ellipse(c: G, rx: f32, ry: f32, steps: usize) -> Vec<G> {
    (0..steps)
        .map(|i| {
            let a = i as f32 / steps as f32 * std::f32::consts::TAU;
            (c.0 + rx * a.cos(), c.1 + ry * a.sin())
        })
        .collect()
}

/// Regular polygon with `n` corners, the first one straight up.
fn regular(c: G, r: f32, n: usize) -> Vec<G> {
    (0..n)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 / n as f32 * std::f32::consts::TAU;
            polar(c, r, a)
        })
        .collect()
}

/// Triangle head: `[tip, left, right]`, `len` long and `width` wide at the base.
fn head_points(tip: G, from: G, len: f32, width: f32) -> [G; 3] {
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let d = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
    let (ux, uy) = (dx / d, dy / d);
    let base = (tip.0 - ux * len, tip.1 - uy * len);
    let (nx, ny) = (-uy * width * 0.5, ux * width * 0.5);
    [tip, (base.0 + nx, base.1 + ny), (base.0 - nx, base.1 - ny)]
}

/// Corners of a rectangle of `len` × `wid` around `c`, rotated by `deg`.
fn rot_rect(c: G, len: f32, wid: f32, deg: f32) -> [G; 4] {
    let a = deg.to_radians();
    let (ux, uy) = (a.cos() * len * 0.5, a.sin() * len * 0.5);
    let (nx, ny) = (-a.sin() * wid * 0.5, a.cos() * wid * 0.5);
    [
        (c.0 - ux - nx, c.1 - uy - ny),
        (c.0 + ux - nx, c.1 + uy - ny),
        (c.0 + ux + nx, c.1 + uy + ny),
        (c.0 - ux + nx, c.1 - uy + ny),
    ]
}

/// Point `t` units along `u` and `n` units along the normal from `a`.
fn along(a: G, u: G, t: f32, n: f32) -> G {
    (a.0 + u.0 * t - u.1 * n, a.1 + u.1 * t + u.0 * n)
}

fn draw(p: &mut GridPen, icon: Icon) {
    let fg = p.fg;
    let accent = p.accent;
    match icon {
        Icon::Library => {
            p.rect((3.0, 3.0), (9.0, 9.0), 1.5);
            p.rect((11.0, 3.0), (17.0, 9.0), 1.5);
            p.rect((3.0, 11.0), (9.0, 17.0), 1.5);
            p.rect((11.0, 11.0), (17.0, 17.0), 1.5);
        }
        Icon::Assets => {
            p.rect((7.0, 2.5), (13.0, 7.5), 1.0);
            p.seg((10.0, 7.5), (10.0, 10.0));
            p.line(&[(5.0, 12.5), (5.0, 10.0), (15.0, 10.0), (15.0, 12.5)]);
            p.rect((2.5, 12.5), (7.5, 17.5), 1.0);
            p.rect((12.5, 12.5), (17.5, 17.5), 1.0);
        }
        Icon::Capture => {
            p.ring((10.0, 10.0), 7.5);
            p.disc((10.0, 10.0), 4.5, accent);
        }
        Icon::Create => {
            p.closed(&[
                (4.0, 2.5),
                (12.0, 2.5),
                (16.0, 6.5),
                (16.0, 17.5),
                (4.0, 17.5),
            ]);
            p.line(&[(12.0, 2.5), (12.0, 6.5), (16.0, 6.5)]);
            p.seg((10.0, 9.5), (10.0, 14.5));
            p.seg((7.5, 12.0), (12.5, 12.0));
        }
        Icon::Favorites => {
            let c = (10.0, 10.8);
            let pts: Vec<G> = (0..10)
                .map(|i| {
                    let r = if i % 2 == 0 { 8.0 } else { 3.4 };
                    let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                    polar(c, r, a)
                })
                .collect();
            p.closed(&pts);
        }
        Icon::Arrow => {
            let tip = (16.5, 3.5);
            let tail = (3.5, 16.5);
            let head = head_points(tip, tail, 6.0, 6.4);
            let base = ((head[1].0 + head[2].0) * 0.5, (head[1].1 + head[2].1) * 0.5);
            p.seg(tail, base);
            p.fill_outlined(&head, fg);
        }
        Icon::Text => {
            p.line(&[(4.0, 6.5), (4.0, 3.5), (16.0, 3.5), (16.0, 6.5)]);
            p.seg((10.0, 3.5), (10.0, 16.5));
            p.seg((7.0, 16.5), (13.0, 16.5));
        }
        Icon::Callout => {
            let mut pts = arc((5.5, 6.0), 3.0, 180.0, 270.0, 5);
            pts.extend(arc((14.5, 6.0), 3.0, 270.0, 360.0, 5));
            pts.extend(arc((14.5, 10.5), 3.0, 0.0, 90.0, 5));
            pts.extend([(11.0, 13.5), (6.0, 17.5), (7.0, 13.5)]);
            pts.extend(arc((5.5, 10.5), 3.0, 90.0, 180.0, 5));
            p.closed(&pts);
        }
        Icon::Shape => {
            p.rect((2.5, 2.5), (12.0, 12.0), 1.0);
            p.ring((12.5, 12.5), 5.0);
        }
        Icon::Stamp => {
            p.ring((10.0, 4.5), 2.5);
            p.seg((8.6, 6.8), (8.0, 10.5));
            p.seg((11.4, 6.8), (12.0, 10.5));
            p.rect((3.0, 10.5), (17.0, 14.5), 1.5);
            p.seg((4.5, 17.0), (15.5, 17.0));
        }
        Icon::Fill => {
            p.fill(
                &[(2.5, 10.0), (15.5, 10.0), (9.0, 16.5)],
                fg.gamma_multiply(0.45),
            );
            p.closed(&[(9.0, 3.5), (15.5, 10.0), (9.0, 16.5), (2.5, 10.0)]);
            p.quad([(5.75, 6.75), (5.0, 1.5), (9.0, 3.5)]);
            p.fill(&[(16.5, 11.0), (18.1, 14.6), (14.9, 14.6)], accent);
            p.disc((16.5, 15.0), 1.7, accent);
        }
        Icon::Move => {
            p.seg((10.0, 2.5), (10.0, 17.5));
            p.seg((2.5, 10.0), (17.5, 10.0));
            p.chevron((10.0, 2.5), (10.0, 10.0), 2.6);
            p.chevron((10.0, 17.5), (10.0, 10.0), 2.6);
            p.chevron((2.5, 10.0), (10.0, 10.0), 2.6);
            p.chevron((17.5, 10.0), (10.0, 10.0), 2.6);
        }
        Icon::Selection => {
            p.dashed(&[(3.0, 4.0), (17.0, 4.0), (17.0, 16.0), (3.0, 16.0)], true);
        }
        Icon::More => {
            for x in [4.5, 10.0, 15.5] {
                p.disc((x, 10.0), 1.6, fg);
            }
        }
        Icon::Undo | Icon::Redo => {
            // Drawn as Undo; the pen mirrors it for Redo.
            p.cubic([(16.0, 16.5), (16.5, 9.0), (12.5, 6.5), (4.5, 6.5)]);
            p.chevron((4.0, 6.5), (10.0, 6.5), 3.5);
        }
        Icon::CopyAll => {
            p.line(&[
                (7.0, 13.5),
                (3.0, 13.5),
                (3.0, 3.0),
                (13.5, 3.0),
                (13.5, 7.0),
            ]);
            p.rect((7.0, 7.0), (17.0, 17.0), 1.5);
        }
        Icon::Share => {
            p.line(&[
                (7.0, 8.0),
                (4.0, 8.0),
                (4.0, 17.0),
                (16.0, 17.0),
                (16.0, 8.0),
                (13.0, 8.0),
            ]);
            p.seg((10.0, 12.5), (10.0, 2.5));
            p.chevron((10.0, 2.5), (10.0, 12.5), 3.2);
        }
        Icon::Effects => {
            p.seg((3.0, 17.0), (12.0, 8.0));
            let thick = p.stroke_k(1.6, fg);
            p.seg_with((10.5, 9.5), (12.5, 7.5), thick);
            p.sparkle((14.5, 5.5), 3.6, 0.9, fg);
            p.sparkle((5.5, 5.0), 2.0, 0.6, fg);
            p.disc((16.0, 12.5), 0.9, fg);
        }
        Icon::Properties => {
            for (y, knob) in [(5.0, 12.5), (10.0, 7.0), (15.0, 14.0)] {
                p.seg((3.0, y), (17.0, y));
                p.disc((knob, y), 2.0, fg);
            }
        }
        Icon::RectangleSelect => {
            p.dashed(&[(3.0, 4.5), (17.0, 4.5), (17.0, 15.5), (3.0, 15.5)], true);
        }
        Icon::EllipseSelect => {
            p.dashed(&ellipse((10.0, 10.0), 7.5, 5.5, 40), true);
        }
        Icon::PolygonSelect => {
            p.dashed(&regular((10.0, 10.6), 7.5, 5), true);
        }
        Icon::LassoSelect => {
            p.dashed(&ellipse((10.5, 8.0), 7.0, 5.0, 36), true);
            p.quad([(5.6, 11.7), (4.0, 14.5), (6.5, 17.5)]);
        }
        Icon::Step => {
            p.ring((10.0, 10.0), 7.5);
            p.line(&[(8.0, 7.5), (10.5, 5.5), (10.5, 14.5)]);
            p.seg((8.0, 14.5), (13.0, 14.5));
        }
        Icon::Blur => {
            p.ring_with((10.0, 10.0), 7.5, p.stroke_k(1.0, fg.gamma_multiply(0.3)));
            p.ring_with((10.0, 10.0), 5.0, p.stroke_k(1.0, fg.gamma_multiply(0.65)));
            p.disc((10.0, 10.0), 2.6, fg);
        }
        Icon::Pixelate => {
            const ALPHA: [[f32; 3]; 3] = [[1.0, 0.35, 0.7], [0.35, 0.7, 0.2], [0.7, 0.2, 0.5]];
            let cell = 14.0 / 3.0;
            for (row, alphas) in ALPHA.iter().enumerate() {
                for (col, &a) in alphas.iter().enumerate() {
                    let x = 3.0 + col as f32 * cell;
                    let y = 3.0 + row as f32 * cell;
                    p.rect_fill(
                        (x + 0.3, y + 0.3),
                        (x + cell - 0.3, y + cell - 0.3),
                        0.6,
                        fg.gamma_multiply(a),
                    );
                }
            }
        }
        Icon::Highlight => {
            let u = (
                std::f32::consts::FRAC_1_SQRT_2,
                -std::f32::consts::FRAC_1_SQRT_2,
            );
            let a = (7.5, 12.5);
            let hw = 2.6;
            p.fill(
                &[
                    along(a, u, 0.6, -hw + 0.5),
                    along(a, u, 2.8, -hw + 0.5),
                    along(a, u, 2.8, hw - 0.5),
                    along(a, u, 0.6, hw - 0.5),
                ],
                accent,
            );
            p.closed(&[
                along(a, u, 0.0, -hw),
                along(a, u, 9.0, -hw),
                along(a, u, 9.0, hw),
                along(a, u, 0.0, hw),
            ]);
            p.closed(&[
                along(a, u, 0.0, -1.6),
                along(a, u, -3.0, -0.8),
                along(a, u, -3.0, 0.8),
                along(a, u, 0.0, 1.6),
            ]);
        }
        Icon::Pen => {
            let u = (
                std::f32::consts::FRAC_1_SQRT_2,
                -std::f32::consts::FRAC_1_SQRT_2,
            );
            let a = (6.0, 14.0);
            let hw = 2.2;
            p.closed(&[
                along(a, u, -4.5, 0.0),
                along(a, u, 0.0, -hw),
                along(a, u, 9.0, -hw),
                along(a, u, 9.0, hw),
                along(a, u, 0.0, hw),
            ]);
            p.seg(along(a, u, 0.0, -hw), along(a, u, 0.0, hw));
            p.seg(along(a, u, 6.8, -hw), along(a, u, 6.8, hw));
        }
        Icon::Eraser => {
            let c = (10.5, 9.0);
            let r = rot_rect(c, 13.0, 7.0, -45.0);
            // Rubber end (lower left third) filled softly.
            let t = 13.0 / 3.0 / 13.0;
            let m0 = (
                r[0].0 + (r[1].0 - r[0].0) * t,
                r[0].1 + (r[1].1 - r[0].1) * t,
            );
            let m3 = (
                r[3].0 + (r[2].0 - r[3].0) * t,
                r[3].1 + (r[2].1 - r[3].1) * t,
            );
            p.fill(&[r[0], m0, m3, r[3]], fg.gamma_multiply(0.45));
            p.closed(&r);
            p.seg(m0, m3);
            p.seg((9.0, 17.5), (17.0, 17.5));
        }
        Icon::Magnify | Icon::Zoom => {
            p.ring((8.5, 8.5), 5.5);
            let thick = p.stroke_k(1.6, fg);
            p.seg_with((12.6, 12.6), (16.8, 16.8), thick);
            if icon == Icon::Zoom {
                p.seg((8.5, 6.0), (8.5, 11.0));
                p.seg((6.0, 8.5), (11.0, 8.5));
            }
        }
        Icon::Spotlight => {
            p.ring((10.0, 10.0), 4.0);
            for k in 0..8 {
                let a = (k as f32 * 45.0).to_radians();
                p.seg(polar((10.0, 10.0), 6.2, a), polar((10.0, 10.0), 8.3, a));
            }
        }
        Icon::CutOut => {
            p.ring((5.5, 14.5), 2.6);
            p.ring((14.5, 14.5), 2.6);
            p.seg((7.2, 12.5), (14.5, 2.5));
            p.seg((12.8, 12.5), (5.5, 2.5));
        }
        Icon::SmartMove => {
            p.rect((3.0, 7.0), (13.0, 17.0), 1.5);
            p.sparkle((15.0, 5.0), 3.6, 0.9, fg);
            p.sparkle((16.5, 12.5), 1.8, 0.5, fg);
        }
        Icon::Simplify => {
            p.rect((2.5, 3.5), (17.5, 16.5), 2.0);
            p.seg((5.5, 7.0), (14.5, 7.0));
            p.seg((5.5, 10.0), (12.0, 10.0));
            p.seg((5.5, 13.0), (9.5, 13.0));
        }
        Icon::Crop => {
            p.line(&[(5.5, 2.5), (5.5, 14.5), (17.5, 14.5)]);
            p.line(&[(2.5, 5.5), (14.5, 5.5), (14.5, 17.5)]);
        }
        Icon::Help => {
            p.ring((10.0, 10.0), 7.5);
            let mut q = arc((10.0, 8.0), 2.6, 180.0, 400.0, 12);
            q.extend([(10.0, 10.8), (10.0, 12.0)]);
            p.line(&q);
            p.disc((10.0, 14.6), 1.0, fg);
        }
        Icon::Recent => {
            p.ring((10.0, 10.0), 7.5);
            p.line(&[(10.0, 5.5), (10.0, 10.0), (13.2, 12.0)]);
        }
        Icon::Tag => {
            p.closed(&[
                (3.0, 3.0),
                (10.0, 3.0),
                (17.0, 10.0),
                (10.0, 17.0),
                (3.0, 10.0),
            ]);
            p.ring((6.8, 6.8), 1.3);
        }
        Icon::Dimensions => {
            p.rect((3.0, 8.5), (17.0, 17.0), 1.0);
            p.seg((3.5, 4.5), (16.5, 4.5));
            p.chevron((3.5, 4.5), (10.0, 4.5), 2.2);
            p.chevron((16.5, 4.5), (10.0, 4.5), 2.2);
        }
        Icon::Close => {
            p.seg((5.0, 5.0), (15.0, 15.0));
            p.seg((15.0, 5.0), (5.0, 15.0));
        }
        Icon::ChevronDown => {
            p.line(&[(5.0, 7.5), (10.0, 12.5), (15.0, 7.5)]);
        }
        Icon::Pin => {
            p.seg((6.0, 3.0), (14.0, 3.0));
            p.line(&[
                (8.0, 3.0),
                (8.0, 8.5),
                (5.0, 12.0),
                (15.0, 12.0),
                (12.0, 8.5),
                (12.0, 3.0),
            ]);
            p.seg((10.0, 12.0), (10.0, 17.5));
        }
        Icon::Check => {
            p.line(&[(4.0, 10.5), (8.0, 14.5), (16.0, 5.5)]);
        }
        Icon::Border => {
            p.rect((2.5, 2.5), (17.5, 17.5), 1.0);
            p.rect_fill((6.0, 6.0), (14.0, 14.0), 0.5, fg.gamma_multiply(0.4));
            p.rect((6.0, 6.0), (14.0, 14.0), 0.5);
        }
        Icon::DropShadow => {
            let shadow = fg.gamma_multiply(0.35);
            p.rect_fill((14.0, 6.0), (17.5, 17.5), 0.0, shadow);
            p.rect_fill((6.0, 14.0), (14.0, 17.5), 0.0, shadow);
            p.rect((2.5, 2.5), (14.0, 14.0), 1.0);
        }
        Icon::Frame => {
            p.rect((2.5, 2.5), (17.5, 17.5), 0.0);
            p.rect((5.5, 5.5), (14.5, 14.5), 2.5);
        }
        Icon::TornEdge => {
            p.closed(&[
                (3.0, 3.0),
                (17.0, 3.0),
                (17.0, 14.0),
                (15.25, 16.0),
                (13.5, 14.0),
                (11.75, 16.0),
                (10.0, 14.0),
                (8.25, 16.0),
                (6.5, 14.0),
                (4.75, 16.0),
                (3.0, 14.0),
            ]);
        }
        Icon::Perspective => {
            p.closed(&[(6.0, 4.0), (14.0, 4.0), (18.0, 16.0), (2.0, 16.0)]);
            let thin = p.stroke_k(0.7, fg.gamma_multiply(0.7));
            p.seg_with((10.0, 4.0), (10.0, 16.0), thin);
            p.seg_with((4.0, 10.0), (16.0, 10.0), thin);
        }
        Icon::ColorFilter => {
            p.ring((10.0, 6.6), 4.5);
            p.ring((6.8, 12.2), 4.5);
            p.ring((13.2, 12.2), 4.5);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use egui::pos2;
    use std::collections::HashSet;

    fn style() -> IconStyle {
        IconStyle {
            fg: Color32::from_gray(220),
            accent: Color32::from_rgb(200, 30, 60),
        }
    }

    fn targets() -> [Rect; 3] {
        [
            Rect::from_min_size(pos2(0.0, 0.0), vec2(20.0, 20.0)),
            Rect::from_min_size(pos2(100.0, 50.0), vec2(48.0, 30.0)),
            Rect::from_min_size(pos2(3.0, 7.0), vec2(12.0, 12.0)),
        ]
    }

    #[test]
    fn every_icon_has_shapes() {
        for icon in Icon::ALL {
            for rect in targets() {
                assert!(!shapes(icon, rect, style()).is_empty(), "{icon:?} is empty");
            }
        }
    }

    #[test]
    fn shapes_stay_inside_target() {
        for icon in Icon::ALL {
            for rect in targets() {
                let allowed = rect.expand(2.0);
                for shape in shapes(icon, rect, style()) {
                    let b = shape.visual_bounding_rect();
                    assert!(
                        allowed.contains_rect(b),
                        "{icon:?} in {rect:?}: shape bounds {b:?} leave {allowed:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn all_is_unique_with_names() {
        let icons: HashSet<Icon> = Icon::ALL.iter().copied().collect();
        assert_eq!(icons.len(), Icon::ALL.len());
        let names: HashSet<&str> = Icon::ALL.iter().map(|i| i.name()).collect();
        assert_eq!(names.len(), Icon::ALL.len());
        assert!(Icon::ALL.iter().all(|i| !i.name().is_empty()));
        assert_eq!(Icon::RectangleSelect.name(), "Rectangle Select");
    }

    #[test]
    fn capture_uses_accent() {
        let rect = targets()[0];
        let s = style();
        let found = shapes(Icon::Capture, rect, s)
            .iter()
            .any(|shape| matches!(shape, Shape::Circle(c) if c.fill == s.accent));
        assert!(found);
    }

    #[test]
    fn redo_mirrors_undo() {
        let rect = targets()[0];
        let undo = shapes(Icon::Undo, rect, style());
        let redo = shapes(Icon::Redo, rect, style());
        assert_eq!(undo.len(), redo.len());
        let ub = undo.first().unwrap().visual_bounding_rect();
        let rb = redo.first().unwrap().visual_bounding_rect();
        assert!((ub.min.x - (20.0 - rb.max.x)).abs() < 1e-3);
    }

    #[test]
    fn stroke_never_below_one_point() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(8.0, 8.0));
        for shape in shapes(Icon::Close, rect, style()) {
            if let Shape::LineSegment { stroke, .. } = shape {
                assert!(stroke.width >= 1.0);
            }
        }
    }
}
