//! The Library's files: the image files in the captures folders (where
//! Share › File, the toolbar's Share and the tray app save captures),
//! listed next to the open documents. Thumbnails are decoded on a worker
//! thread so a folder of large captures never stalls the editor.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use capturecraft_engine::render::Flattened;
use capturecraft_ui_egui::panels::recent_tray::Thumbnail;
use capturecraft_ui_egui::state::{AppState, ShareDestination, TrayItem};
use capturecraft_ui_egui::views::library_view::file_item;

use crate::{app::share_dir, documents, export};

/// Extensions the Library lists (lower case).
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff", "qoi",
];

/// Files kept per scan, newest first.
pub const MAX_FILES: usize = 500;

/// How often an open Library looks for new files.
const RESCAN: Duration = Duration::from_secs(3);

/// The tray app's `output_dir` from `<config dir>/CaptureCraft/config.toml`.
pub fn tray_output_dir() -> Option<PathBuf> {
    let path = dirs::config_dir()?.join("CaptureCraft").join("config.toml");
    output_dir_from(&std::fs::read_to_string(path).ok()?)
}

/// `output_dir` of a tray `config.toml`, `None` when unset or unreadable.
pub fn output_dir_from(config: &str) -> Option<PathBuf> {
    let table: toml::Table = config.parse().ok()?;
    let dir = table.get("output_dir")?.as_str()?.trim();
    (!dir.is_empty()).then(|| PathBuf::from(dir))
}

