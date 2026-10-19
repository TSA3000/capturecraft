//! Text as vector geometry: callout text and step badge labels.
//!
//! [`TextSpec`]s are shaped and wrapped by photocraft-text (parley, the
//! same engine as PhotoCraft's Type tool), and their glyph outlines are
//! flattened with photocraft-vector into polygons. The prepare step fills
//! them like any other silhouette, so text anti-aliasing is the exact-area
//! coverage both compositors share and the CPU and GPU frames agree.
//!
//! Fonts come from photocraft-text's process-wide engine: its bundled
//! fonts (Inter, JetBrains Mono) always, plus the installed system fonts on
//! desktop builds and the craft-fonts collection when built with
//! `CRAFT_FONTS_DIR`. An unknown family falls back to the bundled default.

use photocraft_doc::text::{
    CharStyle, ParagraphRun, ParagraphStyle, TextAlign as DocAlign, TextRun, TextShape,
};
use photocraft_doc::{Knot, Path, Subpath, TextLayer};
use photocraft_geom::{Affine, Point};
use std::sync::Mutex;

use photocraft_text::render::{PathEl, outlines};
use photocraft_text::{TextEngine, TextLayout};
use photocraft_vector::flatten_path;

use crate::primitives::{MAX_COORD, P, TOLERANCE};
use crate::scene::{Color, TextAlign, TextSpec, TextVAlign};

/// Longest text laid out per node, in bytes (the format's limit).
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// Font sizes are clamped to this range, in pixels.
const SIZE_RANGE: (f32, f32) = (1.0, 1024.0);
/// Height of the layout box of wrapped text: tall enough never to hide a
/// line; the text is placed vertically from its measured bounds.
const BOX_HEIGHT: f32 = 1.0e6;

/// The shared engine (font database and shaping caches). Unit tests use
/// the bundled fonts only, so they do not depend on the machine.
fn engine() -> &'static Mutex<TextEngine> {
    #[cfg(test)]
    {
        static BUNDLED: std::sync::LazyLock<Mutex<TextEngine>> =
            std::sync::LazyLock::new(|| Mutex::new(TextEngine::new()));
        &BUNDLED
    }
    #[cfg(not(test))]
    photocraft_text::shared()
}

/// Polygons (filled non-zero, one component per glyph or decoration) of
/// every run colour of `spec`, through `t`.
pub fn text_components(spec: &TextSpec, t: &Affine) -> Vec<(Color, Vec<Vec<Vec<P>>>)> {
    let Some((layer, colors)) = layer(spec) else {
        return Vec::new();
    };
    let layout = engine()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .layout(&layer, 72.0);
    let Some(b) = layout.bounds() else {
        return Vec::new();
    };
    let f = spec.frame;
    // Point text starts at the anchor's baseline; box text inside its box.
    let dx = if spec.wrap {
        0.0
    } else {
        match spec.align {
            TextAlign::Left => f.x,
            TextAlign::Center => f.x + f.w * 0.5,
            TextAlign::Right => f.x + f.w,
        }
    };
    let top = match spec.valign {
        TextVAlign::Top => f.y,
        TextVAlign::Middle => f.y + (f.h - (b[3] - b[1])) * 0.5,
        TextVAlign::Bottom => f.y + f.h - (b[3] - b[1]),
    };
    let place = Affine::translate(f64::from(dx), f64::from(top - b[1])).then(t);
    if !place.m.iter().all(|v| v.is_finite()) {
        return Vec::new();
    }
    colors
        .iter()
        .enumerate()
        .filter_map(|(style, color)| {
            let comps = run_components(&layout, style as u32, &place);
            (!comps.is_empty()).then_some((*color, comps))
        })
        .collect()
}

