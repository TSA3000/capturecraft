//! UI segmentation: a flattened screenshot into a [`UiSceneGraph`] of
//! containers, buttons, inputs, icons, dividers, images and text blocks.
//!
//! 1. **Flat regions.** 4-connected labelling of neighbouring pixels within
//!    [`SegmentParams::flat_tolerance`] of each other (tile-parallel,
//!    [`crate::label`]). Flat UI fills, gradients included, become one
//!    region each; glyphs, icons and photos break into small ones.
//! 2. **Surfaces.** Large regions are classified by their row spans:
//!    rectangles (corner radius measured from the first rows), circles,
//!    thin rings (borders, attached to the rectangle they frame, or turned
//!    into an outlined container) and thin dividers. Their nesting gives the
//!    hierarchy; a surface the colour of its parent that fills most of it
//!    is the parent's own body and is merged into it.
//! 3. **Foreground.** Pixels of small regions, minus anti-aliasing halos
//!    (pixels within 2 px of two different surfaces), are ink. Each one's
//!    local background is the smallest surface around it.
//! 4. **Lines.** Per surface, ink is closed horizontally (dilate → erode,
//!    radius from the median glyph height) and labelled 8-connected. Blobs
//!    that look like text are split into lines at empty rows; thin runs are
//!    dotted dividers; compact clusters icons; large busy areas images,
//!    in which MSER ([`crate::mser`]) looks for text.
//! 5. **Grouping and classification.** Lines stack into text blocks (font
//!    size from cap height, weight from stroke width). Surfaces become
//!    buttons (text inside or an accent fill, clear of the parent's edges),
//!    inputs (bordered, wide, neutral), icons (small squares) or
//!    containers; Sobel edge support along the outline is their confidence.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use capturecraft_engine::{self as engine, UiElementId};
use capturecraft_format::Color;
use photocraft_geom::Rect;

use crate::edges::{edge_density, side_support, sobel};
use crate::image::{Px, RgbaImage, dist, luma, mode, saturation};
use crate::label::{Conn, NONE, label};
use crate::morph::close;
use crate::mser::{MserParams, detect as mser};

pub type RectI32 = Rect;
pub type Rgba8 = Color;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointI32 {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiElementKind {
    /// Cards, modals, toolbars, sidebars: surfaces that hold other elements.
    Container {
        fill: Rgba8,
        border: Option<Rgba8>,
        corner_radius: f32,
    },
    TextBlock {
        /// Filled in by a text recogniser ([`crate::OcrEngine`]).
        text: Option<String>,
        font_size_px: f32,
        font_weight: u16,
        dominant_fg: Rgba8,
        dominant_bg: Rgba8,
        lines: Vec<RectI32>,
    },
    Button {
        bounds: RectI32,
        corner_radius: f32,
        fill_color: Rgba8,
        border_color: Option<Rgba8>,
        label: Option<Box<UiElement>>,
    },
    IconOrAvatar {
        bounds: RectI32,
        is_circular: bool,
        dominant_color: Rgba8,
    },
    InputField {
        bounds: RectI32,
        bg_color: Rgba8,
        border_color: Rgba8,
    },
    Divider {
        start: PointI32,
        end: PointI32,
        thickness: u32,
        color: Rgba8,
    },
    /// Photos and other busy content.
    Image { dominant_color: Rgba8 },
}

impl UiElementKind {
    pub fn engine_kind(&self) -> engine::UiElementKind {
        match self {
            UiElementKind::Container { .. } => engine::UiElementKind::Panel,
            UiElementKind::TextBlock { .. } => engine::UiElementKind::Text,
            UiElementKind::Button { .. } => engine::UiElementKind::Button,
            UiElementKind::IconOrAvatar { .. } => engine::UiElementKind::Icon,
            UiElementKind::InputField { .. } => engine::UiElementKind::Input,
            UiElementKind::Divider { .. } => engine::UiElementKind::Divider,
            UiElementKind::Image { .. } => engine::UiElementKind::Image,
        }
    }
}

/// One detected object, base-space coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct UiElement {
    pub id: UiElementId,
    pub parent: Option<UiElementId>,
    pub bounds: RectI32,
    /// The colour around the element (its parent's fill).
    pub background: Rgba8,
    pub confidence: f32,
    pub kind: UiElementKind,
}

impl UiElement {
    fn new(kind: UiElementKind, bounds: Rect, background: Rgba8, confidence: f32) -> Self {
        Self {
            id: UiElementId::of(kind.engine_kind(), bounds),
            parent: None,
            bounds,
            background,
            confidence,
            kind,
        }
    }
}

/// Everything found in a region, parents before children. Button labels
/// live inside their button ([`UiElementKind::Button::label`]).
#[derive(Clone, Debug, PartialEq)]
pub struct UiSceneGraph {
    pub region: Rect,
    /// The page background behind everything.
    pub background: Rgba8,
    pub elements: Vec<UiElement>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentParams {
    /// Largest per-channel step between neighbours of one flat region.
    pub flat_tolerance: u8,
    pub min_surface_area: u32,
    pub min_surface_side: u32,
    /// Sobel magnitude (`|gx| + |gy|` on luma) that counts as an edge.
    pub edge_threshold: u16,
    /// Look for text inside images with MSER.
    pub mser: bool,
}

impl Default for SegmentParams {
    fn default() -> Self {
        Self {
            flat_tolerance: 6,
            min_surface_area: 300,
            min_surface_side: 10,
            edge_threshold: 96,
            mser: true,
        }
    }
}

fn color(p: Px) -> Color {
    Color::rgba(p[0], p[1], p[2], p[3])
}

impl UiSceneGraph {
    /// Segments `img` (see the module docs). Never fails: an empty or
    /// featureless image gives an empty graph.
    pub fn analyze(img: &RgbaImage, p: &SegmentParams) -> Self {
        Segmenter::new(img, p).run()
    }

    pub fn get(&self, id: UiElementId) -> Option<&UiElement> {
        self.all().find(|e| e.id == id)
    }

