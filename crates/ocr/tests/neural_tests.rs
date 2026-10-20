//! Real text recognition: text rendered with PhotoCraft's bundled font must
//! read back. Needs the models (`cargo xtask models`); the release workflow
//! fetches them and runs this test before packaging.
#![cfg(feature = "neural")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use capturecraft_ocr::models::Models;
use capturecraft_ocr::neural::NeuralEngine;
use capturecraft_ocr::{OcrEngine, RgbaImage};
use photocraft_color::PixelFormat;
use photocraft_doc::TextLayer;
use photocraft_geom::{Affine, Rect};
use photocraft_text::TextEngine;

/// Black `text` at 28 px on white, with a margin of at least 24 px.
fn render(text: &str) -> RgbaImage {
    let layer = TextLayer {
        text: text.into(),
        size_pt: 28.0,
        transform: Affine::translate(24.0_f64, 64.0),
        ..Default::default()
    };
    let (_, r) = TextEngine::new().render(&layer, 72.0, PixelFormat::RGBA8);
    let glyphs = RgbaImage::from_surface(&r.surface, r.rect).unwrap();
    let (w, h) = (r.rect.x1 + 24, r.rect.y1 + 24);
    let mut rgba = vec![255u8; (w * h * 4) as usize];
    for y in r.rect.y0..r.rect.y1 {
        for x in r.rect.x0..r.rect.x1 {
            let a = u32::from(glyphs.get(x - r.rect.x0, y - r.rect.y0)[3]);
            let i = ((y * w + x) * 4) as usize;
            for c in &mut rgba[i..i + 3] {
                *c = (255 * (255 - a) / 255) as u8;
            }
        }
    }
    RgbaImage::from_rgba8(Rect::new(0, 0, w, h), &rgba).unwrap()
}

#[test]
#[ignore = "needs the text recognition models (cargo xtask models)"]
fn reads_rendered_text() {
    let models = Models::find().expect("models not found; run `cargo xtask models`");
    models.verify().unwrap();
    let engine = NeuralEngine::load(&models).unwrap();
    assert!(engine.reads_text());
    let lines = engine.recognize(&render("Grab Text works 2026")).unwrap();
    let text: Vec<_> = lines.iter().filter_map(|l| l.text.clone()).collect();
    assert_eq!(text.join("\n").trim(), "Grab Text works 2026", "{lines:?}");
}

/// Light-on-dark `lines` (a terminal) at 16 px, `w` × `h`.
fn terminal(lines: &[&str], w: i32, h: i32) -> Vec<u8> {
    let mut rgba = [0x1E, 0x1E, 0x1E, 0xFF].repeat((w * h) as usize);
    let mut engine = TextEngine::new();
    for (i, text) in lines.iter().enumerate() {
        let layer = TextLayer {
            text: (*text).into(),
            size_pt: 16.0,
            transform: Affine::translate(12.0_f64, 24.0 + 22.0 * i as f64),
            ..Default::default()
        };
        let (_, r) = engine.render(&layer, 72.0, PixelFormat::RGBA8);
        let glyphs = RgbaImage::from_surface(&r.surface, r.rect).unwrap();
        for y in r.rect.y0.max(0)..r.rect.y1.min(h) {
            for x in r.rect.x0.max(0)..r.rect.x1.min(w) {
                let a = u32::from(glyphs.get(x - r.rect.x0, y - r.rect.y0)[3]);
                let i = ((y * w + x) * 4) as usize;
                for c in &mut rgba[i..i + 3] {
                    *c = ((u32::from(*c) * (255 - a) + 0xDD * a) / 255) as u8;
                }
            }
        }
    }
    rgba
}

/// Grab Text over a dragged region, then Replace as Editable Text: the
/// replacement reuses the region's analysis instead of reading the whole
/// capture again (that froze the editor for seconds).
#[test]
#[ignore = "needs the text recognition models (cargo xtask models)"]
fn replaces_grabbed_lines_without_reading_again() {
    use capturecraft_engine::{Pixels, Session};
    use capturecraft_format::Manifest;
    use capturecraft_ocr::Analyzer;
    use photocraft_geom::Size;
    use serde_json::json;

    let text: Vec<String> = (0..26)
        .map(|i| format!("Compiling crate-{i} v0.{i}.7"))
        .collect();
    let lines: Vec<&str> = text.iter().map(String::as_str).collect();
    let (w, h) = (1115, 628);
    let size = Size::new(w as u32, h as u32);
    let mut s = Session::new();
    s.open(
        Manifest::new(size),
        Pixels::new(size, terminal(&lines, w, h)),
    )
    .unwrap();
    s.set_analyzer(Box::new(Analyzer::auto()));

    let region = json!({ "x0": 0, "y0": 0, "x1": 600, "y1": 600 });
    let g = s.run("ocr.grabText", &json!({ "region": region })).unwrap();
    assert!(
        g["text"]
            .as_str()
            .unwrap()
            .contains("Compiling crate-3 v0.3.7"),
        "{g}"
    );
    let t = std::time::Instant::now();
    let r = s
        .run(
            "ocr.replaceLines",
            &json!({ "lines": g["lines"], "region": g["region"] }),
        )
        .unwrap();
    let took = t.elapsed();
    assert!(!r["replaced"].as_array().unwrap().is_empty(), "{r}");
    // Healing only; reading the capture again takes seconds.
    assert!(
        took < std::time::Duration::from_millis(1500),
        "replace took {took:?}"
    );
}
