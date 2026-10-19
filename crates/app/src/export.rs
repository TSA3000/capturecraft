//! Share / Export and Copy All: the flattened document
//! (`capturecraft_engine::render::flatten`) encoded as PNG, JPEG or WebP
//! (photocraft-codecs), as an SVG holding that PNG, or as a PDF with one
//! capture per page (pdfcraft-create), written to the captures folder, or
//! put on the clipboard.
//!
//! Everything but the clipboard is plain logic, tested without a display.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use capturecraft_engine::render::Flattened;
use capturecraft_ui_egui::state::ImageFormat;
use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};

/// Where exports go when no folder is set: `Pictures/CaptureCraft`.
pub fn default_dir() -> PathBuf {
    dirs::picture_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CaptureCraft")
}

/// `dir/<stem>.<ext>`, adding ` (2)`, ` (3)`, … while the name is taken.
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let stem = sanitize(stem);
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    path
}

/// A file name stem without path separators or characters Windows rejects.
fn sanitize(stem: &str) -> String {
    let s: String = stem
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "Capture".into() } else { s }
}

fn image(flat: &Flattened, opaque: bool) -> Result<Image, String> {
    let result = if opaque {
        // JPEG has no alpha: composite onto white.
        let rgb = flat
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let a = u32::from(p[3]);
                let over = |c: u8| ((u32::from(c) * a + 255 * (255 - a) + 127) / 255) as u8;
                [over(p[0]), over(p[1]), over(p[2])]
            })
            .collect();
        Image::from_u8(flat.width, flat.height, ChannelLayout::Rgb, rgb)
    } else {
        Image::from_u8(
            flat.width,
            flat.height,
            ChannelLayout::Rgba,
            flat.rgba.clone(),
        )
    };
    result.map_err(|e| e.to_string())
}

/// One document as `format` (PDF: a single page).
pub fn encode(flat: &Flattened, format: ImageFormat) -> Result<Vec<u8>, String> {
    let raster = |f: Format, opaque: bool| {
        photocraft_codecs::encode(&image(flat, opaque)?, f, &EncodeOptions::default())
            .map_err(|e| e.to_string())
    };
    match format {
        ImageFormat::Png => raster(Format::Png, false),
        ImageFormat::Jpeg => raster(Format::Jpeg, true),
        ImageFormat::Webp => raster(Format::WebP, false),
        ImageFormat::Svg => Ok(svg(&raster(Format::Png, false)?, flat.width, flat.height)),
        ImageFormat::Pdf => pdf(std::slice::from_ref(flat)),
        ImageFormat::Avif | ImageFormat::Gif => {
            Err(format!("{} export is not available yet", format.label()))
        }
    }
}

/// An SVG of the given size showing `png`.
pub fn svg(png: &[u8], width: u32, height: u32) -> Vec<u8> {
    let data = base64::engine::general_purpose::STANDARD.encode(png);
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" \
         width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">\
         <image width=\"{width}\" height=\"{height}\" href=\"data:image/png;base64,{data}\"/></svg>\n"
    )
    .into_bytes()
}

