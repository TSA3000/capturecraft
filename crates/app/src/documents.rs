//! Documents the app opens besides captures: image files from the command
//! line (the tray's Open editor), blank canvases (Create › New Blank
//! Image) and copies; plus the Recent Captures thumbnails.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use capturecraft_engine::render::Flattened;
use capturecraft_engine::{Pixels, Session};
use capturecraft_format::Manifest;
use photocraft_geom::Size;

/// Longest side of a tray thumbnail, pixels (two points a pixel on HiDPI).
pub const THUMB_MAX: [u32; 2] = [260, 170];

/// A document id no open document has: `doc-<unix ms>-<n>`.
pub fn unique_doc_id(session: &Session) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    loop {
        let id = format!("doc-{ms}-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        if !session.documents().iter().any(|d| d.manifest.id == id) {
            return id;
        }
    }
}

/// Opens `path` (PNG, JPEG, WebP, …) as a new active document titled
/// after the file.
pub fn open_image(session: &mut Session, path: &Path) -> Result<usize, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let img = photocraft_codecs::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let size = Size::new(img.width(), img.height());
    let pixels = Pixels::new(size, img.to_rgba8())
        .ok_or_else(|| format!("{}: not an RGBA image", path.display()))?;
    let mut m = Manifest::new(size);
    m.id = unique_doc_id(session);
    m.title = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    m.capture.captured_at = std::fs::metadata(path)
        .and_then(|md| md.modified())
        .map(|t| chrono::DateTime::<chrono::Local>::from(t).to_rfc3339())
        .unwrap_or_else(|_| chrono::Local::now().to_rfc3339());
    session.open(m, Some(pixels)).map_err(|e| e.to_string())
}

/// A white `size` canvas as a new active document.
pub fn open_blank(session: &mut Session, size: Size) -> Result<usize, String> {
    let n = size.width as usize * size.height as usize;
    let pixels = Pixels::new(size, vec![255; n * 4]).ok_or("bad canvas size")?;
    let mut m = Manifest::new(size);
    m.id = unique_doc_id(session);
    m.title = "Untitled".into();
    m.capture.captured_at = chrono::Local::now().to_rfc3339();
    session.open(m, Some(pixels)).map_err(|e| e.to_string())
}

/// Index of the open document `id`.
pub fn index_of(session: &Session, id: &str) -> Option<usize> {
    session.documents().iter().position(|d| d.manifest.id == id)
}

/// A box-filtered copy of `flat` that fits [`THUMB_MAX`]; `None` when it
/// is empty. Returns width, height and straight RGBA8.
pub fn thumbnail(flat: &Flattened) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = (flat.width, flat.height);
    if w == 0 || h == 0 {
        return None;
    }
    let s = (f64::from(THUMB_MAX[0]) / f64::from(w))
        .min(f64::from(THUMB_MAX[1]) / f64::from(h))
        .min(1.0);
    let tw = ((f64::from(w) * s).round() as u32).max(1);
    let th = ((f64::from(h) * s).round() as u32).max(1);
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for ty in 0..th {
        let y0 = u64::from(ty) * u64::from(h) / u64::from(th);
        let y1 = (u64::from(ty + 1) * u64::from(h) / u64::from(th)).max(y0 + 1);
        for tx in 0..tw {
            let x0 = u64::from(tx) * u64::from(w) / u64::from(tw);
            let x1 = (u64::from(tx + 1) * u64::from(w) / u64::from(tw)).max(x0 + 1);
            let mut acc = [0u64; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y * u64::from(w) + x) * 4) as usize;
                    let px = flat.rgba.get(i..i + 4)?;
                    // Premultiply so transparent pixels do not darken edges.
                    let a = u64::from(px[3]);
                    acc[0] += u64::from(px[0]) * a;
                    acc[1] += u64::from(px[1]) * a;
                    acc[2] += u64::from(px[2]) * a;
                    acc[3] += a;
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            let a = acc[3];
            let c = |v: u64| v.checked_div(a).map_or(0, |c| c as u8);
            out.extend_from_slice(&[c(acc[0]), c(acc[1]), c(acc[2]), (a / n) as u8]);
        }
    }
    Some((tw, th, out))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_blank_images_open() {
        let mut s = Session::new();
        let a = open_blank(&mut s, Size::new(30, 20)).unwrap();
        let b = open_blank(&mut s, Size::new(30, 20)).unwrap();
        let docs = s.documents();
        assert_ne!(docs[a].manifest.id, docs[b].manifest.id);
        assert!(docs[a].manifest.id.starts_with("doc-"));
        assert_eq!(docs[b].pixels.as_ref().unwrap().pixel(3, 3), [255; 4]);
        assert_eq!(index_of(&s, &docs[b].manifest.id), Some(1));
    }

    #[test]
    fn png_files_open_titled_after_the_file() {
        let dir = std::env::temp_dir().join(format!("capturecraft-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let flat = Flattened {
            width: 4,
            height: 3,
            rgba: [10, 20, 30, 255].repeat(12),
        };
        let png =
            crate::export::encode(&flat, capturecraft_ui_egui::state::ImageFormat::Png).unwrap();
        let path = dir.join("login bug.png");
        std::fs::write(&path, png).unwrap();
        let mut s = Session::new();
        let i = open_image(&mut s, &path).unwrap();
        let d = &s.documents()[i];
        assert_eq!(d.manifest.title, "login bug");
        assert_eq!(d.manifest.canvas.size, Size::new(4, 3));
        assert_eq!(d.pixels.as_ref().unwrap().pixel(1, 1), [10, 20, 30, 255]);
        assert!(open_image(&mut s, &dir.join("missing.png")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn thumbnails_fit_the_card_and_average() {
        let flat = Flattened {
            width: 1040,
            height: 340,
            rgba: [0, 0, 255, 255].repeat(1040 * 340),
        };
        let (w, h, px) = thumbnail(&flat).unwrap();
        assert_eq!((w, h), (260, 85));
        assert_eq!(px.len(), (w * h * 4) as usize);
        assert_eq!(&px[..4], &[0, 0, 255, 255]);
        let small = Flattened {
            width: 2,
            height: 2,
            rgba: vec![255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255, 0, 0, 0, 0],
        };
        let (w, h, px) = thumbnail(&small).unwrap();
        assert_eq!((w, h), (2, 2), "never enlarged");
        assert_eq!(&px[4..8], &[0, 0, 0, 0]);
        assert!(
            thumbnail(&Flattened {
                width: 0,
                height: 0,
                rgba: vec![]
            })
            .is_none()
        );
    }
}
