//! The CaptureCraft document format (`.capturecraft`).
//!
//! A document is a ZIP archive: `manifest.json` ([`manifest::Manifest`])
//! describes everything, pixel data lives next to it as PNG tiles
//! (`tiles/base/{tx}_{ty}.png`, 256 × 256, the `photocraft-raster` grid) and
//! the cursor sprite (`cursor/sprite.png`). Every struct reads with
//! `#[serde(default)]`, so older files open in newer builds and unknown
//! fields from newer builds are ignored; unknown annotation types are kept
//! verbatim ([`manifest::NodeKind::Unknown`]) and written back unchanged.
//!
//! Nothing here panics on input: [`manifest::Manifest::from_json`] parses
//! and then [`manifest::Manifest::validate`]s, and every limit is a
//! constant in [`manifest`].
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod archive;
pub mod manifest;

pub use manifest::*;

/// Errors reading or validating a document.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("manifest is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("format version {found} is newer than this build supports ({supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("{path}: {msg}")]
    Invalid { path: String, msg: String },
}

pub type Result<T> = std::result::Result<T, FormatError>;
