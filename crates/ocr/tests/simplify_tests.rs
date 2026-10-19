//! A synthetic dark-mode settings dialog run through the whole pipeline:
//! `UiSceneGraph::analyze`, then `smart.moveObject` on the primary button
//! (the hole must heal to the dialog's fill), then `smart.simplify` (the
//! vector wireframe must mirror the dialog).
#![allow(clippy::unwrap_used, clippy::panic)]

use capturecraft_engine::{Pixels, Session, UiElementKind as Kind};
use capturecraft_format::{Annotation, Color, Fill, Manifest, RegionShape, ShapeNode};
use capturecraft_ocr::{Analyzer, RgbaImage, UiElementKind, UiSceneGraph};
use photocraft_geom::{Rect, Size};
use serde_json::{Value, json};

const W: i32 = 480;
const H: i32 = 320;
const PAGE: [u8; 4] = [0x1E, 0x1F, 0x22, 255];
const CARD: [u8; 4] = [0x2B, 0x2D, 0x31, 255];
const BORDER: [u8; 4] = [0x3F, 0x41, 0x47, 255];
const HEADER: [u8; 4] = [0x38, 0x3A, 0x40, 255];
const TEXT: [u8; 4] = [0xDB, 0xDE, 0xE1, 255];
const MUTED: [u8; 4] = [0x94, 0x9B, 0xA4, 255];
const AVATAR: [u8; 4] = [0xF0, 0xB2, 0x32, 255];
const ACCENT: [u8; 4] = [0x58, 0x65, 0xF2, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

const CARD_R: Rect = Rect::new(40, 30, 440, 290);
const HEADER_R: Rect = Rect::new(41, 31, 439, 71);
const BUTTON_R: Rect = Rect::new(300, 236, 420, 272);
/// Avatar: centre and radius.
const AVATAR_C: (f32, f32, f32) = (90.0, 120.0, 22.0);

struct Canvas(Vec<[u8; 4]>);

impl Canvas {
    fn new() -> Self {
        Canvas(vec![PAGE; (W * H) as usize])
    }

    fn rect(&mut self, r: Rect, c: [u8; 4]) {
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                self.0[(y * W + x) as usize] = c;
            }
        }
    }

    /// Anti-aliased fill: `inside(x, y)` sampled 4 × 4 per pixel.
    fn shape(&mut self, bounds: Rect, c: [u8; 4], inside: impl Fn(f32, f32) -> bool) {
        for y in bounds.y0..bounds.y1 {
            for x in bounds.x0..bounds.x1 {
                let mut n = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let (fx, fy) = (
                            x as f32 + (sx as f32 + 0.5) / 4.0,
                            y as f32 + (sy as f32 + 0.5) / 4.0,
                        );
                        n += u32::from(inside(fx, fy));
                    }
                }
                if n == 0 {
                    continue;
                }
                let a = n as f32 / 16.0;
                let p = &mut self.0[(y * W + x) as usize];
                for k in 0..3 {
                    p[k] = (f32::from(p[k]) * (1.0 - a) + f32::from(c[k]) * a).round() as u8;
                }
            }
        }
    }

    fn rounded(&mut self, r: Rect, radius: f32, c: [u8; 4]) {
        let (x0, y0, x1, y1) = (r.x0 as f32, r.y0 as f32, r.x1 as f32, r.y1 as f32);
        self.shape(r, c, |x, y| {
            let cx = x.clamp(x0 + radius, x1 - radius);
            let cy = y.clamp(y0 + radius, y1 - radius);
            (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
        });
    }

    fn circle(&mut self, (cx, cy, r): (f32, f32, f32), c: [u8; 4]) {
        let b = Rect::new(
            (cx - r) as i32 - 1,
            (cy - r) as i32 - 1,
            (cx + r) as i32 + 2,
            (cy + r) as i32 + 2,
        );
        self.shape(b, c, |x, y| (x - cx).powi(2) + (y - cy).powi(2) <= r * r);
    }

    /// Blocky "glyphs" (a stem and a cap bar, 6 × 11 px, 2 px apart), with
    /// a wider gap every `word` letters.
    fn text(&mut self, x: i32, y: i32, letters: i32, word: i32, c: [u8; 4]) -> Rect {
        let mut cx = x;
        for k in 0..letters {
            if k > 0 && k % word == 0 {
                cx += 5;
            }
            self.rect(Rect::new(cx + 2, y, cx + 4, y + 11), c);
            self.rect(Rect::new(cx, y, cx + 6, y + 2), c);
            if k % 3 == 1 {
                self.rect(Rect::new(cx, y + 9, cx + 6, y + 11), c);
            }
            cx += 8;
        }
        Rect::new(x, y, cx - 2, y + 11)
    }

    fn rgba(&self) -> Vec<u8> {
        self.0.iter().flatten().copied().collect()
    }
}

