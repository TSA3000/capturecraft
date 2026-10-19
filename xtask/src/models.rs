//! `cargo xtask models [DIR]`: fetch the text recognition models
//! (`ATTRIBUTION.toml` entries with `kind = "model"`) into `assets/models/`
//! (git-ignored), where `capturecraft-ocr` looks for them. Each file is
//! checked against its SHA-256 and comes with its licence
//! (`<file>.LICENCE.txt`); `ATTRIBUTION.txt` credits them all, so release
//! packages can ship the directory as it is. Same scheme as
//! storytold/pdfcraft.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub fetched: Vec<Fetched>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fetched {
    pub file: String,
    pub url: String,
    pub title: String,
    pub author: String,
    pub source: String,
    pub licence: String,
    pub licence_url: String,
    pub licence_sha256: String,
    pub kind: String,
    pub usage: String,
    pub sha256: String,
}

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

pub fn load(root: &Path) -> Result<Manifest, String> {
    let path = root.join("ATTRIBUTION.toml");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

pub fn run(args: &[String]) -> Result<(), String> {
    let root = root();
    let m = load(&root)?;
    let dir = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("assets/models"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for f in m.fetched.iter().filter(|f| f.kind == "model") {
        let path = dir.join(&f.file);
        fetch_verified(&f.url, &path, &f.sha256)?;
        fetch_verified(
            &f.licence_url,
            &dir.join(format!("{}.LICENCE.txt", f.file)),
            &f.licence_sha256,
        )?;
        println!("models: {}", path.display());
    }
    std::fs::write(dir.join("ATTRIBUTION.txt"), attribution(&m)).map_err(|e| e.to_string())
}

/// Downloads `url` to `path` unless a file with the right hash is there.
/// Uses `curl` (macOS, Windows 10+ and Linux all have it).
fn fetch_verified(url: &str, path: &Path, sha256: &str) -> Result<(), String> {
    if std::fs::read(path).is_ok_and(|b| sha256_hex(&b) == sha256) {
        return Ok(());
    }
    println!("models: fetching {url}");
    let tmp = path.with_extension("part");
    let status = Command::new("curl")
        .args(["-sSfL", "--retry", "3", "-o"])
        .arg(&tmp)
        .arg(url)
        .status()
        .map_err(|e| format!("running curl: {e}"))?;
    if !status.success() {
        return Err(format!("download failed: {url}"));
    }
    let bytes = std::fs::read(&tmp).map_err(|e| e.to_string())?;
    if sha256_hex(&bytes) != sha256 {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "{url}: SHA-256 mismatch (expected {sha256}); refusing to use it"
        ));
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// The credits shipped beside the models (their licences require attribution).
fn attribution(m: &Manifest) -> String {
    let mut s = String::from(
        "CaptureCraft's text recognition (Grab Text, Edit Text) uses these models, unmodified.\n",
    );
    for f in m.fetched.iter().filter(|f| f.kind == "model") {
        let _ = write!(
            s,
            "\n{}\n  {}\n  by {}\n  from {}\n  used for: {}\n  licence: {} ({}; full text in {}.LICENCE.txt)\n",
            f.file, f.title, f.author, f.source, f.usage, f.licence, f.licence_url, f.file
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The OCR crate checks the files at load time against its own copy
    /// of the hashes; both lists must agree.
    #[test]
    fn ocr_crate_knows_every_model_hash() {
        let m = load(&root()).unwrap_or_default();
        let models: Vec<&Fetched> = m.fetched.iter().filter(|f| f.kind == "model").collect();
        assert_eq!(models.len(), 2);
        let src = include_str!("../../crates/ocr/src/models.rs");
        for f in models {
            assert!(
                src.contains(&format!("\"{}\"", f.sha256)),
                "{} hash not in crates/ocr/src/models.rs",
                f.file
            );
            assert!(attribution(&m).contains(&f.author));
        }
    }
}