    /// Every element, button labels included.
    pub fn all(&self) -> impl Iterator<Item = &UiElement> {
        self.elements.iter().flat_map(|e| {
            let label = match &e.kind {
                UiElementKind::Button { label: Some(l), .. } => Some(l.as_ref()),
                _ => None,
            };
            std::iter::once(e).chain(label)
        })
    }

    pub fn children(&self, id: UiElementId) -> impl Iterator<Item = &UiElement> {
        self.all().filter(move |e| e.parent == Some(id))
    }

    /// Text blocks, button labels included.
    pub fn text_blocks(&self) -> impl Iterator<Item = &UiElement> {
        self.all()
            .filter(|e| matches!(e.kind, UiElementKind::TextBlock { .. }))
    }

    /// The flat form the engine works with.
    pub fn to_engine(&self) -> Vec<engine::UiElement> {
        self.all().map(to_engine).collect()
    }
}

fn to_engine(e: &UiElement) -> engine::UiElement {
    let mut o = engine::UiElement::new(e.kind.engine_kind(), e.bounds);
    o.id = e.id;
    o.parent = e.parent;
    o.confidence = e.confidence;
    o.bg = e.background;
    match &e.kind {
        UiElementKind::Container {
            fill,
            border,
            corner_radius,
        } => {
            o.bg = *fill;
            o.fg = border.unwrap_or(e.background);
            o.border = *border;
            o.corner_radius = *corner_radius;
        }
        UiElementKind::TextBlock {
            text,
            font_size_px,
            font_weight,
            dominant_fg,
            dominant_bg,
            lines,
        } => {
            o.fg = *dominant_fg;
            o.bg = *dominant_bg;
            o.text = text.clone();
            o.font_px = *font_size_px;
            o.font_weight = *font_weight;
            o.lines = lines.clone();
        }
        UiElementKind::Button {
            corner_radius,
            fill_color,
            border_color,
            label,
            ..
        } => {
            o.bg = *fill_color;
            o.fg = match label.as_deref().map(|l| &l.kind) {
                Some(UiElementKind::TextBlock { dominant_fg, .. }) => *dominant_fg,
                _ => border_color.unwrap_or(*fill_color),
            };
            o.border = *border_color;
            o.corner_radius = *corner_radius;
        }
        UiElementKind::IconOrAvatar {
            is_circular,
            dominant_color,
            ..
        } => {
            o.fg = *dominant_color;
            o.circular = *is_circular;
        }
        UiElementKind::InputField {
            bg_color,
            border_color,
            ..
        } => {
            o.bg = *bg_color;
            o.fg = *border_color;
            o.border = Some(*border_color);
            o.corner_radius = 0.0;
        }
        UiElementKind::Divider { color, .. } => o.fg = *color,
        UiElementKind::Image { dominant_color } => o.fg = *dominant_color,
    }
    o
}

// ---------------------------------------------------------------------------
// Implementation

#[derive(Clone, Copy)]
struct Reg {
    area: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    sum: [u64; 4],
    border: bool,
}

impl Reg {
    fn rect(&self) -> Rect {
        Rect::new(self.x0, self.y0, self.x1, self.y1)
    }

    fn mean(&self) -> Px {
        let n = u64::from(self.area.max(1));
        self.sum.map(|s| (s / n) as u8)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    /// Ink: glyphs, icon detail, anti-aliasing, photo texture.
    Small,
    Root,
    /// A large irregular flat area (the page around a sidebar).
    Backdrop,
    Surface,
    Divider,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    Rect,
    Circle,
    Ring(u32),
}

#[derive(Clone, Copy, Debug)]
struct Surf {
    rect: Rect,
    fill: Px,
    border: Option<Px>,
    radius: f32,
    shape: Shape,
    score: f32,
    alive: bool,
    parent: Option<usize>,
}

struct Glyph {
    rect: Rect,
    area: u32,
    perim: u32,
}

/// A text line, icon or image candidate from the closed ink.
#[derive(Clone)]
struct Blob {
    rect: Rect,
    ink: u32,
    owner: u32,
    glyphs: Vec<u32>,
}

struct TextOut {
    rect: Rect,
    owner: u32,
    lines: Vec<Rect>,
    glyphs: Vec<u32>,
}

struct Segmenter<'a> {
    img: &'a RgbaImage,
    p: SegmentParams,
    w: usize,
    h: usize,
}

impl<'a> Segmenter<'a> {
    fn new(img: &'a RgbaImage, p: &SegmentParams) -> Self {
        Self {
            img,
            p: *p,
            w: img.width(),
            h: img.height(),
        }
    }

    fn px(&self, i: usize) -> Px {
        self.img.pixels().get(i).copied().unwrap_or([0; 4])
    }

