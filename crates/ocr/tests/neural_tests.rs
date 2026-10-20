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
