//! Annotation editor: the capture opens here before it is saved.
//!
//! The model follows PhotoCraft (storytold/photocraft): coordinates are
//! `photocraft_geom` types in *image* space, annotations are an ordered
//! stack drawn bottom-to-top, and history is theirs — `photocraft-ops`
//! keeps labelled snapshots per step, capped by a state count, with redo
//! cleared on the next edit, which is what [`History`] does here. The blur
//! uses their Gaussian kernel from `photocraft-algo`. Nothing here panics:
//! no `unwrap`, `expect` or unchecked indexing, as their rules require.

use crate::RgbaImage;
use photocraft_geom::{Point, Rect, Size};
use xcap::image::{Rgba, imageops};

/// Tools the editor offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Arrow,
    Box,
    Highlight,
    Pixelate,
    Blur,
    Step,
    Crop,
}

impl Tool {
    pub const ALL: [Tool; 7] = [
        Tool::Arrow,
        Tool::Box,
        Tool::Highlight,
        Tool::Pixelate,
        Tool::Blur,
        Tool::Step,
        Tool::Crop,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Arrow => "Arrow",
            Tool::Box => "Box",
            Tool::Highlight => "Highlight",
            Tool::Pixelate => "Pixelate",
            Tool::Blur => "Blur",
            Tool::Step => "Step",
            Tool::Crop => "Crop",
        }
    }

    /// Tools that are dragged out between two points.
    pub fn is_drag(self) -> bool {
        !matches!(self, Tool::Step)
    }
}

/// One annotation in image space.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Arrow {
        from: Point,
        to: Point,
        color: [u8; 3],
        width: f32,
    },
    Box {
        rect: Rect,
        color: [u8; 3],
        width: f32,
    },
    Highlight {
        rect: Rect,
        color: [u8; 3],
    },
    Pixelate {
        rect: Rect,
        block: u32,
    },
    Blur {
        rect: Rect,
        sigma: f32,
    },
    Step {
        at: Point,
        number: u32,
        color: [u8; 3],
        radius: f32,
    },
}

impl Shape {
    /// Name of the step, as the Edit menu shows it.
    pub fn label(&self) -> &'static str {
        match self {
            Shape::Arrow { .. } => "Arrow",
            Shape::Box { .. } => "Box",
            Shape::Highlight { .. } => "Highlight",
            Shape::Pixelate { .. } => "Pixelate",
            Shape::Blur { .. } => "Blur",
            Shape::Step { .. } => "Step",
        }
    }

    /// Bounding box in image space, for hit tests and redraw bounds.
    pub fn bounds(&self) -> Rect {
        match self {
            Shape::Arrow {
                from, to, width, ..
            } => {
                let pad = width.ceil() as i32 + 8;
                rect_from_points(*from, *to).inflate(pad)
            }
            Shape::Box { rect, width, .. } => rect.inflate(width.ceil() as i32),
            Shape::Highlight { rect, .. }
            | Shape::Pixelate { rect, .. }
            | Shape::Blur { rect, .. } => *rect,
            Shape::Step { at, radius, .. } => {
                let r = radius.ceil() as i32;
                Rect::new(
                    at.x as i32 - r,
                    at.y as i32 - r,
                    at.x as i32 + r,
                    at.y as i32 + r,
                )
            }
        }
    }
}

/// One recorded step: the state *before* an edit, with the label shown in
/// the Edit menu. Mirrors `photocraft_ops::HistoryState`.
#[derive(Clone, Debug)]
struct State {
    label: String,
    shapes: Vec<Shape>,
    crop: Option<Rect>,
}

/// Labelled snapshot history, the model `photocraft-ops` uses: every edit
/// records the previous state, redo is cleared, and the oldest states fall
/// off beyond `max_states`.
#[derive(Clone, Debug)]
pub struct History {
    undo: std::collections::VecDeque<State>,
    redo: Vec<State>,
    pub max_states: usize,
    /// Label of the step that produced the current state.
    current_label: String,
}

impl Default for History {
    fn default() -> Self {
        Self::new(50)
    }
}