    fn run(self) -> UiSceneGraph {
        let region = self.img.rect();
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 {
            return UiSceneGraph {
                region,
                background: Color::WHITE,
                elements: Vec::new(),
            };
        }
        let origin = (region.x0, region.y0);
        let full = Rect::new(0, 0, w as i32, h as i32);
        let pix = self.img.pixels();
        let tol = self.p.flat_tolerance;

        // 1. Flat regions.
        let flat = label(
            w,
            h,
            origin,
            Conn::Four,
            |_| true,
            |a, b| dist(pix[a], pix[b]) <= tol,
        );
        let regs = self.region_stats(&flat.labels, flat.count);
        let root = (0..regs.len())
            .filter(|&k| regs[k].border)
            .max_by_key(|&k| (regs[k].area, Reverse(k)));
        let root_fill = root
            .map(|r| regs[r].mean())
            .or_else(|| mode(pix.iter().copied()))
            .unwrap_or([255; 4]);

        // 2. Surfaces.
        let mut role = vec![Role::Small; regs.len()];
        if let Some(r) = root {
            role[r] = Role::Root;
        }
        let min_side = self.p.min_surface_side as i32;
        let cand: Vec<usize> = (0..regs.len())
            .filter(|&k| role[k] != Role::Root)
            .filter(|&k| {
                let r = &regs[k];
                r.area >= self.p.min_surface_area
                    && r.x1 - r.x0 >= min_side
                    && r.y1 - r.y0 >= min_side
            })
            .collect();
        let spans = self.row_spans(&flat.labels, &regs, &cand);
        let mut surfs: Vec<Surf> = Vec::new();
        let mut dividers: Vec<(Rect, Px, f32)> = Vec::new();
        for (ci, &k) in cand.iter().enumerate() {
            let r = &regs[k];
            match shape_of(r, &spans[ci]) {
                Some((shape, radius, score)) => {
                    role[k] = Role::Surface;
                    surfs.push(Surf {
                        rect: r.rect(),
                        fill: r.mean(),
                        border: None,
                        radius,
                        shape,
                        score,
                        alive: true,
                        parent: None,
                    });
                }
                None => {
                    let rw = (r.x1 - r.x0) as u64;
                    let rh = (r.y1 - r.y0) as u64;
                    let fill = f64::from(r.area) / (rw * rh) as f64;
                    // Large or solid irregular areas are background; the
                    // rest (big headline glyphs) is ink.
                    if r.area >= 4000 || fill >= 0.55 {
                        role[k] = Role::Backdrop;
                    }
                }
            }
        }
        for (k, r) in regs.iter().enumerate() {
            if role[k] != Role::Small {
                continue;
            }
            let (rw, rh) = (r.x1 - r.x0, r.y1 - r.y0);
            let thin = (rh <= 3 && rw >= 24) || (rw <= 3 && rh >= 24);
            if thin && f64::from(r.area) >= 0.85 * f64::from(rw * rh) {
                role[k] = Role::Divider;
                dividers.push((r.rect(), r.mean(), 0.8));
            }
        }
        self.attach_rings(&mut surfs);
        link(&mut surfs);
        // A surface the colour of its parent filling most of it is the
        // parent's own body (a card's fill below its header).
        for i in 0..surfs.len() {
            let s = surfs[i];
            if !s.alive || s.shape != Shape::Rect || s.border.is_some() {
                continue;
            }
            if let Some(pi) = s.parent {
                let par = surfs[pi];
                if dist(s.fill, par.fill) <= tol * 2
                    && s.rect.size().area() * 2 >= par.rect.size().area()
                {
                    surfs[i].alive = false;
                }
            }
        }
        link(&mut surfs);
        let owner = self.owner_map(&surfs);
        let fill_of = |o: u32| -> Px {
            if o == NONE {
                root_fill
            } else {
                surfs.get(o as usize).map_or(root_fill, |s| s.fill)
            }
        };

        // 3. Foreground.
        let big: Vec<bool> = role.iter().map(|r| *r != Role::Small).collect();
        let fg = self.foreground(&flat.labels, &big);

        // 4. Glyphs and lines.
        let glyph_l = label(w, h, origin, Conn::Eight, |i| fg[i], |_, _| true);
        let glyphs = self.glyph_stats(&glyph_l.labels, glyph_l.count, &fg);
        let mut heights: Vec<u32> = glyphs
            .iter()
            .filter(|g| g.area >= 4 && (3..=200).contains(&g.rect.height()))
            .map(|g| g.rect.height())
            .collect();
        heights.sort_unstable();
        let h_med = heights.get(heights.len() / 2).copied().unwrap_or(10).max(3);
        let rx = ((h_med as f32 * 0.45).round() as usize).clamp(2, 24);
        let mut by_owner: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (gi, g) in glyphs.iter().enumerate() {
            let c = g.rect;
            let o = owner[(c.y0 as usize) * w + c.x0 as usize];
            by_owner.entry(o).or_default().push(gi);
        }
        let mag = sobel(self.img);
        let thr = self.p.edge_threshold;
        let mut lines: Vec<Blob> = Vec::new();
        let mut icons: Vec<Blob> = Vec::new();
        let mut images: Vec<Blob> = Vec::new();
        for (&o, gs) in &by_owner {
            let crop = gs
                .iter()
                .map(|&g| glyphs[g].rect)
                .reduce(|a, b| a.union(&b))
                .unwrap_or(Rect::EMPTY)
                .inflate(rx as i32)
                .intersect(&full);
            for b in self.blobs(crop, o, &owner, &fg, &glyph_l.labels, rx) {
                let (bw, bh) = (b.rect.width(), b.rect.height());
                if b.ink < 6 {
                    continue;
                }
                if (bh <= 3 && bw >= 24) || (bw <= 3 && bh >= 24) {
                    let ink = self.ink_color(b.rect, b.owner, &owner, &fg, fill_of(b.owner));
                    dividers.push((b.rect, ink, 0.6));
                } else if (bw >= 72 && bh >= 72)
                    || (bw >= 48 && bh >= 48 && edge_density(&mag, w, b.rect, thr) >= 0.3)
                {
                    images.push(b);
                } else if (4..=120).contains(&bh) && bw >= bh && b.glyphs.len() >= 2 {
                    lines.extend(self.split_lines(
                        &b,
                        &fg,
                        &owner,
                        &glyphs,
                        &glyph_l.labels,
                        h_med,
                    ));
                } else if bw <= 128 && bh <= 128 {
                    icons.push(b);
                } else if bh <= 120 {
                    lines.push(b);
                } else {
                    images.push(b);
                }
            }
        }
        let mut texts = group_lines(lines);
        if self.p.mser {
            for im in &images {
                texts.extend(self.mser_lines(im));
            }
        }

        // 5. Elements.
        let base = |r: Rect| self.img.global(r);
        let mut ids: Vec<Option<UiElementId>> = vec![None; surfs.len()];
        let mut kinds: Vec<Option<UiElementKind>> = vec![None; surfs.len()];
        let mut text_in: Vec<usize> = vec![0; surfs.len()];
        for t in &texts {
            if let Some(n) = text_in.get_mut(t.owner as usize) {
                *n += 1;
            }
        }
        for (i, s) in surfs.iter().enumerate() {
            if !s.alive {
                continue;
            }
            let pr = s.parent.map_or(full, |pi| surfs[pi].rect);
            let pfill = s.parent.map_or(root_fill, |pi| surfs[pi].fill);
            let k = classify(s, base(s.rect), pr, pfill, text_in[i]);
            ids[i] = Some(UiElementId::of(k.engine_kind(), base(s.rect)));
            kinds[i] = Some(k);
        }
        let parent_id = |o: u32| -> Option<UiElementId> {
            if o == NONE {
                None
            } else {
                ids.get(o as usize).copied().flatten()
            }
        };
        let mut out: Vec<UiElement> = Vec::new();
        let mut buttons: BTreeMap<usize, usize> = BTreeMap::new();
        for (i, s) in surfs.iter().enumerate() {
            let Some(kind) = kinds[i].clone() else {
                continue;
            };
            let bgc = s.parent.map_or(root_fill, |pi| surfs[pi].fill);
            let support = side_support(&mag, w, h, s.rect, thr);
            let conf = match s.shape {
                Shape::Circle => 0.6 + 0.35 * s.score,
                _ => 0.5 + 0.45 * support,
            };
            if matches!(kind, UiElementKind::Button { .. }) {
                buttons.insert(i, out.len());
            }
            let mut e = UiElement::new(kind, base(s.rect), color(bgc), conf.clamp(0.0, 1.0));
            e.parent = s.parent.and_then(|pi| ids[pi]);
            out.push(e);
        }
        for t in &texts {
            let bg = self
                .text_background(t, &fg)
                .unwrap_or_else(|| fill_of(t.owner));
            let ink = self.ink_color(t.rect, t.owner, &owner, &fg, bg);
            let (font_px, weight) = font_metrics(t, &glyphs);
            let n = t.glyphs.len().min(4) as f32;
            let kind = UiElementKind::TextBlock {
                text: None,
                font_size_px: font_px,
                font_weight: weight,
                dominant_fg: color(ink),
                dominant_bg: color(bg),
                lines: t.lines.iter().map(|l| base(*l)).collect(),
            };
            let mut e = UiElement::new(kind, base(t.rect), color(bg), 0.55 + 0.1 * n);
            e.confidence = e.confidence.min(0.95);
            e.parent = parent_id(t.owner);
            // The first text in a button is its label.
            if let Some(&bi) = buttons.get(&(t.owner as usize))
                && let Some(UiElementKind::Button {
                    label: l @ None, ..
                }) = out.get_mut(bi).map(|b| &mut b.kind)
            {
                *l = Some(Box::new(e));
                continue;
            }
            out.push(e);
        }
        for b in &icons {
            let bg = fill_of(b.owner);
            let ink = self.ink_color(b.rect, b.owner, &owner, &fg, bg);
            let kind = UiElementKind::IconOrAvatar {
                bounds: base(b.rect),
                is_circular: false,
                dominant_color: color(ink),
            };
            let mut e = UiElement::new(kind, base(b.rect), color(bg), 0.6);
            e.parent = parent_id(b.owner);
            out.push(e);
        }
        for b in &images {
            let dom = mode(self.pixels_in(b.rect)).unwrap_or(root_fill);
            let mut e = UiElement::new(
                UiElementKind::Image {
                    dominant_color: color(dom),
                },
                base(b.rect),
                color(fill_of(b.owner)),
                0.7,
            );
            e.parent = parent_id(b.owner);
            out.push(e);
        }
        for (r, c, conf) in dividers {
            let o = owner[(r.y0 as usize) * w + r.x0 as usize];
            let horizontal = r.width() >= r.height();
            let g = base(r);
            let (start, end, thickness) = if horizontal {
                let y = (g.y0 + g.y1) / 2;
                (
                    PointI32 { x: g.x0, y },
                    PointI32 { x: g.x1 - 1, y },
                    r.height(),
                )
            } else {
                let x = (g.x0 + g.x1) / 2;
                (
                    PointI32 { x, y: g.y0 },
                    PointI32 { x, y: g.y1 - 1 },
                    r.width(),
                )
            };
            let kind = UiElementKind::Divider {
                start,
                end,
                thickness,
                color: color(c),
            };
            let mut e = UiElement::new(kind, g, color(fill_of(o)), conf);
            e.parent = parent_id(o);
            out.push(e);
        }
        // Text found inside images belongs to them.
        let image_ids: Vec<(Rect, UiElementId)> = out
            .iter()
            .filter(|e| matches!(e.kind, UiElementKind::Image { .. }))
            .map(|e| (e.bounds, e.id))
            .collect();
        for e in &mut out {
            if matches!(e.kind, UiElementKind::TextBlock { .. })
                && let Some((_, id)) = image_ids.iter().find(|(r, _)| r.contains_rect(&e.bounds))
            {
                e.parent = Some(*id);
            }
        }
        // Parents first (by depth), then reading position.
        let depth: BTreeMap<UiElementId, usize> = {
            let parents: BTreeMap<UiElementId, Option<UiElementId>> =
                out.iter().map(|e| (e.id, e.parent)).collect();
            out.iter()
                .map(|e| {
                    let mut d = 0;
                    let mut cur = e.parent;
                    while let Some(p) = cur {
                        d += 1;
                        if d > out.len() {
                            break;
                        }
                        cur = parents.get(&p).copied().flatten();
                    }
                    (e.id, d)
                })
                .collect()
        };
        out.sort_by_key(|e| {
            (
                depth.get(&e.id).copied().unwrap_or(0),
                e.bounds.y0,
                e.bounds.x0,
                e.id,
            )
        });
        out.dedup_by_key(|e| e.id);
        UiSceneGraph {
            region,
            background: color(root_fill),
            elements: out,
        }
    }