/// The photocraft text layer of `spec` and the colour of each of its runs
/// (`None` when there is nothing to draw).
fn layer(spec: &TextSpec) -> Option<(TextLayer, Vec<Color>)> {
    if !spec.frame.is_valid() {
        return None;
    }
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut colors = Vec::new();
    for r in &spec.runs {
        if r.text.is_empty() || text.len() + r.text.len() > MAX_TEXT_BYTES {
            continue;
        }
        let size = if r.size_px.is_finite() {
            r.size_px.clamp(SIZE_RANGE.0, SIZE_RANGE.1)
        } else {
            continue;
        };
        text.push_str(&r.text);
        runs.push(TextRun {
            len: r.text.len(),
            style: CharStyle {
                font_family: r.family.clone(),
                weight: r.weight.clamp(1, 1000),
                italic: r.italic,
                size_pt: size,
                underline: r.underline,
                strikethrough: r.strike,
                ..CharStyle::default()
            },
        });
        colors.push(r.color);
    }
    if text.trim().is_empty() || colors.iter().all(|c| c.premultiplied()[3] <= 0.0) {
        return None;
    }
    let f = spec.frame;
    let shape = if spec.wrap {
        TextShape::Box {
            x: f.x,
            y: f.y,
            width: f.w,
            height: BOX_HEIGHT,
        }
    } else {
        TextShape::Point
    };
    let align = match spec.align {
        TextAlign::Left => DocAlign::Left,
        TextAlign::Center => DocAlign::Center,
        TextAlign::Right => DocAlign::Right,
    };
    let len = text.len();
    let layer = TextLayer {
        text,
        runs,
        paragraphs: vec![ParagraphRun {
            len,
            style: ParagraphStyle {
                align,
                ..ParagraphStyle::default()
            },
        }],
        shape,
        ..TextLayer::default()
    };
    Some((layer, colors))
}

/// Glyphs and decorations of run `style`, as polygons through `t`.
fn run_components(layout: &TextLayout, style: u32, t: &Affine) -> Vec<Vec<Vec<P>>> {
    let mut sub = layout.clone();
    sub.glyphs.retain(|g| g.style == style);
    let mut comps: Vec<Vec<Vec<P>>> = outlines(&sub, t, None)
        .iter()
        .map(|g| glyph_polygons(g))
        .filter(|c| !c.is_empty())
        .collect();
    for d in layout.decorations.iter().filter(|d| d.style == style) {
        let corners = [(d.x0, d.y0), (d.x1, d.y0), (d.x1, d.y1), (d.x0, d.y1)];
        let poly: Vec<P> = corners
            .iter()
            .map(|&(x, y)| {
                let (x, y) = layout.to_text(x, y);
                let p = t.apply(Point::new(f64::from(x), f64::from(y)));
                (p.x, p.y)
            })
            .collect();
        if poly.iter().all(|p| ok(*p)) {
            comps.push(vec![poly]);
        }
    }
    comps
}

fn ok(p: P) -> bool {
    p.0.is_finite() && p.1.is_finite() && p.0.abs() <= MAX_COORD && p.1.abs() <= MAX_COORD
}

