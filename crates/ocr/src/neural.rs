//! Text recognition with storytold/pdfcraft's `pdfcraft-ocr`, the recogniser
//! behind PdfCraft's Scan & OCR: the ocrs engine (MIT/Apache-2.0) and its
//! pre-trained detection and recognition models (CC-BY-SA-4.0), run by the
//! pure-Rust `rten` runtime. The models read the Latin alphabet.

use photocraft_geom::Rect;

use crate::image::RgbaImage;
use crate::models::Models;
use crate::{OcrEngine, OcrError, TextLine, Word};

/// The recognition model's alphabet in training order.
pub use pdfcraft_ocr::ALPHABET;

/// A loaded recogniser. Loading takes a moment; keep one and reuse it.
pub struct NeuralEngine {
    ocr: pdfcraft_ocr::Ocr,
}

impl NeuralEngine {
    pub fn load(models: &Models) -> Result<Self, OcrError> {
        let models = pdfcraft_ocr::Models {
            detection: models.detection.clone(),
            recognition: models.recognition.clone(),
        };
        let ocr = pdfcraft_ocr::Ocr::load(&models).map_err(|e| match e {
            pdfcraft_ocr::OcrError::Load(p, e) => OcrError::Load(p, e),
            e => OcrError::Load("ocr engine".into(), e.to_string()),
        })?;
        Ok(Self { ocr })
    }
}

impl OcrEngine for NeuralEngine {
    fn name(&self) -> &'static str {
        "ocrs"
    }

    fn reads_text(&self) -> bool {
        true
    }

    fn recognize(&self, img: &RgbaImage) -> Result<Vec<TextLine>, OcrError> {
        let (w, h) = (img.width() as u32, img.height() as u32);
        if w == 0 || h == 0 {
            return Err(OcrError::Empty);
        }
        let read = self
            .ocr
            .recognize(&img.to_rgb8(), w, h)
            .map_err(|e| OcrError::Recognize(e.to_string()))?;
        Ok(read
            .into_iter()
            .filter_map(|line| {
                let words: Vec<Word> = line
                    .words
                    .into_iter()
                    .map(|w| {
                        let [x0, y0, x1, y1] = w.rect;
                        Word {
                            rect: Rect::new(
                                x0.floor() as i32,
                                y0.floor() as i32,
                                x1.ceil() as i32,
                                y1.ceil() as i32,
                            ),
                            text: w.text,
                        }
                    })
                    .collect();
                let r = words.iter().map(|w| w.rect).reduce(|a, b| a.union(&b))?;
                let text = words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                Some(TextLine {
                    rect: r,
                    text: Some(text),
                    words,
                })
            })
            .collect())
    }
}
