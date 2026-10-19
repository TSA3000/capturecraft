//! The `.capturecraft` container: a ZIP archive written and read the way
//! PhotoCraft's `photocraft-format` does it — a minimal STORE-only writer
//! and reader of our own (no zip crate, no ZIP64: at most 65 535 entries
//! of under 4 GiB), because every pixel entry is already a compressed PNG.
//! Plus the PNG tile codec (`photocraft-codecs`) and SHA-256 content
//! hashes for deduplicated history tiles.
//!
//! The reader accepts only stored entries, checks every CRC-32 and never
//! panics on input.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

/// Errors reading or writing an archive.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("not a .capturecraft archive: {0}")]
    Corrupt(String),
    #[error("{0} is missing from the archive")]
    Missing(String),
    #[error("{0}")]
    Limit(String),
    #[error("image: {0}")]
    Image(String),
}

pub type Result<T> = std::result::Result<T, ArchiveError>;

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
/// Version 2.0, UTF-8 names (bit 11).
const VERSION: u16 = 20;
const UTF8: u16 = 1 << 11;
/// 1980-01-01 00:00 in DOS date/time.
const DOS_DATE: u16 = (1 << 5) | 1;

pub const MAX_ENTRIES: usize = 65_535;

struct Central {
    name: String,
    crc: u32,
    len: u32,
    offset: u32,
}

/// Builds a STORE-only ZIP in memory.
#[derive(Default)]
pub struct ZipWriter {
    buf: Vec<u8>,
    entries: Vec<Central>,
}

fn u16le(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn u32le(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

impl ZipWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `path` (forward slashes, no leading slash) with `data`.
    pub fn add(&mut self, path: &str, data: &[u8]) -> Result<()> {
        if self.entries.len() >= MAX_ENTRIES {
            return Err(ArchiveError::Limit("more than 65 535 entries".into()));
        }
        if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains("..") {
            return Err(ArchiveError::Limit(format!("bad entry name {path:?}")));
        }
        let name_len = u16::try_from(path.len())
            .map_err(|_| ArchiveError::Limit(format!("entry name too long: {path}")))?;
        let len = u32::try_from(data.len())
            .map_err(|_| ArchiveError::Limit(format!("{path} is 4 GiB or larger")))?;
        let offset = u32::try_from(self.buf.len())
            .map_err(|_| ArchiveError::Limit("archive is 4 GiB or larger".into()))?;
        let crc = crc32fast::hash(data);
        let b = &mut self.buf;
        u32le(b, LOCAL);
        u16le(b, VERSION);
        u16le(b, UTF8);
        u16le(b, 0); // stored
        u16le(b, 0); // time
        u16le(b, DOS_DATE);
        u32le(b, crc);
        u32le(b, len);
        u32le(b, len);
        u16le(b, name_len);
        u16le(b, 0); // extra
        b.extend_from_slice(path.as_bytes());
        b.extend_from_slice(data);
        self.entries.push(Central {
            name: path.to_string(),
            crc,
            len,
            offset,
        });
        Ok(())
    }

    /// The finished archive.
    pub fn finish(mut self) -> Result<Vec<u8>> {
        let start = u32::try_from(self.buf.len())
            .map_err(|_| ArchiveError::Limit("archive is 4 GiB or larger".into()))?;
        let b = &mut self.buf;
        for e in &self.entries {
            u32le(b, CENTRAL);
            u16le(b, VERSION);
            u16le(b, VERSION);
            u16le(b, UTF8);
            u16le(b, 0);
            u16le(b, 0);
            u16le(b, DOS_DATE);
            u32le(b, e.crc);
            u32le(b, e.len);
            u32le(b, e.len);
            u16le(b, e.name.len() as u16);
            u16le(b, 0); // extra
            u16le(b, 0); // comment
            u16le(b, 0); // disk
            u16le(b, 0); // internal attrs
            u32le(b, 0); // external attrs
            u32le(b, e.offset);
            b.extend_from_slice(e.name.as_bytes());
        }
        let size = u32::try_from(b.len()).unwrap_or(u32::MAX) - start;
        let n = self.entries.len() as u16;
        u32le(b, END);
        u16le(b, 0);
        u16le(b, 0);
        u16le(b, n);
        u16le(b, n);
        u32le(b, size);
        u32le(b, start);
        u16le(b, 0);
        Ok(self.buf)
    }
}

/// A read-only view of a STORE-only ZIP.
pub struct ZipReader<'a> {
    data: &'a [u8],
    /// Name → (data start, length, CRC-32).
    entries: BTreeMap<String, (usize, usize, u32)>,
}

fn rd16(d: &[u8], at: usize) -> Result<u16> {
    d.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| ArchiveError::Corrupt("truncated".into()))
}
fn rd32(d: &[u8], at: usize) -> Result<u32> {
    d.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| ArchiveError::Corrupt("truncated".into()))
}