/// A PDF with one page per capture, one point per pixel.
pub fn pdf(pages: &[Flattened]) -> Result<Vec<u8>, String> {
    if pages.is_empty() {
        return Err("nothing to export".into());
    }
    let images = pages
        .iter()
        .enumerate()
        .map(|(i, p)| {
            Ok((
                format!("capture-{}.png", i + 1),
                encode(p, ImageFormat::Png)?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let doc = pdfcraft_create::from_images_with_resolution(
        &images,
        pdfcraft_create::ImageResolution::Dpi(72.0),
    )
    .map_err(|e| e.to_string())?;
    // A classic cross-reference table: every reader opens it.
    let opts = pdfcraft_cos::SaveOptions {
        object_streams: false,
        ..Default::default()
    };
    pdfcraft_cos::write_full(&doc, &opts).map_err(|e| e.to_string())
}

/// Writes `bytes` to a new file in `dir`; returns its path.
pub fn write_new(
    dir: &Path,
    stem: &str,
    format: ImageFormat,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = unique_path(dir, stem, format.extension());
    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Puts `flat` on the clipboard. On Linux the clipboard empties when its
/// owner exits, so the caller keeps `clipboard` alive.
pub fn copy(clipboard: &mut Option<arboard::Clipboard>, flat: &Flattened) -> Result<(), String> {
    if clipboard.is_none() {
        *clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    let Some(cb) = clipboard.as_mut() else {
        return Err("no clipboard".into());
    };
    cb.set_image(arboard::ImageData {
        width: flat.width as usize,
        height: flat.height as usize,
        bytes: std::borrow::Cow::Borrowed(&flat.rgba),
    })
    .map_err(|e| e.to_string())
}

/// Puts `text` on the clipboard (Grab Text's Copy Text).
pub fn copy_text(clipboard: &mut Option<arboard::Clipboard>, text: &str) -> Result<(), String> {
    if clipboard.is_none() {
        *clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    let Some(cb) = clipboard.as_mut() else {
        return Err("no clipboard".into());
    };
    cb.set_text(text).map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, px: [u8; 4]) -> Flattened {
        Flattened {
            width: w,
            height: h,
            rgba: px.repeat((w * h) as usize),
        }
    }

    #[test]
    fn rasters_have_their_signatures() {
        let f = flat(8, 6, [10, 200, 30, 255]);
        let png = encode(&f, ImageFormat::Png).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let jpg = encode(&f, ImageFormat::Jpeg).unwrap();
        assert_eq!(&jpg[..3], &[0xFF, 0xD8, 0xFF]);
        let webp = encode(&f, ImageFormat::Webp).unwrap();
        assert_eq!((&webp[..4], &webp[8..12]), (&b"RIFF"[..], &b"WEBP"[..]));
        assert!(encode(&f, ImageFormat::Gif).is_err());
    }

    #[test]
    fn png_round_trips_with_alpha() {
        let f = flat(3, 2, [255, 0, 0, 128]);
        let png = encode(&f, ImageFormat::Png).unwrap();
        let back = photocraft_codecs::decode(&png).unwrap();
        assert_eq!(back.dimensions(), (3, 2));
        assert_eq!(back.layout(), ChannelLayout::Rgba);
        assert_eq!(back.as_u8().unwrap(), f.rgba.as_slice());
    }

    #[test]
    fn jpeg_flattens_alpha_onto_white() {
        let img = image(&flat(1, 1, [0, 0, 0, 0]), true).unwrap();
        assert_eq!(img.as_u8().unwrap(), &[255, 255, 255]);
        let img = image(&flat(1, 1, [0, 0, 0, 255]), true).unwrap();
        assert_eq!(img.as_u8().unwrap(), &[0, 0, 0]);
    }

    #[test]
    fn svg_embeds_the_png_at_its_size() {
        let svg = String::from_utf8(encode(&flat(4, 3, [1, 2, 3, 255]), ImageFormat::Svg).unwrap())
            .unwrap();
        assert!(
            svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"3\"")
        );
        assert!(svg.contains("href=\"data:image/png;base64,iVBORw0KGgo"));
    }

    #[test]
    fn pdf_has_a_page_per_capture() {
        let bytes = pdf(&[flat(20, 10, [255; 4]), flat(10, 30, [0, 0, 0, 255])]).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Count 2"), "two pages");
        assert!(pdf(&[]).is_err());
    }

    #[test]
    fn names_are_unique_and_safe() {
        let dir = std::env::temp_dir().join(format!("capturecraft-export-{}", std::process::id()));
        let a = write_new(&dir, "Capture 1/2: a?", ImageFormat::Png, b"x").unwrap();
        let b = write_new(&dir, "Capture 1/2: a?", ImageFormat::Png, b"y").unwrap();
        assert_eq!(a.file_name().unwrap(), "Capture 1-2- a-.png");
        assert_eq!(b.file_name().unwrap(), "Capture 1-2- a- (2).png");
        assert_eq!(sanitize(" .. "), "Capture");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