/// One glyph's contours, flattened. Glyph contours are wound for the
/// non-zero rule, so counters stay open when the glyph is filled as one
/// component.
fn glyph_polygons(els: &[PathEl]) -> Vec<Vec<P>> {
    let mut subpaths = Vec::new();
    let mut cur: Vec<Knot> = Vec::new();
    let finish = |cur: &mut Vec<Knot>, out: &mut Vec<Subpath>| {
        if cur.len() >= 3 {
            out.push(Subpath {
                closed: true,
                knots: std::mem::take(cur),
                op: Default::default(),
            });
        }
        cur.clear();
    };
    let pt = |v: [f64; 2]| Point::new(v[0], v[1]);
    for el in els {
        match *el {
            PathEl::MoveTo(a) => {
                finish(&mut cur, &mut subpaths);
                cur.push(Knot::corner(a[0], a[1]));
            }
            PathEl::LineTo(a) => cur.push(Knot::corner(a[0], a[1])),
            PathEl::QuadTo(c, a) => {
                // Exact quadratic → cubic elevation.
                let Some(last) = cur.last_mut() else { continue };
                let s = last.anchor;
                last.out_ctrl = Point::new(
                    s.x + 2.0 / 3.0 * (c[0] - s.x),
                    s.y + 2.0 / 3.0 * (c[1] - s.y),
                );
                let c2 = Point::new(
                    a[0] + 2.0 / 3.0 * (c[0] - a[0]),
                    a[1] + 2.0 / 3.0 * (c[1] - a[1]),
                );
                cur.push(Knot {
                    anchor: pt(a),
                    in_ctrl: c2,
                    out_ctrl: pt(a),
                    smooth: false,
                });
            }
            PathEl::CurveTo(c1, c2, a) => {
                let Some(last) = cur.last_mut() else { continue };
                last.out_ctrl = pt(c1);
                cur.push(Knot {
                    anchor: pt(a),
                    in_ctrl: pt(c2),
                    out_ctrl: pt(a),
                    smooth: false,
                });
            }
            PathEl::Close => finish(&mut cur, &mut subpaths),
        }
    }
    finish(&mut cur, &mut subpaths);
    flatten_path(&Path::new(subpaths), TOLERANCE)
        .into_iter()
        .map(|l| l.pts)
        .filter(|p| p.len() >= 3 && p.iter().all(|q| ok(*q)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{RectF32, TextRunSpec};

    fn spec(text: &str, family: &str) -> TextSpec {
        TextSpec {
            runs: vec![TextRunSpec {
                text: text.into(),
                family: family.into(),
                size_px: 20.0,
                weight: 700,
                italic: false,
                color: Color::BLACK,
                underline: false,
                strike: false,
            }],
            frame: RectF32::new(10.0, 10.0, 100.0, 40.0),
            align: TextAlign::Center,
            valign: TextVAlign::Middle,
            wrap: false,
        }
    }

    fn bounds(c: &[(Color, Vec<Vec<Vec<P>>>)]) -> [f64; 4] {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for p in c.iter().flat_map(|(_, c)| c).flatten().flatten() {
            b = [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)];
        }
        b
    }

    #[test]
    fn a_label_is_centred_in_its_frame() {
        let c = text_components(&spec("12", "Inter"), &Affine::IDENTITY);
        assert_eq!(c.len(), 1);
        let b = bounds(&c);
        let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        assert!((cx - 60.0).abs() < 3.0, "{b:?}");
        assert!((cy - 30.0).abs() < 4.0, "{b:?}");
        assert!(b[2] - b[0] > 10.0 && b[3] - b[1] > 10.0, "{b:?}");
    }

    #[test]
    fn unknown_families_fall_back_and_empty_text_draws_nothing() {
        assert!(!text_components(&spec("A", "No Such Font 123"), &Affine::IDENTITY).is_empty());
        assert!(text_components(&spec("  ", "Inter"), &Affine::IDENTITY).is_empty());
        let mut bad = spec("A", "Inter");
        bad.frame.w = f32::NAN;
        assert!(text_components(&bad, &Affine::IDENTITY).is_empty());
    }

    #[test]
    fn wrapped_text_breaks_into_lines_and_follows_the_transform() {
        let mut s = spec("one two three four five six", "Inter");
        s.wrap = true;
        s.frame = RectF32::new(0.0, 0.0, 80.0, 200.0);
        s.valign = TextVAlign::Top;
        let b = bounds(&text_components(&s, &Affine::IDENTITY));
        assert!(b[2] <= 82.0 && b[3] > 40.0, "{b:?}");
        let moved = bounds(&text_components(&s, &Affine::translate(100.0, 50.0)));
        assert!((moved[0] - b[0] - 100.0).abs() < 1e-6 && (moved[1] - b[1] - 50.0).abs() < 1e-6);
    }
}