impl History {
    pub fn new(max_states: usize) -> Self {
        Self {
            undo: std::collections::VecDeque::new(),
            redo: Vec::new(),
            max_states: max_states.max(1),
            current_label: "Open".to_string(),
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Label of the step undo would take back, as the Edit menu shows it.
    pub fn undo_label(&self) -> Option<&str> {
        self.can_undo().then_some(self.current_label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }
}

/// An editable capture: the pixels plus the annotation stack.
#[derive(Clone, Debug)]
pub struct Document {
    base: RgbaImage,
    shapes: Vec<Shape>,
    /// Crop applied to the base image, in image space.
    crop: Option<Rect>,
    history: History,
}

impl Document {
    pub fn new(base: RgbaImage) -> Self {
        Self {
            base,
            shapes: Vec::new(),
            crop: None,
            history: History::default(),
        }
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// Records the state before an edit, under the label the menu shows.
    fn record(&mut self, label: &str) {
        let before = State {
            label: std::mem::replace(&mut self.history.current_label, label.to_string()),
            shapes: self.shapes.clone(),
            crop: self.crop,
        };
        self.history.undo.push_back(before);
        self.history.redo.clear();
        while self.history.undo.len() > self.history.max_states {
            self.history.undo.pop_front();
        }
    }

    /// Swaps `state` in and returns what was current, for the other stack.
    fn restore(&mut self, state: State) -> State {
        State {
            label: std::mem::replace(&mut self.history.current_label, state.label),
            shapes: std::mem::replace(&mut self.shapes, state.shapes),
            crop: std::mem::replace(&mut self.crop, state.crop),
        }
    }

    pub fn size(&self) -> Size {
        match self.crop {
            Some(r) => r.size(),
            None => Size::new(self.base.width(), self.base.height()),
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect::from_size(self.size())
    }

    pub fn shapes(&self) -> &[Shape] {
        &self.shapes
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Next step number, so badges count up as they are placed.
    pub fn next_step(&self) -> u32 {
        self.shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Step { number, .. } => Some(*number),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }

    /// Adds a shape, recording the step under its tool's name.
    pub fn push(&mut self, shape: Shape) {
        self.record(shape.label());
        self.shapes.push(shape);
    }

    pub fn undo(&mut self) {
        let Some(previous) = self.history.undo.pop_back() else {
            return;
        };
        let current = self.restore(previous);
        self.history.redo.push(current);
    }

    pub fn redo(&mut self) {
        let Some(next) = self.history.redo.pop() else {
            return;
        };
        let current = self.restore(next);
        self.history.undo.push_back(current);
    }

    /// Crops to `rect` (clipped to the image) and moves annotations with it.
    pub fn apply_crop(&mut self, rect: Rect) {
        let clipped = rect.intersect(&self.bounds());
        if clipped.is_empty() {
            return;
        }
        self.record("Crop");
        let offset = match self.crop {
            Some(existing) => Rect::new(
                existing.x0 + clipped.x0,
                existing.y0 + clipped.y0,
                existing.x0 + clipped.x1,
                existing.y0 + clipped.y1,
            ),
            None => clipped,
        };
        self.crop = Some(offset);
        let (dx, dy) = (-clipped.x0, -clipped.y0);
        for shape in &mut self.shapes {
            translate(shape, dx, dy);
        }
    }

    /// Flattens the base image and every annotation into a new image.
    pub fn render(&self) -> RgbaImage {
        let mut out = match self.crop {
            Some(r) => imageops::crop_imm(
                &self.base,
                r.x0.max(0) as u32,
                r.y0.max(0) as u32,
                r.width(),
                r.height(),
            )
            .to_image(),
            None => self.base.clone(),
        };
        for shape in &self.shapes {
            draw(&mut out, shape);
        }
        out
    }
}

fn translate(shape: &mut Shape, dx: i32, dy: i32) {
    let shift = |p: &mut Point| {
        p.x += f64::from(dx);
        p.y += f64::from(dy);
    };
    match shape {
        Shape::Arrow { from, to, .. } => {
            shift(from);
            shift(to);
        }
        Shape::Box { rect, .. }
        | Shape::Highlight { rect, .. }
        | Shape::Pixelate { rect, .. }
        | Shape::Blur { rect, .. } => *rect = rect.translate(dx, dy),
        Shape::Step { at, .. } => shift(at),
    }
}

/// Integer rect spanning two points, in any direction.
pub fn rect_from_points(a: Point, b: Point) -> Rect {
    let (x0, x1) = (a.x.min(b.x) as i32, a.x.max(b.x) as i32);
    let (y0, y1) = (a.y.min(b.y) as i32, a.y.max(b.y) as i32);
    Rect::new(x0, y0, x1, y1)
}

// ---------------------------------------------------------------- rendering

fn draw(img: &mut RgbaImage, shape: &Shape) {
    match shape {
        Shape::Arrow {
            from,
            to,
            color,
            width,
        } => draw_arrow(img, *from, *to, *color, *width),
        Shape::Box { rect, color, width } => draw_box(img, *rect, *color, *width),
        Shape::Highlight { rect, color } => tint(img, *rect, *color, 0.35),
        Shape::Pixelate { rect, block } => pixelate(img, *rect, *block),
        Shape::Blur { rect, sigma } => blur(img, *rect, *sigma),
        Shape::Step {
            at,
            number,
            color,
            radius,
        } => draw_step(img, *at, *number, *color, *radius),
    }
}

fn blend_pixel(img: &mut RgbaImage, x: i32, y: i32, color: [u8; 3], alpha: f32) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    let a = alpha.clamp(0.0, 1.0);
    let px = img.get_pixel_mut(x as u32, y as u32);
    for (channel, target) in px.0.iter_mut().zip(color).take(3) {
        let src = f32::from(*channel);
        let dst = f32::from(target);
        *channel = (src + (dst - src) * a).round().clamp(0.0, 255.0) as u8;
    }
    px.0[3] = 255;
}

/// Filled disc, anti-aliased at the edge.
fn disc(img: &mut RgbaImage, cx: f32, cy: f32, radius: f32, color: [u8; 3]) {
    let r = radius.max(0.5);
    let (x0, x1) = ((cx - r).floor() as i32, (cx + r).ceil() as i32);
    let (y0, y1) = ((cy - r).floor() as i32, (cy + r).ceil() as i32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            let alpha = (r - d + 0.5).clamp(0.0, 1.0);
            if alpha > 0.0 {
                blend_pixel(img, x, y, color, alpha);
            }
        }
    }
}

fn line(img: &mut RgbaImage, a: Point, b: Point, color: [u8; 3], width: f32) {
    let steps = ((b.x - a.x).abs().max((b.y - a.y).abs()) * 2.0)
        .ceil()
        .max(1.0);
    let r = (width / 2.0).max(0.5);
    for i in 0..=(steps as i32) {
        let t = f64::from(i) / steps;
        let x = a.x + (b.x - a.x) * t;
        let y = a.y + (b.y - a.y) * t;
        disc(img, x as f32, y as f32, r, color);
    }
}

fn draw_arrow(img: &mut RgbaImage, from: Point, to: Point, color: [u8; 3], width: f32) {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let head = (f64::from(width) * 4.0).min(len * 0.5);
    let (ux, uy) = (dx / len, dy / len);
    // Shaft stops where the head begins.
    let base = Point::new(to.x - ux * head, to.y - uy * head);
    line(img, from, base, color, width);

    // Filled triangular head.
    let half = head * 0.45;
    let (px, py) = (-uy, ux);
    let left = Point::new(base.x + px * half, base.y + py * half);
    let right = Point::new(base.x - px * half, base.y - py * half);
    fill_triangle(img, to, left, right, color);
}

fn fill_triangle(img: &mut RgbaImage, a: Point, b: Point, c: Point, color: [u8; 3]) {
    let min_x = a.x.min(b.x).min(c.x).floor() as i32;
    let max_x = a.x.max(b.x).max(c.x).ceil() as i32;
    let min_y = a.y.min(b.y).min(c.y).floor() as i32;
    let max_y = a.y.max(b.y).max(c.y).ceil() as i32;
    let area = edge(a, b, c);
    if area.abs() < f64::EPSILON {
        return;
    }
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let p = Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5);
            let (w0, w1, w2) = (edge(b, c, p), edge(c, a, p), edge(a, b, p));
            let inside = if area > 0.0 {
                w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0
            } else {
                w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0
            };
            if inside {
                blend_pixel(img, x, y, color, 1.0);
            }
        }
    }
}

fn edge(a: Point, b: Point, p: Point) -> f64 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

fn draw_box(img: &mut RgbaImage, rect: Rect, color: [u8; 3], width: f32) {
    if rect.is_empty() {
        return;
    }
    let t = width.max(1.0).round() as i32;
    for i in 0..t {
        let r = rect.inflate(-i);
        if r.is_empty() {
            break;
        }
        for x in r.x0..r.x1 {
            blend_pixel(img, x, r.y0, color, 1.0);
            blend_pixel(img, x, r.y1 - 1, color, 1.0);
        }
        for y in r.y0..r.y1 {
            blend_pixel(img, r.x0, y, color, 1.0);
            blend_pixel(img, r.x1 - 1, y, color, 1.0);
        }
    }
}

fn tint(img: &mut RgbaImage, rect: Rect, color: [u8; 3], alpha: f32) {
    let r = rect.intersect(&Rect::from_size(Size::new(img.width(), img.height())));
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            blend_pixel(img, x, y, color, alpha);
        }
    }
}