struct Dialog {
    rgba: Vec<u8>,
    title: Rect,
    name: Rect,
    description: Rect,
    label: Rect,
}

/// Page, bordered card, header bar with a title, avatar, two lines of text
/// and a primary button.
fn dialog() -> Dialog {
    let mut c = Canvas::new();
    c.rect(CARD_R, BORDER);
    c.rect(CARD_R.inflate(-1), CARD);
    c.rect(HEADER_R, HEADER);
    let title = c.text(56, 45, 12, 5, TEXT);
    c.circle(AVATAR_C, AVATAR);
    let name = c.text(126, 104, 12, 7, TEXT);
    let description = c.text(126, 124, 24, 6, MUTED);
    c.rounded(BUTTON_R, 6.0, ACCENT);
    let label = c.text(338, 248, 6, 6, WHITE);
    Dialog {
        rgba: c.rgba(),
        title,
        name,
        description,
        label,
    }
}

fn near(a: Rect, b: Rect, tol: i32) -> bool {
    (a.x0 - b.x0).abs() <= tol
        && (a.y0 - b.y0).abs() <= tol
        && (a.x1 - b.x1).abs() <= tol
        && (a.y1 - b.y1).abs() <= tol
}

fn rgba(c: Color) -> [u8; 4] {
    [c.r, c.g, c.b, c.a]
}

fn close_to(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tol)
}