    fn region_stats(&self, labels: &[u32], count: usize) -> Vec<Reg> {
        let mut regs = vec![
            Reg {
                area: 0,
                x0: i32::MAX,
                y0: i32::MAX,
                x1: i32::MIN,
                y1: i32::MIN,
                sum: [0; 4],
                border: false,
            };
            count
        ];
        let (w, h) = (self.w, self.h);
        for (i, &l) in labels.iter().enumerate() {
            let Some(r) = regs.get_mut(l as usize) else {
                continue;
            };
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            r.area += 1;
            r.x0 = r.x0.min(x);
            r.y0 = r.y0.min(y);
            r.x1 = r.x1.max(x + 1);
            r.y1 = r.y1.max(y + 1);
            for (s, v) in r.sum.iter_mut().zip(self.px(i)) {
                *s += u64::from(v);
            }
            r.border |= x == 0 || y == 0 || x as usize == w - 1 || y as usize == h - 1;
        }
        regs
    }

    /// Per candidate region and bbox row: (min x, max x, pixel count).
    fn row_spans(&self, labels: &[u32], regs: &[Reg], cand: &[usize]) -> Vec<Vec<(i32, i32, u32)>> {
        let mut idx = vec![NONE; regs.len()];
        for (ci, &k) in cand.iter().enumerate() {
            idx[k] = ci as u32;
        }
        let mut spans: Vec<Vec<(i32, i32, u32)>> = cand
            .iter()
            .map(|&k| vec![(i32::MAX, i32::MIN, 0); (regs[k].y1 - regs[k].y0).max(0) as usize])
            .collect();
        let w = self.w;
        for (i, &l) in labels.iter().enumerate() {
            let Some(&ci) = idx.get(l as usize) else {
                continue;
            };
            if ci == NONE {
                continue;
            }
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            let y0 = regs[cand[ci as usize]].y0;
            if let Some(s) = spans[ci as usize].get_mut((y - y0) as usize) {
                s.0 = s.0.min(x);
                s.1 = s.1.max(x);
                s.2 += 1;
            }
        }
        spans
    }