/// Averages each block, so faces and text become unreadable.
fn pixelate(img: &mut RgbaImage, rect: Rect, block: u32) {
    let bounds = Rect::from_size(Size::new(img.width(), img.height()));
    let r = rect.intersect(&bounds);
    if r.is_empty() {
        return;
    }
    let step = block.max(2) as i32;
    let mut y = r.y0;
    while y < r.y1 {
        let mut x = r.x0;
        while x < r.x1 {
            let cell = Rect::new(x, y, (x + step).min(r.x1), (y + step).min(r.y1));
            let mut sum = [0u64; 3];
            let mut count = 0u64;
            for cy in cell.y0..cell.y1 {
                for cx in cell.x0..cell.x1 {
                    if let Some(px) = get_pixel(img, cx, cy) {
                        for (total, value) in sum.iter_mut().zip(px.0).take(3) {
                            *total += u64::from(value);
                        }
                        count += 1;
                    }
                }
            }
            if count > 0 {
                let avg = [
                    (sum[0] / count) as u8,
                    (sum[1] / count) as u8,
                    (sum[2] / count) as u8,
                ];
                for cy in cell.y0..cell.y1 {
                    for cx in cell.x0..cell.x1 {
                        blend_pixel(img, cx, cy, avg, 1.0);
                    }
                }
            }
            x += step;
        }
        y += step;
    }
}