#[test]
fn scene_graph_of_a_dark_settings_dialog() {
    let d = dialog();
    let img = RgbaImage::from_rgba8(Rect::new(0, 0, W, H), &d.rgba).unwrap();
    let g = UiSceneGraph::analyze(&img, &Default::default());
    assert!(close_to(rgba(g.background), PAGE, 1));

    let card = g
        .elements
        .iter()
        .find(|e| near(e.bounds, CARD_R, 1))
        .unwrap_or_else(|| panic!("card not found: {:#?}", g.elements));
    let UiElementKind::Container { fill, border, .. } = &card.kind else {
        panic!("card is {:?}", card.kind);
    };
    assert!(close_to(rgba(*fill), CARD, 1));
    assert!(close_to(rgba(border.unwrap()), BORDER, 1));

    let header = g
        .elements
        .iter()
        .find(|e| near(e.bounds, HEADER_R, 1))
        .unwrap();
    assert!(
        matches!(header.kind, UiElementKind::Container { .. }),
        "{:?}",
        header.kind
    );
    assert_eq!(header.parent, Some(card.id));

    let title = g
        .text_blocks()
        .find(|e| near(e.bounds, d.title, 1))
        .unwrap();
    assert_eq!(title.parent, Some(header.id));

    // The two lines, stacked and left-aligned, are one block.
    let body = g
        .text_blocks()
        .find(|e| near(e.bounds, d.name.union(&d.description), 1))
        .unwrap_or_else(|| {
            panic!(
                "text block not found: {:#?}",
                g.text_blocks().collect::<Vec<_>>()
            )
        });
    let UiElementKind::TextBlock {
        lines,
        dominant_fg,
        dominant_bg,
        font_size_px,
        ..
    } = &body.kind
    else {
        unreachable!()
    };
    assert_eq!(lines.len(), 2);
    assert!(
        near(lines[0], d.name, 1) && near(lines[1], d.description, 1),
        "{lines:?}"
    );
    // Two inks (name and description): the estimate lies between them.
    let fg = rgba(*dominant_fg);
    assert!(
        (0..3).all(|c| (MUTED[c].min(TEXT[c]) - 2..=MUTED[c].max(TEXT[c]) + 2).contains(&fg[c])),
        "{fg:?}"
    );
    assert!(close_to(rgba(*dominant_bg), CARD, 1));
    assert!((12.0..=18.0).contains(font_size_px), "{font_size_px}");
    assert_eq!(body.parent, Some(card.id));

    let avatar = g
        .elements
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                UiElementKind::IconOrAvatar {
                    is_circular: true,
                    ..
                }
            )
        })
        .unwrap_or_else(|| panic!("avatar not found: {:#?}", g.elements));
    assert!(
        near(avatar.bounds, Rect::new(68, 98, 112, 142), 2),
        "{:?}",
        avatar.bounds
    );
    assert_eq!(avatar.parent, Some(card.id));

    let button = g
        .elements
        .iter()
        .find(|e| matches!(e.kind, UiElementKind::Button { .. }))
        .unwrap_or_else(|| panic!("button not found: {:#?}", g.elements));
    assert!(near(button.bounds, BUTTON_R, 1), "{:?}", button.bounds);
    let UiElementKind::Button {
        fill_color,
        corner_radius,
        label,
        ..
    } = &button.kind
    else {
        unreachable!()
    };
    assert!(close_to(rgba(*fill_color), ACCENT, 1));
    assert!((3.0..=10.0).contains(corner_radius), "{corner_radius}");
    let label = label.as_ref().unwrap();
    assert!(near(label.bounds, d.label, 1), "{:?}", label.bounds);
    assert_eq!(label.parent, Some(button.id));
    let UiElementKind::TextBlock { dominant_fg, .. } = &label.kind else {
        unreachable!()
    };
    assert!(close_to(rgba(*dominant_fg), WHITE, 8), "{dominant_fg:?}");

    // Nothing else: no stray icons from anti-aliasing.
    assert_eq!(g.all().count(), 7, "{:#?}", g.elements);
    // Same pixels, same graph.
    assert_eq!(g, UiSceneGraph::analyze(&img, &Default::default()));
}

fn session(d: &Dialog) -> Session {
    let size = Size::new(W as u32, H as u32);
    let mut s = Session::new();
    s.open(Manifest::new(size), Pixels::new(size, d.rgba.clone()))
        .unwrap();
    s.set_analyzer(Box::new(Analyzer::morphological()));
    s
}