impl<'a> ZipReader<'a> {
    pub fn open(data: &'a [u8]) -> Result<Self> {
        let bad = |m: &str| ArchiveError::Corrupt(m.into());
        // The end record: last 22 bytes plus at most a 64 KiB comment.
        let lo = data.len().saturating_sub(22 + 65_535);
        let eocd = (lo..data.len().saturating_sub(21))
            .rev()
            .find(|&i| rd32(data, i).ok() == Some(END))
            .ok_or_else(|| bad("no end of central directory"))?;
        let n = usize::from(rd16(data, eocd + 10)?);
        let mut at = rd32(data, eocd + 16)? as usize;
        let mut entries = BTreeMap::new();
        for _ in 0..n {
            if rd32(data, at)? != CENTRAL {
                return Err(bad("bad central directory entry"));
            }
            let method = rd16(data, at + 10)?;
            let crc = rd32(data, at + 16)?;
            let csize = rd32(data, at + 20)? as usize;
            let size = rd32(data, at + 24)? as usize;
            let name_len = usize::from(rd16(data, at + 28)?);
            let extra = usize::from(rd16(data, at + 30)?);
            let comment = usize::from(rd16(data, at + 32)?);
            let local = rd32(data, at + 42)? as usize;
            let name = data
                .get(at + 46..at + 46 + name_len)
                .ok_or_else(|| bad("truncated name"))?;
            let name = String::from_utf8(name.to_vec()).map_err(|_| bad("name is not UTF-8"))?;
            if method != 0 || csize != size {
                return Err(ArchiveError::Corrupt(format!(
                    "{name} is compressed; only stored entries are supported"
                )));
            }
            if rd32(data, local)? != LOCAL {
                return Err(bad("bad local header"));
            }
            let start = local
                + 30
                + usize::from(rd16(data, local + 26)?)
                + usize::from(rd16(data, local + 28)?);
            if start.checked_add(size).is_none_or(|end| end > data.len()) {
                return Err(ArchiveError::Corrupt(format!("{name} runs past the end")));
            }
            entries.insert(name, (start, size, crc));
            at += 46 + name_len + extra + comment;
        }
        Ok(Self { data, entries })
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    /// An entry's bytes, CRC-checked.
    pub fn get(&self, path: &str) -> Result<&'a [u8]> {
        let &(start, len, crc) = self
            .entries
            .get(path)
            .ok_or_else(|| ArchiveError::Missing(path.into()))?;
        let d = self
            .data
            .get(start..start + len)
            .ok_or_else(|| ArchiveError::Corrupt(format!("{path} runs past the end")))?;
        if crc32fast::hash(d) != crc {
            return Err(ArchiveError::Corrupt(format!("{path} fails its CRC check")));
        }
        Ok(d)
    }
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// RGBA8 (`w × h × 4` bytes) as PNG.
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};
    let img = Image::from_u8(w, h, ChannelLayout::Rgba, rgba.to_vec())
        .map_err(|e| ArchiveError::Image(e.to_string()))?;
    photocraft_codecs::encode(&img, Format::Png, &EncodeOptions::default())
        .map_err(|e| ArchiveError::Image(e.to_string()))
}

/// A PNG as `(width, height, RGBA8)`.
pub fn decode_png(png: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let img = photocraft_codecs::decode(png).map_err(|e| ArchiveError::Image(e.to_string()))?;
    Ok((img.width(), img.height(), img.to_rgba8()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn zips_round_trip_and_reject_damage() {
        let mut w = ZipWriter::new();
        w.add("manifest.json", b"{}").unwrap();
        w.add("tiles/base/0_0.png", &[7u8; 1000]).unwrap();
        w.add("empty", b"").unwrap();
        assert!(w.add("../evil", b"x").is_err());
        let zip = w.finish().unwrap();
        let r = ZipReader::open(&zip).unwrap();
        assert_eq!(
            r.names().collect::<Vec<_>>(),
            ["empty", "manifest.json", "tiles/base/0_0.png"]
        );
        assert_eq!(r.get("manifest.json").unwrap(), b"{}");
        assert_eq!(r.get("tiles/base/0_0.png").unwrap(), &[7u8; 1000][..]);
        assert!(matches!(r.get("nope"), Err(ArchiveError::Missing(_))));
        // A flipped byte fails the CRC; a cut file fails to open.
        let mut bad = zip.clone();
        let i = bad.windows(4).position(|w| w == [7, 7, 7, 7]).unwrap();
        bad[i] = 8;
        assert!(
            ZipReader::open(&bad)
                .unwrap()
                .get("tiles/base/0_0.png")
                .is_err()
        );
        assert!(ZipReader::open(&zip[..zip.len() - 30]).is_err());
        assert!(ZipReader::open(b"not a zip").is_err());
        // Other tools can read it: the local header layout is standard.
        assert_eq!(&zip[..4], &LOCAL.to_le_bytes());
    }

    #[test]
    fn png_tiles_round_trip_and_hash() {
        let rgba: Vec<u8> = (0..16 * 8 * 4).map(|i| (i % 251) as u8).collect();
        let png = encode_png(16, 8, &rgba).unwrap();
        assert_eq!(decode_png(&png).unwrap(), (16, 8, rgba));
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