/// Normalized Gaussian kernel with standard deviation `sigma` (radius 3σ).
/// Ported from `photocraft_algo::blur::gaussian_kernel`.
pub(crate) fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    if sigma < 0.05 {
        return vec![1.0];
    }
    let r = (sigma * 3.0).ceil() as i32;
    let k: Vec<f32> = (-r..=r)
        .map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let s: f32 = k.iter().sum();
    if s <= 0.0 {
        return vec![1.0];
    }
    k.into_iter().map(|v| v / s).collect()
}

/// Separable Gaussian blur of `rect`, clamped at the edges: the same
/// two-pass convolution `photocraft-algo` uses, on an 8-bit RGBA image.
fn blur(img: &mut RgbaImage, rect: Rect, sigma: f32) {
    let bounds = Rect::from_size(Size::new(img.width(), img.height()));
    let r = rect.intersect(&bounds);
    if r.is_empty() {
        return;
    }
    let kernel = gaussian_kernel(sigma.max(0.5));
    let radius = (kernel.len() / 2) as i32;
    let (w, h) = (r.width() as usize, r.height() as usize);

    // Horizontal pass into a float buffer, sampling outside the rect so the
    // blur does not darken at its edges.
    let mut pass = vec![0.0f32; w * h * 3];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut acc = [0.0f32; 3];
            for (i, weight) in kernel.iter().enumerate() {
                let sx = (r.x0 + x + i as i32 - radius).clamp(bounds.x0, bounds.x1 - 1);
                if let Some(px) = get_pixel(img, sx, r.y0 + y) {
                    for (a, value) in acc.iter_mut().zip(px.0) {
                        *a += f32::from(value) * weight;
                    }
                }
            }
            let base = (y as usize * w + x as usize) * 3;
            for (i, value) in acc.iter().enumerate() {
                if let Some(slot) = pass.get_mut(base + i) {
                    *slot = *value;
                }
            }
        }
    }

    // Vertical pass, straight back into the image.
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut acc = [0.0f32; 3];
            for (i, weight) in kernel.iter().enumerate() {
                let sy = (y + i as i32 - radius).clamp(0, h as i32 - 1);
                let base = (sy as usize * w + x as usize) * 3;
                for (c, a) in acc.iter_mut().enumerate() {
                    if let Some(value) = pass.get(base + c) {
                        *a += value * weight;
                    }
                }
            }
            let color = [
                acc[0].round().clamp(0.0, 255.0) as u8,
                acc[1].round().clamp(0.0, 255.0) as u8,
                acc[2].round().clamp(0.0, 255.0) as u8,
            ];
            blend_pixel(img, r.x0 + x, r.y0 + y, color, 1.0);
        }
    }
}