fn find(objects: &Value, kind: &str) -> Vec<Value> {
    objects["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["kind"] == kind)
        .cloned()
        .collect()
}

#[test]
fn smart_move_heals_and_simplify_draws_the_wireframe() {
    let d = dialog();
    let mut s = session(&d);

    // --- Smart Move the primary button 180 px to the left.
    let found = s.run("smart.detectObjects", &json!({})).unwrap();
    assert_eq!(found["analyzer"], "morphological");
    let buttons = find(&found, "button");
    assert_eq!(buttons.len(), 1, "{found:#}");
    let id = buttons[0]["id"].clone();
    let r = s
        .run(
            "smart.moveObject",
            &json!({ "object": id, "by": { "x": -180, "y": 0 } }),
        )
        .unwrap();
    let node = capturecraft_format::NodeId(r["id"].as_u64().unwrap());
    let doc = s.active().unwrap();
    let Some(Annotation::SmartObject(obj)) = doc.manifest.node(node).and_then(|n| n.annotation())
    else {
        panic!("smart object expected");
    };
    assert!(near(obj.source, BUTTON_R, 1));
    assert_eq!(
        (obj.rect.x, obj.rect.y),
        (f64::from(obj.source.x0 - 180), f64::from(obj.source.y0))
    );

    // The lifted pixels: opaque fill and label, keyed-out corners.
    let lifted = doc.object_pixels(node).unwrap();
    let ow = lifted.size().width as i32;
    assert!(close_to(lifted.pixel(ow / 2, 2), ACCENT, 1));
    assert_eq!(lifted.pixel(0, 0)[3], 0, "corner must be transparent");
    assert!(lifted.pixel(1, 1)[3] < 255);
    let label_px = lifted.pixel(
        d.label.x0 + 3 - obj.source.x0,
        d.label.y0 + 5 - obj.source.y0,
    );
    assert!(close_to(label_px, WHITE, 1), "{label_px:?}");

    // The hole: every pixel of the old button area is the dialog's fill.
    let edited = doc.edited_pixels().unwrap();
    let hole = edited.read_rgba8(obj.source).unwrap();
    let worst = hole
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            p.iter()
                .zip(CARD)
                .map(|(a, b)| a.abs_diff(b))
                .max()
                .unwrap()
        })
        .max()
        .unwrap();
    assert!(
        worst <= 1,
        "hole not healed cleanly (worst channel error {worst})"
    );
    // Nothing outside the hole changed, and the capture itself is intact.
    let base = doc.pixels.as_ref().unwrap();
    for (x, y) in [(150, 120), (299, 250), (421, 250), (360, 235), (360, 273)] {
        assert_eq!(edited.pixel(x, y), base.pixel(x, y), "({x}, {y})");
    }
    assert!(close_to(base.pixel(360, 240), ACCENT, 1));

    // Detection now sees no button where it was.
    let after = s.run("smart.detectObjects", &json!({})).unwrap();
    assert!(find(&after, "button").is_empty(), "{after:#}");

    // --- Undo the move, then Simplify the dialog.
    s.run("edit.undo", &Value::Null).unwrap();
    assert!(s.active().unwrap().manifest.annotations.is_empty());
    let r = s
        .run("smart.simplify", &json!({ "palette": "dark" }))
        .unwrap();
    assert_eq!(r["excluded"], json!([]));
    assert!(r["healed"].as_u64().unwrap() >= 4, "{r:#}");

    let shapes: Vec<(String, ShapeNode)> = s
        .active()
        .unwrap()
        .manifest
        .annotations
        .iter()
        .filter_map(|n| match n.annotation() {
            Some(Annotation::Shape(sh)) => Some((n.name.clone(), sh.clone())),
            _ => None,
        })
        .collect();
    let named = |name: &str| {
        shapes
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, s)| s)
            .collect::<Vec<_>>()
    };
    let fill = |s: &ShapeNode| match s.fill {
        Fill::Solid { color } => color,
        _ => panic!("solid fill expected"),
    };
    let dark = capturecraft_engine::SimplifyPalette::DARK;

    assert_eq!(shapes[0].0, "Simplified Background");
    assert_eq!(fill(&shapes[0].1), dark.background);

    // Card (outlined) and header: two panels, card first.
    let panels = named("Simplified Panel");
    assert_eq!(panels.len(), 2);
    assert!(panels[0].stroke.is_some(), "the card keeps its outline");
    assert_eq!(panels[0].shape.bounds().w, f64::from(CARD_R.width()));

    // The avatar is a circle.
    let icons = named("Simplified Icon");
    assert_eq!(icons.len(), 1);
    let RegionShape::Ellipse { rect } = icons[0].shape else {
        panic!("avatar should be an ellipse: {:?}", icons[0].shape);
    };
    assert!((rect.w - 44.0).abs() <= 2.0 && rect.w == rect.h);
    assert_eq!(fill(icons[0]), dark.accent);

    // The button is an accent rounded rectangle.
    let buttons = named("Simplified Button");
    assert_eq!(buttons.len(), 1);
    let RegionShape::RoundedRect { rect, radius } = buttons[0].shape else {
        panic!("button should be rounded: {:?}", buttons[0].shape);
    };
    assert_eq!((rect.w, rect.h), (120.0, 36.0));
    assert!((3.0..=10.0).contains(&radius));
    assert_eq!(fill(buttons[0]), dark.accent);

    // Text: title, two body lines and the label, one pill bar per line,
    // exactly as wide as the line, radius = height / 4.
    let bars = named("Simplified Text");
    assert_eq!(bars.len(), 4, "{bars:#?}");
    let bar_for = |r: Rect| {
        bars.iter()
            .find(|b| {
                (b.shape.bounds().x - f64::from(r.x0)).abs() <= 1.0
                    && (b.shape.bounds().y - f64::from(r.y0)).abs() <= 1.0
            })
            .unwrap_or_else(|| panic!("no bar at {r:?}: {bars:#?}"))
    };
    for line in [d.title, d.name, d.description] {
        let b = bar_for(line);
        let RegionShape::RoundedRect { rect, radius } = b.shape else {
            panic!("pill expected");
        };
        assert!((rect.w - f64::from(line.width())).abs() <= 1.0);
        assert!((f64::from(radius) - rect.h * 0.25).abs() < 1e-3);
        assert_eq!(fill(b), dark.muted_text);
    }
    assert_eq!(fill(bar_for(d.label)), dark.on_accent);

    // Behind the wireframe the old pixels are healed (the avatar area is
    // card fill in the Simplify layer, untouched in the capture).
    let doc = s.active().unwrap();
    let shown = doc.display_pixels().unwrap();
    assert!(close_to(shown.pixel(90, 120), CARD, 1));
    assert!(close_to(
        doc.pixels.as_ref().unwrap().pixel(90, 120),
        AVATAR,
        1
    ));

    // Keep the button as real pixels (the tutorial's subject).
    let r = s
        .run(
            "smart.setSimplified",
            &json!({ "objects": [id], "simplified": false }),
        )
        .unwrap();
    assert_eq!(
        r["excluded"].as_array().map(Vec::len),
        Some(2),
        "button and label"
    );
    let doc = s.active().unwrap();
    let originals: Vec<_> = doc
        .manifest
        .annotations
        .iter()
        .filter(|n| matches!(n.annotation(), Some(Annotation::SmartObject(_))))
        .collect();
    assert_eq!(originals.len(), 1);
    let px = doc.object_pixels(originals[0].id).unwrap();
    assert!(close_to(px.pixel(60, 2), ACCENT, 1));
    assert!(
        !doc.manifest
            .annotations
            .iter()
            .any(|n| n.name == "Simplified Button")
    );

    // Switching palettes recolours the same layout.
    s.run("smart.setPalette", &json!({ "palette": "high_contrast" }))
        .unwrap();
    let doc = s.active().unwrap();
    let hc = capturecraft_engine::SimplifyPalette::HIGH_CONTRAST;
    let icon = doc
        .manifest
        .annotations
        .iter()
        .find(|n| n.name == "Simplified Icon")
        .unwrap();
    let Some(Annotation::Shape(sh)) = icon.annotation() else {
        unreachable!()
    };
    assert_eq!(fill(sh), hc.accent);
}

