//! Text recognition with the ocrs engine (MIT/Apache-2.0) and its
//! pre-trained detection and recognition models (CC-BY-SA-4.0), run by the
//! pure-Rust `rten` runtime: the same setup as storytold/pdfcraft's
//! `pdfcraft-ocr`. The models read the Latin alphabet.

use photocraft_geom::Rect;

use crate::image::RgbaImage;
use crate::models::Models;
use crate::{OcrEngine, OcrError, TextLine, Word};

/// The recognition model's alphabet in training order (ocrs-models'
/// `DEFAULT_ALPHABET`; the ocrs crate's copy has `E` where training had `€`).
pub const ALPHABET: &str = " 0123456789!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~€ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// A loaded recogniser. Loading takes a moment; keep one and reuse it.
pub struct NeuralEngine {
    engine: ocrs::OcrEngine,
}

impl NeuralEngine {
    pub fn load(models: &Models) -> Result<Self, OcrError> {
        let load = |p: &std::path::Path| {
            rten::Model::load_file(p)
                .map_err(|e| OcrError::Load(p.display().to_string(), e.to_string()))
        };
        let params = ocrs::OcrEngineParams {
            detection_model: Some(load(&models.detection)?),
            recognition_model: Some(load(&models.recognition)?),
            alphabet: Some(ALPHABET.into()),
            ..Default::default()
        };
        let engine = ocrs::OcrEngine::new(params)
            .map_err(|e| OcrError::Load("ocr engine".into(), e.to_string()))?;
        Ok(Self { engine })
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
        let rgb = img.to_rgb8();
        let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
        let source = ocrs::ImageSource::from_bytes(&rgb, (w, h)).map_err(|e| err(&e))?;
        let input = self.engine.prepare_input(source).map_err(|e| err(&e))?;
        let found = self.engine.detect_words(&input).map_err(|e| err(&e))?;
        let lines = self.engine.find_text_lines(&input, &found);
        let read = self
            .engine
            .recognize_text(&input, &lines)
            .map_err(|e| err(&e))?;
        use ocrs::TextItem;
        Ok(read
            .into_iter()
            .flatten()
            .filter_map(|line| {
                let words: Vec<Word> = line
                    .words()
                    .map(|w| {
                        let r = w.bounding_rect();
                        Word {
                            rect: Rect::new(
                                (r.left() as f32).floor() as i32,
                                (r.top() as f32).floor() as i32,
                                (r.right() as f32).ceil() as i32,
                                (r.bottom() as f32).ceil() as i32,
                            ),
                            text: w.to_string(),
                        }
                    })
                    .filter(|w| !w.text.trim().is_empty())
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