/// The folders the Library lists: the export default, the tray app's
/// output folder and every HUD tab's Share › File folder, each once.
pub fn library_dirs(state: &AppState, tray_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let hud = &state.hud;
    let shared = [&hud.all_in_one, &hud.image, &hud.video.capture]
        .into_iter()
        .filter_map(|s| match &s.share {
            ShareDestination::File { directory, .. } => Some(share_dir(directory)),
            _ => None,
        });
    let mut dirs: Vec<PathBuf> = Vec::new();
    for d in std::iter::once(export::default_dir())
        .chain(tray_dir)
        .chain(shared)
    {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The image files directly in `dirs` (not their subfolders), newest
/// first, at most [`MAX_FILES`]. Missing folders are skipped.
pub fn scan(dirs: &[PathBuf]) -> Vec<TrayItem> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let mut seen = HashSet::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_file() && is_image(&path) && seen.insert(path.clone()) {
                let modified = md.modified().unwrap_or(std::time::UNIX_EPOCH);
                files.push((modified, path));
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    files.truncate(MAX_FILES);
    files
        .into_iter()
        .map(|(t, p)| {
            let stamp = chrono::DateTime::<chrono::Local>::from(t).to_rfc3339();
            file_item(&p, stamp)
        })
        .collect()
}

/// A thumbnail of the image at `path`: width, height, straight RGBA8.
pub fn file_thumbnail(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    let img = photocraft_codecs::decode(&bytes).ok()?;
    let flat = Flattened {
        width: img.width(),
        height: img.height(),
        rgba: img.to_rgba8(),
    };
    documents::thumbnail(&flat)
}

type Job = (String, PathBuf);
type Done = (String, Option<(u32, u32, Vec<u8>)>);

/// The Library's file entries and their thumbnails.
#[derive(Default)]
pub struct LibraryFiles {
    pub items: Vec<TrayItem>,
    scanned: Option<Instant>,
    /// By doc id, with the `captured_at` (modification time) drawn from.
    thumbs: HashMap<String, (String, Thumbnail, egui::TextureHandle)>,
    /// Doc ids sent to the worker and not back yet.
    pending: HashSet<String>,
    worker: Option<(mpsc::Sender<Job>, mpsc::Receiver<Done>)>,
}

impl LibraryFiles {
    /// While the Library is open: rescans `dirs` every few seconds, asks
    /// the worker for missing thumbnails and uploads finished ones.
    pub fn refresh(&mut self, ctx: &egui::Context, dirs: &[PathBuf], visible: bool) {
        if !visible {
            self.scanned = None;
            return;
        }
        if self.scanned.is_none_or(|t| t.elapsed() >= RESCAN) {
            self.items = scan(dirs);
            self.scanned = Some(Instant::now());
            let live: HashSet<&str> = self.items.iter().map(|i| i.doc_id.as_str()).collect();
            self.thumbs.retain(|id, _| live.contains(id.as_str()));
        }
        if let Some((_, rx)) = &self.worker {
            for (id, thumb) in rx.try_iter().collect::<Vec<_>>() {
                self.pending.remove(&id);
                let Some((w, h, rgba)) = thumb else { continue };
                let Some(item) = self.items.iter().find(|i| i.doc_id == id) else {
                    continue;
                };
                let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                let tex = ctx.load_texture(
                    format!("capturecraft.library.{id}"),
                    img,
                    egui::TextureOptions::LINEAR,
                );
                let th = Thumbnail {
                    texture: tex.id(),
                    size: [w, h],
                };
                self.thumbs.insert(id, (item.captured_at.clone(), th, tex));
            }
        }
        let wanted: Vec<Job> = self
            .items
            .iter()
            .filter(|i| {
                !self.pending.contains(&i.doc_id)
                    && self
                        .thumbs
                        .get(&i.doc_id)
                        .is_none_or(|(at, ..)| *at != i.captured_at)
            })
            .filter_map(|i| Some((i.doc_id.clone(), PathBuf::from(i.file_path.as_ref()?))))
            .collect();
        if wanted.is_empty() {
            return;
        }
        let (tx, _) = self.worker.get_or_insert_with(|| spawn_worker(ctx.clone()));
        for job in wanted {
            self.pending.insert(job.0.clone());
            if tx.send(job).is_err() {
                // The worker is gone (it panicked): start over next frame.
                self.worker = None;
                self.pending.clear();
                return;
            }
        }
    }

    pub fn thumbnail(&self, doc_id: &str) -> Option<Thumbnail> {
        self.thumbs.get(doc_id).map(|(_, th, _)| *th)
    }
}

/// One thread decoding thumbnails in request order; it ends when the
/// sender is dropped.
fn spawn_worker(ctx: egui::Context) -> (mpsc::Sender<Job>, mpsc::Receiver<Done>) {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (done_tx, done_rx) = mpsc::channel::<Done>();
    std::thread::spawn(move || {
        for (id, path) in job_rx {
            if done_tx.send((id, file_thumbnail(&path))).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    (job_tx, done_rx)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use capturecraft_ui_egui::state::ImageFormat;
    use capturecraft_ui_egui::views::library_view::file_path_of;

    #[test]
    fn scans_images_newest_first_and_skips_the_rest() {
        let dir = std::env::temp_dir().join(format!("capturecraft-lib-{}", std::process::id()));
        let other = dir.join("more");
        std::fs::create_dir_all(&other).unwrap();
        let flat = Flattened {
            width: 4,
            height: 3,
            rgba: [10, 20, 30, 255].repeat(12),
        };
        let png = export::encode(&flat, ImageFormat::Png).unwrap();
        std::fs::write(dir.join("old.png"), &png).unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(other.join("New.PNG"), &png).unwrap();
        let dirs = [dir.clone(), other.clone(), dir.clone(), dir.join("missing")];
        let items = scan(&dirs);
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["New", "old"]);
        let path = file_path_of(&items[1].doc_id).unwrap();
        let (w, h, rgba) = file_thumbnail(Path::new(path)).unwrap();
        assert_eq!((w, h), (4, 3));
        assert_eq!(&rgba[..4], &[10, 20, 30, 255]);
        assert!(file_thumbnail(&dir.join("notes.txt")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folders_come_from_the_export_default_tray_config_and_share() {
        assert_eq!(
            output_dir_from("clipboard = true\noutput_dir = \"/shots\"\n"),
            Some(PathBuf::from("/shots"))
        );
        assert_eq!(output_dir_from("output_dir = \"\""), None);
        assert_eq!(output_dir_from("not toml ["), None);
        let mut state = AppState::default();
        state.hud.image.share = ShareDestination::File {
            directory: "/elsewhere".into(),
            name_pattern: String::new(),
            format: ImageFormat::Png,
        };
        let dirs = library_dirs(&state, Some(export::default_dir()));
        assert_eq!(dirs, [export::default_dir(), PathBuf::from("/elsewhere")]);
    }
}