#[test]
fn analysis_kinds_reach_the_engine() {
    let d = dialog();
    let mut s = session(&d);
    let found = s
        .run("smart.detectObjects", &json!({ "kinds": ["text"] }))
        .unwrap();
    let texts = found["objects"].as_array().unwrap();
    assert_eq!(texts.len(), 3);
    // Without models the lines are found but not read.
    let r = s.run("ocr.grabText", &json!({})).unwrap();
    assert_eq!(r["recognized"], false);
    assert_eq!(r["lines"].as_array().map(Vec::len), Some(3));
    // Edit Text on the body block: healed, replaced by a live text box.
    let body = texts
        .iter()
        .find(|t| t["lines"].as_array().map(Vec::len) == Some(2))
        .unwrap();
    let r = s
        .run(
            "ocr.replaceText",
            &json!({ "object": body["id"], "text": "Shown to everyone" }),
        )
        .unwrap();
    assert!((12.0..=18.0).contains(&r["font"]["size"].as_f64().unwrap()));
    let edited = s.active().unwrap().edited_pixels().unwrap();
    assert!(close_to(
        edited.pixel(d.name.x0 + 3, d.name.y0 + 5),
        CARD,
        1
    ));
    assert!(texts.iter().all(|t| t["kind"] == Kind::Text.name()));
}
