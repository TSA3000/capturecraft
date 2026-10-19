//! Native Open and Save dialogs, the way PhotoCraft's `services.rs` runs
//! them: the `rfd::AsyncFileDialog` is built on the main thread (with the
//! editor window as its parent), a "file dialog" thread waits for the
//! answer and wakes the UI, and the frame loop polls for it, so the editor
//! never blocks. One dialog at a time; Cancel is not an error.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// File extension of CaptureCraft documents.
pub const DOC_EXT: &str = "capturecraft";

/// What Open lists besides documents.
pub const OPEN_IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff", "qoi",
];

/// Save As filters, in order: the document first (so it is the default),
/// then flattened image exports.
pub const SAVE_FILTERS: &[(&str, &[&str])] = &[
    ("CaptureCraft document", &[DOC_EXT]),
    ("PNG image", &["png"]),
    ("JPEG image", &["jpg", "jpeg"]),
    ("WebP image", &["webp"]),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Open { dir: Option<PathBuf> },
    Save { doc_id: String, suggested: PathBuf },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Open(Vec<PathBuf>),
    Save { doc_id: String, path: PathBuf },
}

/// A queued request and, once shown, the answer channel.
#[derive(Default)]
pub struct FileDialogs {
    queued: Option<Request>,
    pending: Option<(Request, Receiver<Option<Vec<PathBuf>>>)>,
}

/// Waits for `future` on this thread (PhotoCraft's `block_on`: park until
/// woken, spurious wake-ups just poll again).
fn block_on<T>(future: impl Future<Output = T>) -> T {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let std::task::Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

/// `name` with `ext` unless it already ends in one of `allowed`.
pub fn with_extension(path: &Path, allowed: &[&str], ext: &str) -> PathBuf {
    let has = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| allowed.iter().any(|a| a.eq_ignore_ascii_case(e)));
    if has {
        path.to_path_buf()
    } else {
        let mut s = path.as_os_str().to_owned();
        s.push(".");
        s.push(ext);
        PathBuf::from(s)
    }
}

/// Case-insensitive globs for GTK/portal filters on Linux (`*.[pP][nN][gG]`).
fn filter_exts(exts: &[&str]) -> Vec<String> {
    if cfg!(target_os = "linux") {
        exts.iter()
            .map(|e| {
                e.chars()
                    .map(|c| {
                        if c.is_ascii_alphabetic() {
                            format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
                        } else {
                            c.to_string()
                        }
                    })
                    .collect()
            })
            .collect()
    } else {
        exts.iter().map(|e| (*e).to_string()).collect()
    }
}

impl FileDialogs {
    /// Queues `r`; it is shown on the next [`FileDialogs::poll`]. `false`
    /// when a dialog is already open.
    pub fn ask(&mut self, r: Request) -> bool {
        if self.queued.is_some() || self.pending.is_some() {
            return false;
        }
        self.queued = Some(r);
        true
    }

    /// Shows a queued dialog over `parent` and returns a finished answer
    /// (`None` while waiting, or when cancelled).
    pub fn poll<W>(&mut self, ctx: &egui::Context, parent: Option<&W>) -> Option<Answer>
    where
        W: raw_window_handle::HasWindowHandle + raw_window_handle::HasDisplayHandle,
    {
        if let Some(r) = self.queued.take() {
            let mut dialog = rfd::AsyncFileDialog::new();
            if let Some(p) = parent {
                dialog = dialog.set_parent(p);
            }
            let picked: std::pin::Pin<Box<dyn Future<Output = Option<Vec<PathBuf>>> + Send>> =
                match &r {
                    Request::Open { dir } => {
                        let mut all: Vec<&str> = vec![DOC_EXT];
                        all.extend(OPEN_IMAGE_EXTS);
                        dialog = dialog
                            .set_title("Open")
                            .add_filter("CaptureCraft documents and images", &filter_exts(&all))
                            .add_filter("CaptureCraft document", &filter_exts(&[DOC_EXT]));
                        if let Some(d) = dir {
                            dialog = dialog.set_directory(d);
                        }
                        let f = dialog.pick_files();
                        Box::pin(async move {
                            f.await
                                .map(|v| v.into_iter().map(|h| h.path().to_path_buf()).collect())
                        })
                    }
                    Request::Save { suggested, .. } => {
                        dialog = dialog.set_title("Save As");
                        for (name, exts) in SAVE_FILTERS {
                            dialog = dialog.add_filter(*name, &filter_exts(exts));
                        }
                        if let Some(d) = suggested.parent().filter(|d| !d.as_os_str().is_empty()) {
                            dialog = dialog.set_directory(d);
                        }
                        if let Some(n) = suggested.file_name() {
                            dialog = dialog.set_file_name(n.to_string_lossy());
                        }
                        let f = dialog.save_file();
                        Box::pin(async move { f.await.map(|h| vec![h.path().to_path_buf()]) })
                    }
                };
            let (tx, rx) = mpsc::channel();
            let wake = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("file dialog".into())
                .spawn(move || {
                    let _ = tx.send(block_on(picked));
                    wake.request_repaint();
                });
            if spawned.is_ok() {
                self.pending = Some((r, rx));
            }
        }
        let (r, rx) = self.pending.as_ref()?;
        let answer = match rx.try_recv() {
            Err(TryRecvError::Empty) => return None,
            Ok(a) => a,
            Err(TryRecvError::Disconnected) => None,
        };
        let r = r.clone();
        self.pending = None;
        let paths = answer?;
        match r {
            Request::Open { .. } => (!paths.is_empty()).then_some(Answer::Open(paths)),
            Request::Save { doc_id, .. } => {
                let path = paths.into_iter().next()?;
                // A name typed without an extension is a document.
                let all: Vec<&str> = SAVE_FILTERS
                    .iter()
                    .flat_map(|(_, e)| e.iter().copied())
                    .collect();
                Some(Answer::Save {
                    doc_id,
                    path: with_extension(&path, &all, DOC_EXT),
                })
            }
        }
    }
}

/// Writes `bytes` to `path` atomically: a temporary file next to it,
/// then a rename, so a failed save never leaves half a document.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".into());
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn names_get_the_document_extension_unless_they_have_a_known_one() {
        let all: Vec<&str> = SAVE_FILTERS
            .iter()
            .flat_map(|(_, e)| e.iter().copied())
            .collect();
        assert_eq!(
            with_extension(Path::new("/a/Login bug"), &all, DOC_EXT),
            PathBuf::from("/a/Login bug.capturecraft")
        );
        assert_eq!(
            with_extension(Path::new("/a/x.PNG"), &all, DOC_EXT),
            PathBuf::from("/a/x.PNG")
        );
        assert_eq!(
            with_extension(Path::new("/a/v1.2"), &all, DOC_EXT),
            PathBuf::from("/a/v1.2.capturecraft")
        );
    }

    #[test]
    fn writes_are_atomic_and_one_dialog_at_a_time() {
        let dir = std::env::temp_dir().join(format!("capturecraft-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("doc.capturecraft");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "no temporary left"
        );
        assert!(atomic_write(&dir.join("missing/x"), b"x").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
        let mut d = FileDialogs::default();
        assert!(d.ask(Request::Open { dir: None }));
        assert!(!d.ask(Request::Open { dir: None }));
    }

    #[test]
    fn linux_filters_ignore_case() {
        if cfg!(target_os = "linux") {
            assert_eq!(filter_exts(&["png"]), ["[pP][nN][gG]"]);
        }
    }
}