    /// Rings become the border of the rectangle they frame, or an outlined
    /// container filled with the colour most common inside them.
    fn attach_rings(&self, surfs: &mut [Surf]) {
        for i in 0..surfs.len() {
            let Shape::Ring(t) = surfs[i].shape else {
                continue;
            };
            let ring = surfs[i];
            let inner = ring.rect.inflate(-(t as i32));
            let near = |a: i32, b: i32| (a - b).abs() <= 2;
            let framed = (0..surfs.len()).find(|&j| {
                let s = &surfs[j];
                j != i
                    && s.alive
                    && s.shape == Shape::Rect
                    && near(s.rect.x0, inner.x0)
                    && near(s.rect.y0, inner.y0)
                    && near(s.rect.x1, inner.x1)
                    && near(s.rect.y1, inner.y1)
            });
            match framed {
                Some(j) => {
                    let s = &mut surfs[j];
                    s.border = Some(ring.fill);
                    s.rect = ring.rect;
                    s.radius = s.radius.max(ring.radius);
                    s.score = s.score.max(ring.score);
                    surfs[i].alive = false;
                }
                None => {
                    let fill = mode(self.pixels_in(inner)).unwrap_or(ring.fill);
                    let s = &mut surfs[i];
                    s.border = Some(ring.fill);
                    s.fill = fill;
                    s.shape = Shape::Rect;
                }
            }
        }
    }

    fn pixels_in(&self, r: Rect) -> impl Iterator<Item = Px> + '_ {
        let r = r.intersect(&Rect::new(0, 0, self.w as i32, self.h as i32));
        let step = if r.size().area() > 1 << 16 { 2 } else { 1 };
        (r.y0..r.y1)
            .step_by(step)
            .flat_map(move |y| (r.x0..r.x1).step_by(step).map(move |x| self.img.get(x, y)))
    }

    /// Per pixel, the smallest live surface whose box holds it (or NONE).
    fn owner_map(&self, surfs: &[Surf]) -> Vec<u32> {
        let mut owner = vec![NONE; self.w * self.h];
        let mut order: Vec<usize> = (0..surfs.len()).filter(|&i| surfs[i].alive).collect();
        order.sort_by_key(|&i| Reverse(surfs[i].rect.size().area()));
        for i in order {
            let r = surfs[i].rect;
            for y in r.y0..r.y1 {
                let row = y as usize * self.w;
                if let Some(s) = owner.get_mut(row + r.x0 as usize..row + r.x1 as usize) {
                    s.fill(i as u32);
                }
            }
        }
        owner
    }