fn get_pixel(img: &RgbaImage, x: i32, y: i32) -> Option<Rgba<u8>> {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return None;
    }
    Some(*img.get_pixel(x as u32, y as u32))
}

/// Numbered badge: filled disc with the number drawn in the 5x7 font.
fn draw_step(img: &mut RgbaImage, at: Point, number: u32, color: [u8; 3], radius: f32) {
    disc(img, at.x as f32, at.y as f32, radius, color);
    let text = number.to_string();
    let scale = ((radius / 6.0).round() as u32).max(1);
    let w = crate::draw::text_width(&text, scale) as f32;
    let h = (crate::draw::GLYPH_H * scale) as f32;
    let x0 = (at.x as f32 - w / 2.0).round() as i32;
    let y0 = (at.y as f32 - h / 2.0).round() as i32;
    crate::draw::text_mono(img, x0, y0, &text, scale, [255, 255, 255]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([10, 20, 30, 255]))
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut doc = Document::new(image(20, 20));
        assert!(!doc.can_undo() && !doc.can_redo());

        doc.push(Shape::Box {
            rect: Rect::new(1, 1, 5, 5),
            color: [255, 0, 0],
            width: 2.0,
        });
        assert!(doc.can_undo());
        doc.undo();
        assert!(!doc.can_undo() && doc.can_redo());
        doc.redo();
        assert_eq!(doc.shapes().len(), 1);

        // A new shape clears the redo stack.
        doc.undo();
        doc.push(Shape::Highlight {
            rect: Rect::new(0, 0, 3, 3),
            color: [255, 255, 0],
        });
        assert!(!doc.can_redo());
    }

    #[test]
    fn history_labels_follow_the_edits() {
        let mut doc = Document::new(image(20, 20));
        assert_eq!(doc.history().undo_label(), None);
        doc.push(Shape::Box {
            rect: Rect::new(1, 1, 9, 9),
            color: [255, 0, 0],
            width: 2.0,
        });
        assert_eq!(doc.history().undo_label(), Some("Box"));
        doc.apply_crop(Rect::new(0, 0, 10, 10));
        assert_eq!(doc.history().undo_label(), Some("Crop"));
        doc.undo();
        assert_eq!(doc.history().redo_label(), Some("Crop"));
        assert_eq!(doc.history().undo_label(), Some("Box"));
    }

    #[test]
    fn history_is_capped() {
        let mut doc = Document::new(image(20, 20));
        doc.history.max_states = 3;
        for _ in 0..10 {
            doc.push(Shape::Highlight {
                rect: Rect::new(0, 0, 5, 5),
                color: [255, 255, 0],
            });
        }
        assert_eq!(doc.history.undo.len(), 3);
        assert!(doc.can_undo());
    }

    #[test]
    fn crop_can_be_undone() {
        let mut doc = Document::new(image(40, 40));
        doc.apply_crop(Rect::new(10, 10, 30, 30));
        assert_eq!(doc.size(), Size::new(20, 20));
        doc.undo();
        assert_eq!(doc.size(), Size::new(40, 40));
        doc.redo();
        assert_eq!(doc.size(), Size::new(20, 20));
    }

    #[test]
    fn gaussian_kernel_is_normalized() {
        let k = gaussian_kernel(2.0);
        assert!(k.len() % 2 == 1);
        let sum: f32 = k.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "sum was {sum}");
        assert!(k.first() < k.get(k.len() / 2));
        assert_eq!(gaussian_kernel(0.0), vec![1.0]);
    }

    #[test]
    fn blur_smooths_an_edge() {
        let mut doc = Document::new(RgbaImage::from_fn(20, 20, |x, _| {
            Rgba([if x < 10 { 0 } else { 255 }, 0, 0, 255])
        }));
        doc.push(Shape::Blur {
            rect: Rect::new(0, 0, 20, 20),
            sigma: 3.0,
        });
        let out = doc.render();
        let mid = out.get_pixel(10, 10).0[0];
        assert!(mid > 0 && mid < 255, "edge not smoothed: {mid}");
    }

    #[test]
    fn step_numbers_count_up() {
        let mut doc = Document::new(image(20, 20));
        assert_eq!(doc.next_step(), 1);
        doc.push(Shape::Step {
            at: Point::new(5.0, 5.0),
            number: doc.next_step(),
            color: [255, 0, 0],
            radius: 8.0,
        });
        assert_eq!(doc.next_step(), 2);
    }

    #[test]
    fn rect_from_points_any_direction() {
        let a = rect_from_points(Point::new(10.0, 20.0), Point::new(2.0, 4.0));
        assert_eq!(a, Rect::new(2, 4, 10, 20));
    }

    #[test]
    fn crop_resizes_and_moves_shapes() {
        let mut doc = Document::new(image(40, 40));
        doc.push(Shape::Box {
            rect: Rect::new(12, 12, 20, 20),
            color: [255, 0, 0],
            width: 1.0,
        });
        doc.apply_crop(Rect::new(10, 10, 30, 30));
        assert_eq!(doc.size(), Size::new(20, 20));
        assert_eq!(doc.render().dimensions(), (20, 20));
        match doc.shapes().first() {
            Some(Shape::Box { rect, .. }) => assert_eq!(*rect, Rect::new(2, 2, 10, 10)),
            other => panic!("unexpected shape: {other:?}"),
        }
    }

    #[test]
    fn crop_outside_image_is_ignored() {
        let mut doc = Document::new(image(10, 10));
        doc.apply_crop(Rect::new(50, 50, 60, 60));
        assert_eq!(doc.size(), Size::new(10, 10));
    }

    #[test]
    fn render_draws_inside_bounds_only() {
        let mut doc = Document::new(image(16, 16));
        doc.push(Shape::Highlight {
            rect: Rect::new(-5, -5, 4, 4),
            color: [255, 255, 0],
        });
        let out = doc.render();
        assert_eq!(out.dimensions(), (16, 16));
        assert_ne!(out.get_pixel(0, 0), &Rgba([10, 20, 30, 255]));
        assert_eq!(out.get_pixel(15, 15), &Rgba([10, 20, 30, 255]));
    }

    #[test]
    fn pixelate_flattens_a_block() {
        let mut img = RgbaImage::from_fn(8, 8, |x, _| {
            Rgba([if x % 2 == 0 { 0 } else { 255 }, 0, 0, 255])
        });
        pixelate(&mut img, Rect::new(0, 0, 8, 8), 4);
        let first = *img.get_pixel(0, 0);
        assert_eq!(*img.get_pixel(1, 0), first);
        assert_eq!(*img.get_pixel(3, 3), first);
    }

    #[test]
    fn arrow_marks_pixels_near_the_tip() {
        let mut doc = Document::new(image(40, 40));
        doc.push(Shape::Arrow {
            from: Point::new(5.0, 5.0),
            to: Point::new(30.0, 30.0),
            color: [255, 0, 0],
            width: 3.0,
        });
        let out = doc.render();
        assert_ne!(out.get_pixel(29, 29), &Rgba([10, 20, 30, 255]));
    }
}
