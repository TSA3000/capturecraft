//! The text recognition models: which files, where to find them and how to
//! check them. `cargo xtask models` downloads them (listed with their
//! licences in `ATTRIBUTION.toml`) into `assets/models/`; the SHA-256 sums
//! below must match that list (an xtask test checks it).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::OcrError;

pub const DETECTION_MODEL: &str = "text-detection.rten";
pub const RECOGNITION_MODEL: &str = "text-recognition.rten";

/// `(file, SHA-256)` of every model.
pub const MODEL_SHA256: &[(&str, &str)] = &[
    (
        DETECTION_MODEL,
        "f15cfb56bd02c4bf478a20343986504a1f01e1665c2b3a0ad66340f054b1b5ca",
    ),
    (
        RECOGNITION_MODEL,
        "e484866d4cce403175bd8d00b128feb08ab42e208de30e42cd9889d8f1735a6e",
    ),
];

/// Overrides the search ([`Models::search_dirs`]).
pub const MODELS_ENV: &str = "CAPTURECRAFT_MODELS";

/// Where the two model files are.
#[derive(Clone, Debug, PartialEq)]
pub struct Models {
    pub detection: PathBuf,
    pub recognition: PathBuf,
}

impl Models {
    /// The models in `dir`, if both files are there.
    pub fn in_dir(dir: &Path) -> Option<Models> {
        let m = Models {
            detection: dir.join(DETECTION_MODEL),
            recognition: dir.join(RECOGNITION_MODEL),
        };
        (m.detection.is_file() && m.recognition.is_file()).then_some(m)
    }

    /// The first directory of [`Models::search_dirs`] holding both files.
    pub fn find() -> Option<Models> {
        Self::search_dirs().iter().find_map(|d| Self::in_dir(d))
    }

    /// `$CAPTURECRAFT_MODELS`, then beside the executable (`models/` for the
    /// Windows installers and zips, `../share/capturecraft/models` for
    /// Linux packages, `../Resources/models` in a macOS bundle), then the
    /// source tree's `assets/models/`.
    pub fn search_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(d) = std::env::var_os(MODELS_ENV) {
            dirs.push(PathBuf::from(d));
        }
        if let Some(exe) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
        {
            dirs.push(exe.join("models"));
            dirs.push(exe.join("../share/capturecraft/models"));
            dirs.push(exe.join("../Resources/models"));
        }
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models"));
        dirs
    }

    /// Checks both files against [`MODEL_SHA256`]; a truncated download or
    /// a different model release is refused rather than half-working.
    pub fn verify(&self) -> Result<(), OcrError> {
        for (path, name) in [
            (&self.detection, DETECTION_MODEL),
            (&self.recognition, RECOGNITION_MODEL),
        ] {
            let want = MODEL_SHA256
                .iter()
                .find(|(f, _)| *f == name)
                .map(|(_, s)| *s)
                .unwrap_or_default();
            let bytes = std::fs::read(path)
                .map_err(|e| OcrError::Load(path.display().to_string(), e.to_string()))?;
            let got = sha256_hex(&bytes);
            if got != want {
                return Err(OcrError::Load(
                    path.display().to_string(),
                    format!("SHA-256 {got} does not match {want}; run `cargo xtask models`"),
                ));
            }
        }
        Ok(())
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn verify_refuses_other_files() {
        let dir = std::env::temp_dir().join(format!("capturecraft-models-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in [DETECTION_MODEL, RECOGNITION_MODEL] {
            std::fs::write(dir.join(f), b"not a model").unwrap();
        }
        let m = Models::in_dir(&dir).unwrap();
        assert!(matches!(m.verify(), Err(OcrError::Load(..))));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