    /// Ink: small-region pixels that are not a halo between two surfaces.
    fn foreground(&self, labels: &[u32], big: &[bool]) -> Vec<bool> {
        let (w, h) = (self.w as i32, self.h as i32);
        let is_big = |l: u32| big.get(l as usize).copied().unwrap_or(false);
        let mut fg = vec![false; labels.len()];
        for (i, slot) in fg.iter_mut().enumerate() {
            if is_big(labels[i]) {
                continue;
            }
            let (x, y) = (i as i32 % w, i as i32 / w);
            let mut first = NONE;
            let mut halo = false;
            'win: for dy in -2..=2 {
                for dx in -2..=2 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w || ny >= h {
                        continue;
                    }
                    let l = labels[(ny * w + nx) as usize];
                    if !is_big(l) {
                        continue;
                    }
                    if first == NONE {
                        first = l;
                    } else if l != first {
                        halo = true;
                        break 'win;
                    }
                }
            }
            *slot = !halo;
        }
        fg
    }

    fn glyph_stats(&self, labels: &[u32], count: usize, fg: &[bool]) -> Vec<Glyph> {
        let mut g: Vec<Glyph> = (0..count)
            .map(|_| Glyph {
                rect: Rect::new(i32::MAX, i32::MAX, i32::MIN, i32::MIN),
                area: 0,
                perim: 0,
            })
            .collect();
        let (w, h) = (self.w as i32, self.h as i32);
        let on = |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h && fg[(y * w + x) as usize];
        for (i, &l) in labels.iter().enumerate() {
            let Some(gl) = g.get_mut(l as usize) else {
                continue;
            };
            let (x, y) = (i as i32 % w, i as i32 / w);
            gl.area += 1;
            gl.rect = Rect::new(
                gl.rect.x0.min(x),
                gl.rect.y0.min(y),
                gl.rect.x1.max(x + 1),
                gl.rect.y1.max(y + 1),
            );
            if !(on(x - 1, y) && on(x + 1, y) && on(x, y - 1) && on(x, y + 1)) {
                gl.perim += 1;
            }
        }
        g
    }

    /// Closed ink of one owner inside `crop`, as blobs.
    #[allow(clippy::too_many_arguments)]
    fn blobs(
        &self,
        crop: Rect,
        o: u32,
        owner: &[u32],
        fg: &[bool],
        glyph_l: &[u32],
        rx: usize,
    ) -> Vec<Blob> {
        let (cw, ch) = (crop.width() as usize, crop.height() as usize);
        if cw == 0 || ch == 0 {
            return Vec::new();
        }
        let w = self.w;
        let gidx = |cx: usize, cy: usize| (crop.y0 as usize + cy) * w + crop.x0 as usize + cx;
        let mask: Vec<bool> = (0..cw * ch)
            .map(|k| {
                let i = gidx(k % cw, k / cw);
                fg[i] && owner[i] == o
            })
            .collect();
        let closed = close(&mask, cw, ch, rx, 1);
        let origin = (self.img.rect().x0 + crop.x0, self.img.rect().y0 + crop.y0);
        let cl = label(cw, ch, origin, Conn::Eight, |k| closed[k], |_, _| true);
        let mut blobs: Vec<Blob> = (0..cl.count)
            .map(|_| Blob {
                rect: Rect::new(i32::MAX, i32::MAX, i32::MIN, i32::MIN),
                ink: 0,
                owner: o,
                glyphs: Vec::new(),
            })
            .collect();
        for k in 0..cw * ch {
            if !mask[k] {
                continue;
            }
            let Some(b) = blobs.get_mut(cl.labels[k] as usize) else {
                continue;
            };
            let (x, y) = ((k % cw) as i32 + crop.x0, (k / cw) as i32 + crop.y0);
            b.ink += 1;
            b.rect = Rect::new(
                b.rect.x0.min(x),
                b.rect.y0.min(y),
                b.rect.x1.max(x + 1),
                b.rect.y1.max(y + 1),
            );
            let g = glyph_l[gidx(k % cw, k / cw)];
            if b.glyphs.last() != Some(&g) {
                b.glyphs.push(g);
            }
        }
        for b in &mut blobs {
            b.glyphs.sort_unstable();
            b.glyphs.dedup();
        }
        blobs.retain(|b| b.ink > 0);
        blobs
    }

    /// Splits a tall blob into lines at rows without ink.
    fn split_lines(
        &self,
        b: &Blob,
        fg: &[bool],
        owner: &[u32],
        glyphs: &[Glyph],
        glyph_l: &[u32],
        h_med: u32,
    ) -> Vec<Blob> {
        if (b.rect.height() as f32) < 1.6 * h_med as f32 {
            return vec![b.clone()];
        }
        let w = self.w;
        let r = b.rect;
        let ink_at = |x: i32, y: i32| {
            let i = y as usize * w + x as usize;
            fg[i] && owner[i] == b.owner && b.glyphs.binary_search(&glyph_l[i]).is_ok()
        };
        let rows: Vec<u32> = (r.y0..r.y1)
            .map(|y| (r.x0..r.x1).filter(|&x| ink_at(x, y)).count() as u32)
            .collect();
        let mut out = Vec::new();
        let mut y = 0usize;
        while y < rows.len() {
            if rows[y] == 0 {
                y += 1;
                continue;
            }
            let s = y;
            while y < rows.len() && rows[y] > 0 {
                y += 1;
            }
            let band = Rect::new(r.x0, r.y0 + s as i32, r.x1, r.y0 + y as i32);
            let gs: Vec<u32> = b
                .glyphs
                .iter()
                .copied()
                .filter(|&g| {
                    glyphs
                        .get(g as usize)
                        .is_some_and(|gl| !gl.rect.intersect(&band).is_empty())
                })
                .collect();
            let (mut x0, mut x1) = (i32::MAX, i32::MIN);
            let mut ink = 0;
            for yy in band.y0..band.y1 {
                for xx in band.x0..band.x1 {
                    if ink_at(xx, yy) {
                        x0 = x0.min(xx);
                        x1 = x1.max(xx + 1);
                        ink += 1;
                    }
                }
            }
            if ink > 0 {
                out.push(Blob {
                    rect: Rect::new(x0, band.y0, x1, band.y1),
                    ink,
                    owner: b.owner,
                    glyphs: gs,
                });
            }
        }
        out
    }

    /// The colour of the ink in `r` on `bg`: the mean of the pixels at
    /// least 60 % as far from `bg` as the farthest one.
    fn ink_color(&self, r: Rect, o: u32, owner: &[u32], fg: &[bool], bg: Px) -> Px {
        let w = self.w;
        let mut ink: Vec<Px> = Vec::new();
        for y in r.y0.max(0)..r.y1.min(self.h as i32) {
            for x in r.x0.max(0)..r.x1.min(w as i32) {
                let i = y as usize * w + x as usize;
                if fg[i] && owner[i] == o {
                    ink.push(self.px(i));
                }
            }
        }
        if ink.is_empty() {
            ink = self.pixels_in(r).collect();
        }
        let far = ink.iter().map(|p| dist(*p, bg)).max().unwrap_or(0);
        let cut = (u16::from(far) * 6 / 10) as u8;
        let (mut s, mut n) = ([0u32; 4], 0u32);
        for p in ink.iter().filter(|p| dist(**p, bg) >= cut) {
            for c in 0..4 {
                s[c] += u32::from(p[c]);
            }
            n += 1;
        }
        if n == 0 {
            return bg;
        }
        s.map(|v| (v / n) as u8)
    }

    /// The most common non-ink colour on a 2 px band around a text block.
    fn text_background(&self, t: &TextOut, fg: &[bool]) -> Option<Px> {
        let (w, h) = (self.w as i32, self.h as i32);
        let r = t.rect;
        let o = r.inflate(2).intersect(&Rect::new(0, 0, w, h));
        let mut px = Vec::new();
        for y in o.y0..o.y1 {
            for x in o.x0..o.x1 {
                if !r.contains(x, y) && !fg[(y * w + x) as usize] {
                    px.push(self.img.get(x, y));
                }
            }
        }
        mode(px.into_iter())
    }

    /// Text lines inside an image, from MSER regions chained left to right.
    fn mser_lines(&self, im: &Blob) -> Vec<TextOut> {
        let r = im.rect;
        let (cw, ch) = (r.width() as usize, r.height() as usize);
        let gray: Vec<u8> = (r.y0..r.y1)
            .flat_map(|y| (r.x0..r.x1).map(move |x| (x, y)))
            .map(|(x, y)| luma(self.img.get(x, y)))
            .collect();
        let mut glyphs: Vec<Rect> = mser(&gray, cw, ch, &MserParams::default())
            .into_iter()
            .map(|m| m.rect)
            .filter(|g| {
                (6..=64).contains(&g.height()) && g.width() >= 2 && g.width() <= g.height() * 2
            })
            .collect();
        glyphs.sort_by_key(|g| (g.x0, g.y0));
        glyphs.dedup();
        glyphs.truncate(2000);
        // Union-find chaining of horizontally adjacent, vertically aligned glyphs.
        let n = glyphs.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        for a in 0..n {
            for b in a + 1..n {
                let (ga, gb) = (glyphs[a], glyphs[b]);
                let mh = ga.height().max(gb.height()) as i32;
                if gb.x0 - ga.x1 > mh {
                    break;
                }
                let overlap = ga.y1.min(gb.y1) - ga.y0.max(gb.y0);
                let ratio = ga.height().max(gb.height()) as f32
                    / ga.height().min(gb.height()).max(1) as f32;
                if overlap as f32 >= 0.6 * ga.height().min(gb.height()) as f32 && ratio <= 1.5 {
                    let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                    parent[ra.max(rb)] = ra.min(rb);
                }
            }
        }
        let mut groups: BTreeMap<usize, Vec<Rect>> = BTreeMap::new();
        for (k, g) in glyphs.iter().enumerate() {
            let root = find(&mut parent, k);
            groups.entry(root).or_default().push(*g);
        }
        groups
            .into_values()
            .filter(|g| g.len() >= 3)
            .filter_map(|g| g.into_iter().reduce(|a, b| a.union(&b)))
            .map(|l| {
                let l = l.translate(r.x0, r.y0);
                TextOut {
                    rect: l,
                    owner: im.owner,
                    lines: vec![l],
                    glyphs: Vec::new(),
                }
            })
            .collect()
    }
}

/// Parent of each live surface: the smallest live surface whose box holds it.
fn link(surfs: &mut [Surf]) {
    let mut order: Vec<usize> = (0..surfs.len()).filter(|&i| surfs[i].alive).collect();
    order.sort_by_key(|&i| {
        (
            Reverse(surfs[i].rect.size().area()),
            surfs[i].rect.y0,
            surfs[i].rect.x0,
            i,
        )
    });
    for s in surfs.iter_mut() {
        s.parent = None;
    }
    for (pos, &i) in order.iter().enumerate() {
        let r = surfs[i].rect;
        let parent = order[..pos]
            .iter()
            .rev()
            .copied()
            .find(|&j| surfs[j].rect.contains_rect(&r) && surfs[j].rect != r);
        surfs[i].parent = parent;
    }
}

/// Rectangle (with corner radius), circle or thin ring, from the row spans.
fn shape_of(r: &Reg, spans: &[(i32, i32, u32)]) -> Option<(Shape, f32, f32)> {
    let (w, h) = (r.x1 - r.x0, r.y1 - r.y0);
    if w <= 0 || h <= 0 || spans.len() != h as usize {
        return None;
    }
    let fill = f64::from(r.area) / f64::from(w * h);
    let width_of = |s: &(i32, i32, u32)| if s.2 == 0 { 0 } else { s.1 - s.0 + 1 };
    // Circle: the span widths follow 2·√(r² − dy²).
    if (w - h).abs() <= 2.max(w.max(h) / 12) && (0.45..0.92).contains(&fill) {
        let rad = (w + h) as f32 / 4.0;
        let cy = (r.y0 + r.y1) as f32 / 2.0;
        let err: f32 = spans
            .iter()
            .enumerate()
            .map(|(k, s)| {
                let dy = r.y0 as f32 + k as f32 + 0.5 - cy;
                let ideal = 2.0 * (rad * rad - dy * dy).max(0.0).sqrt();
                (width_of(s) as f32 - ideal).abs()
            })
            .sum::<f32>()
            / h as f32;
        if err <= 1.0 + 0.04 * rad && fill < 0.88 {
            return Some((Shape::Circle, rad, (1.0 - err / rad).clamp(0.0, 1.0)));
        }
    }
    // Rectangle: the middle rows run edge to edge.
    let a = ((h as f32 * 0.35) as usize).min(spans.len() - 1);
    let b = ((h as f32 * 0.65).ceil() as usize).clamp(a + 1, spans.len());
    let band = &spans[a..b];
    let full = band
        .iter()
        .filter(|s| s.2 > 0 && s.0 <= r.x0 + 2 && s.1 >= r.x1 - 3)
        .count();
    if (full as f32) < 0.9 * band.len() as f32 {
        return None;
    }
    if fill < 0.4 {
        let t = f64::from(r.area) / f64::from(2 * (w + h));
        let mid = band.iter().map(|s| f64::from(s.2)).sum::<f64>() / band.len() as f64;
        if t <= 4.5 && mid <= 4.0 * t + 2.0 {
            let t = t.round().max(1.0) as u32;
            return Some((Shape::Ring(t), corner_radius(r, spans), 1.0));
        }
        return None;
    }
    if fill < 0.55 {
        return None;
    }
    Some((Shape::Rect, corner_radius(r, spans), 1.0))
}

/// From how far the first and last rows stop short of the corners: a
/// quarter circle of radius `r` leaves `d ≈ r − √r` uncovered on row 0.
fn corner_radius(r: &Reg, spans: &[(i32, i32, u32)]) -> f32 {
    let inset = |s: &(i32, i32, u32)| -> f32 {
        if s.2 == 0 {
            return 0.0;
        }
        ((s.0 - r.x0).max(r.x1 - 1 - s.1)).max(0) as f32
    };
    let d = match (spans.first(), spans.last()) {
        (Some(a), Some(b)) => (inset(a) + inset(b)) / 2.0,
        _ => 0.0,
    };
    if d < 0.5 {
        return 0.0;
    }
    let half = (r.x1 - r.x0).min(r.y1 - r.y0) as f32 / 2.0;
    (d + d.sqrt() + 0.5).min(half)
}

/// Lines stacked closely, left- or centre-aligned, of similar height, on
/// the same surface form one block.
fn group_lines(mut lines: Vec<Blob>) -> Vec<TextOut> {
    lines.sort_by_key(|l| (l.owner, l.rect.y0, l.rect.x0));
    let mut out: Vec<TextOut> = Vec::new();
    for l in lines {
        let r = l.rect;
        let joined = out.iter_mut().rev().take(8).find(|t| {
            let Some(last) = t.lines.last() else {
                return false;
            };
            let mh = r.height().max(last.height()) as f32;
            let similar = (r.height() as f32 - last.height() as f32).abs() <= 0.35 * mh;
            let gap = (r.y0 - last.y1) as f32;
            let aligned = ((r.x0 - last.x0).abs() as f32) <= 0.75 * mh
                || (((r.x0 + r.x1) - (last.x0 + last.x1)).abs() as f32 / 2.0) <= 0.75 * mh;
            t.owner == l.owner && similar && (-1.0..=mh).contains(&gap) && aligned
        });
        match joined {
            Some(t) => {
                t.rect = t.rect.union(&r);
                t.lines.push(r);
                t.glyphs.extend(l.glyphs);
            }
            None => out.push(TextOut {
                rect: r,
                owner: l.owner,
                lines: vec![r],
                glyphs: l.glyphs,
            }),
        }
    }
    out
}

/// Font size from the cap height (`≈ 0.72 em` for UI sans fonts) and weight
/// from the stroke width (`2 · area / perimeter`) relative to it.
fn font_metrics(t: &TextOut, glyphs: &[Glyph]) -> (f32, u16) {
    let gs: Vec<&Glyph> = t
        .glyphs
        .iter()
        .filter_map(|&g| glyphs.get(g as usize))
        .filter(|g| g.rect.height() >= 3)
        .collect();
    let line_h = t.lines.first().map_or(t.rect.height(), |l| l.height()) as f32;
    if gs.is_empty() {
        return ((line_h / 0.9).clamp(4.0, 400.0), 400);
    }
    // Cap height per line (median baseline to the top of the tall glyphs).
    let caps: Vec<f32> = t
        .lines
        .iter()
        .filter_map(|l| {
            let lg: Vec<&&Glyph> = gs
                .iter()
                .filter(|g| !g.rect.intersect(l).is_empty())
                .collect();
            let mut bottoms: Vec<i32> = lg.iter().map(|g| g.rect.y1).collect();
            bottoms.sort_unstable();
            let baseline = *bottoms.get(bottoms.len() / 2)?;
            let max_h = lg.iter().map(|g| g.rect.height()).max()?;
            let top = lg
                .iter()
                .filter(|g| g.rect.height() * 2 >= max_h)
                .map(|g| g.rect.y0)
                .min()?;
            (baseline > top).then(|| (baseline - top) as f32)
        })
        .collect();
    let size = if caps.is_empty() {
        line_h / 0.9
    } else {
        caps.iter().sum::<f32>() / caps.len() as f32 / 0.72
    }
    .clamp(4.0, 400.0);
    let area: u32 = gs.iter().map(|g| g.area).sum();
    let perim: u32 = gs.iter().map(|g| g.perim).sum::<u32>().max(1);
    let stroke = 2.0 * area as f32 / perim as f32;
    let ratio = stroke / size;
    let weight = (400.0 + (ratio - 0.09) / 0.035 * 300.0).clamp(100.0, 900.0);
    (size, ((weight / 100.0).round() * 100.0) as u16)
}

/// What a surface is, from its size, fill, frame and place in its parent.
fn classify(s: &Surf, bounds: Rect, parent: Rect, parent_fill: Px, texts: usize) -> UiElementKind {
    let (w, h) = (s.rect.width() as f32, s.rect.height() as f32);
    let fill = color(s.fill);
    if s.shape == Shape::Circle {
        return UiElementKind::IconOrAvatar {
            bounds,
            is_circular: true,
            dominant_color: fill,
        };
    }
    let slack = 3 + s.border.map_or(0, |_| 2);
    let touches = [
        (s.rect.x0 - parent.x0).abs() <= slack,
        (s.rect.y0 - parent.y0).abs() <= slack,
        (parent.x1 - s.rect.x1).abs() <= slack,
        (parent.y1 - s.rect.y1).abs() <= slack,
    ]
    .iter()
    .filter(|t| **t)
    .count();
    let aspect = w.max(h) / w.min(h).max(1.0);
    let contrast = dist(s.fill, parent_fill);
    if w <= 64.0 && h <= 64.0 && aspect <= 1.4 && texts == 0 {
        return UiElementKind::IconOrAvatar {
            bounds,
            is_circular: false,
            dominant_color: fill,
        };
    }
    let container = || UiElementKind::Container {
        fill,
        border: s.border.map(color),
        corner_radius: s.radius,
    };
    if touches >= 2 || w >= 0.9 * parent.width() as f32 || h > 80.0 {
        return container();
    }
    if (14.0..=80.0).contains(&h) && w >= h {
        if let Some(b) = s.border
            && w >= 3.0 * h
            && saturation(s.fill) < 0.25
            && contrast < 48
        {
            return UiElementKind::InputField {
                bounds,
                bg_color: fill,
                border_color: color(b),
            };
        }
        if texts >= 1 || saturation(s.fill) >= 0.25 || contrast >= 48 {
            return UiElementKind::Button {
                bounds,
                corner_radius: s.radius,
                fill_color: fill,
                border_color: s.border.map(color),
                label: None,
            };
        }
    }
    container()
}
