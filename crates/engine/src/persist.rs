//! Saving and opening `.capturecraft` documents, history included.
//!
//! The archive (see `capturecraft_format::archive`):
//!
//! - `manifest.json`: the document, with `base.tiles` (and `edited.tiles`)
//!   listing the stored tiles;
//! - `tiles/base/{tx}_{ty}.png` and `tiles/edited/{tx}_{ty}.png`: the
//!   capture and its edited copy, 256² tiles, transparent tiles left out
//!   and single-colour tiles written as a colour in the manifest;
//! - `objects/{id}.png`: Smart Move objects;
//! - `history.json` + `history/{sha256}.png`: the undo and redo states
//!   (label, manifest, edited pixels, objects). Their tiles are
//!   content-addressed, and states share tiles copy-on-write, so a tile
//!   that no edit touched is stored once however long the history is.
//!
//! Simplify's re-tuning state is not saved: its heals are part of the
//! edited pixels and its shapes are ordinary annotations, so a reopened
//! document looks the same but Simplify starts from scratch.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use capturecraft_format::archive::{self, ZipReader, ZipWriter, sha256_hex};
use capturecraft_format::{
    Color, Manifest, NodeId, RasterLayer, TileData, TileEntry, object_path, tile_path,
};
use photocraft_geom::{Size, TileCoord};
use serde::{Deserialize, Serialize};

use crate::{DocState, EngineError, History, Layers, Pixels, Result, Session, Snapshot};

fn err(e: impl std::fmt::Display) -> EngineError {
    EngineError::Other(e.to_string())
}

/// How a tile is stored.
enum Stored {
    Solid(Color),
    Png(Vec<u8>),
}

/// The tile `c` of `p` (clipped to the raster) as a colour or a PNG;
/// `None` when fully transparent.
fn encode_tile(p: &Pixels, c: TileCoord) -> Result<Option<Stored>> {
    let r = c.rect().intersect(&p.bounds());
    if r.is_empty() {
        return Ok(None);
    }
    let rgba = p.read_rgba8(r).map_err(err)?;
    let px = rgba.as_chunks::<4>().0;
    if px.iter().all(|q| q[3] == 0) {
        return Ok(None);
    }
    if let Some(first) = px.first()
        && px.iter().all(|q| q == first)
    {
        return Ok(Some(Stored::Solid(Color::rgba(
            first[0], first[1], first[2], first[3],
        ))));
    }
    archive::encode_png(r.width(), r.height(), &rgba)
        .map(|png| Some(Stored::Png(png)))
        .map_err(err)
}

/// Writes `p` as `layer`'s tiles; returns the manifest's tile list.
fn write_layer(zip: &mut ZipWriter, layer: &str, p: &Pixels) -> Result<Vec<TileEntry>> {
    let mut coords: Vec<TileCoord> = p.surface().tiles().map(|(c, _)| *c).collect();
    coords.sort_unstable();
    let mut out = Vec::new();
    for c in coords {
        match encode_tile(p, c)? {
            None => {}
            Some(Stored::Solid(color)) => out.push(TileEntry {
                coord: c,
                data: TileData::Solid { color },
            }),
            Some(Stored::Png(png)) => {
                let path = tile_path(layer, c);
                let sha = sha256_hex(&png);
                zip.add(&path, &png).map_err(err)?;
                out.push(TileEntry {
                    coord: c,
                    data: TileData::File {
                        path,
                        sha256: Some(sha),
                    },
                });
            }
        }
    }
    Ok(out)
}

/// Reads a layer's tiles back into pixels of `size`.
fn read_layer(zip: &ZipReader, size: Size, tiles: &[TileEntry]) -> Result<Pixels> {
    let mut p = Pixels::transparent(size);
    for t in tiles {
        let r = t.coord.rect().intersect(&p.bounds());
        if r.is_empty() {
            continue;
        }
        let n = r.width() as usize * r.height() as usize;
        let rgba = match &t.data {
            TileData::Solid { color } => [color.r, color.g, color.b, color.a].repeat(n),
            TileData::File { path, .. } => {
                let (w, h, rgba) = archive::decode_png(zip.get(path).map_err(err)?).map_err(err)?;
                if (w, h) != (r.width(), r.height()) {
                    return Err(err(format!(
                        "{path} is {w}×{h}, expected {}×{}",
                        r.width(),
                        r.height()
                    )));
                }
                rgba
            }
        };
        p.write_rgba8(r, &rgba).map_err(err)?;
    }
    Ok(p)
}

/// A layer as stored in a history state: content-addressed tiles.
#[derive(Serialize, Deserialize, Default)]
struct HistoryLayer {
    size: Size,
    tiles: Vec<(TileCoord, HistoryTile)>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum HistoryTile {
    Solid { color: Color },
    Blob { sha256: String },
}

#[derive(Serialize, Deserialize)]
struct HistoryState {
    label: String,
    manifest: serde_json::Value,
    #[serde(default)]
    edited: Option<HistoryLayer>,
    #[serde(default)]
    objects: BTreeMap<u64, HistoryLayer>,
}

#[derive(Serialize, Deserialize, Default)]
struct HistoryFile {
    undo: Vec<HistoryState>,
    redo: Vec<HistoryState>,
}

/// Writes history tiles once each: by tile identity (states share tiles
/// copy-on-write) and by content hash.
#[derive(Default)]
struct BlobStore {
    by_tile: HashMap<*const photocraft_raster::Tile, Option<HistoryTile>>,
    written: std::collections::HashSet<String>,
}

impl BlobStore {
    fn layer(&mut self, zip: &mut ZipWriter, p: &Pixels) -> Result<HistoryLayer> {
        let mut tiles = Vec::new();
        let mut all: Vec<(TileCoord, Arc<photocraft_raster::Tile>)> =
            p.surface().tiles().map(|(c, t)| (*c, t.clone())).collect();
        all.sort_by_key(|(c, _)| *c);
        for (c, tile) in all {
            let key = Arc::as_ptr(&tile);
            let stored = match self.by_tile.get(&key) {
                Some(s) => s.as_ref().map(clone_tile),
                None => {
                    let s = match encode_tile(p, c)? {
                        None => None,
                        Some(Stored::Solid(color)) => Some(HistoryTile::Solid { color }),
                        Some(Stored::Png(png)) => {
                            let sha = sha256_hex(&png);
                            if self.written.insert(sha.clone()) {
                                zip.add(&format!("history/{sha}.png"), &png).map_err(err)?;
                            }
                            Some(HistoryTile::Blob { sha256: sha })
                        }
                    };
                    self.by_tile.insert(key, s.as_ref().map(clone_tile));
                    s
                }
            };
            if let Some(s) = stored {
                tiles.push((c, s));
            }
        }
        Ok(HistoryLayer {
            size: p.size(),
            tiles,
        })
    }
}

fn clone_tile(t: &HistoryTile) -> HistoryTile {
    match t {
        HistoryTile::Solid { color } => HistoryTile::Solid { color: *color },
        HistoryTile::Blob { sha256 } => HistoryTile::Blob {
            sha256: sha256.clone(),
        },
    }
}

fn read_history_layer(zip: &ZipReader, l: &HistoryLayer) -> Result<Pixels> {
    let entries: Vec<TileEntry> = l
        .tiles
        .iter()
        .map(|(c, t)| TileEntry {
            coord: *c,
            data: match t {
                HistoryTile::Solid { color } => TileData::Solid { color: *color },
                HistoryTile::Blob { sha256 } => TileData::File {
                    path: format!("history/{sha256}.png"),
                    sha256: None,
                },
            },
        })
        .collect();
    read_layer(zip, l.size, &entries)
}

/// What a state shows under the annotations: the edited capture, with
/// Simplify's heals when there are any.
fn shown(l: &Layers) -> Option<&Pixels> {
    l.simplify
        .as_ref()
        .and_then(|s| s.pixels.as_ref())
        .or(l.edited.as_ref())
}

fn history_state(store: &mut BlobStore, zip: &mut ZipWriter, s: &Snapshot) -> Result<HistoryState> {
    let edited = shown(&s.layers).map(|p| store.layer(zip, p)).transpose()?;
    let mut objects = BTreeMap::new();
    for (id, p) in &s.layers.objects {
        objects.insert(id.0, store.layer(zip, p)?);
    }
    Ok(HistoryState {
        label: s.label.clone(),
        manifest: serde_json::to_value(&s.manifest).map_err(err)?,
        edited,
        objects,
    })
}

/// The document as a `.capturecraft` archive; `generator` names the app
/// (`capturecraft 0.22.0`).
pub fn save(doc: &DocState, generator: &str) -> Result<Vec<u8>> {
    let mut zip = ZipWriter::new();
    let mut m = doc.manifest.clone();
    m.generator = generator.to_string();
    let base = doc
        .pixels
        .as_ref()
        .ok_or_else(|| err("the document has no capture pixels"))?;
    m.base.tiles = write_layer(&mut zip, "base", base)?;
    m.edited = match shown(&doc.layers) {
        Some(p) if p != base => Some(RasterLayer {
            tiles: write_layer(&mut zip, "edited", p)?,
            ..m.base.clone()
        }),
        _ => None,
    };
    for (id, p) in &doc.layers.objects {
        let rgba = p.to_rgba8().map_err(err)?;
        let s = p.size();
        let png = archive::encode_png(s.width, s.height, &rgba).map_err(err)?;
        zip.add(&object_path(*id), &png).map_err(err)?;
    }
    let mut store = BlobStore::default();
    let mut history = HistoryFile::default();
    for s in &doc.history.undo {
        history.undo.push(history_state(&mut store, &mut zip, s)?);
    }
    for s in &doc.history.redo {
        history.redo.push(history_state(&mut store, &mut zip, s)?);
    }
    zip.add("history.json", &serde_json::to_vec(&history).map_err(err)?)
        .map_err(err)?;
    // The manifest last, so a reader that finds it knows the rest is there.
    zip.add("manifest.json", &m.to_json().map_err(err)?)
        .map_err(err)?;
    zip.finish().map_err(err)
}

fn read_objects(zip: &ZipReader, m: &Manifest) -> Result<BTreeMap<NodeId, Pixels>> {
    let mut out = BTreeMap::new();
    for n in &m.annotations {
        if let Some(capturecraft_format::Annotation::SmartObject(_)) = n.annotation() {
            let png = zip.get(&object_path(n.id)).map_err(err)?;
            let (w, h, rgba) = archive::decode_png(png).map_err(err)?;
            let p = Pixels::new(Size::new(w, h), rgba).ok_or_else(|| err("bad object pixels"))?;
            out.insert(n.id, p);
        }
    }
    Ok(out)
}

fn restore_state(zip: &ZipReader, h: &HistoryState, base: &Pixels) -> Result<Snapshot> {
    let manifest: Manifest = serde_json::from_value(h.manifest.clone()).map_err(err)?;
    manifest.validate()?;
    let edited = h
        .edited
        .as_ref()
        .map(|l| read_history_layer(zip, l))
        .transpose()?
        .filter(|p| p.size() == base.size());
    let mut objects = BTreeMap::new();
    for (id, l) in &h.objects {
        objects.insert(NodeId(*id), read_history_layer(zip, l)?);
    }
    Ok(Snapshot {
        label: h.label.clone(),
        manifest,
        layers: Layers {
            edited,
            objects,
            simplify: None,
        },
    })
}

impl Session {
    /// Opens a `.capturecraft` archive as a new active document; returns
    /// its index. A damaged history is dropped (with the document still
    /// opening); a damaged document is an error and opens nothing.
    pub fn open_archive(&mut self, bytes: &[u8]) -> Result<usize> {
        let zip = ZipReader::open(bytes).map_err(err)?;
        let mut m = Manifest::from_json(zip.get("manifest.json").map_err(err)?)?;
        let base = read_layer(&zip, m.base.size, &m.base.tiles)?;
        let edited = match &m.edited {
            Some(e) => Some(read_layer(&zip, e.size, &e.tiles)?),
            None => None,
        };
        let objects = read_objects(&zip, &m)?;
        let history = zip
            .get("history.json")
            .ok()
            .and_then(|b| serde_json::from_slice::<HistoryFile>(b).ok())
            .and_then(|h| {
                let undo: Result<Vec<_>> = h
                    .undo
                    .iter()
                    .map(|s| restore_state(&zip, s, &base))
                    .collect();
                let redo: Result<Vec<_>> = h
                    .redo
                    .iter()
                    .map(|s| restore_state(&zip, s, &base))
                    .collect();
                Some(History {
                    undo: undo.ok()?,
                    redo: redo.ok()?,
                })
            })
            .unwrap_or_default();
        // In memory the tiles live in the pixels, not the manifest.
        m.base.tiles.clear();
        m.edited = None;
        let index = self.open(m, Some(base))?;
        if let Some(d) = self.active_mut() {
            d.layers = Layers {
                edited,
                objects,
                simplify: None,
            };
            d.history = history;
        }
        Ok(index)
    }

    /// The document at `index` as a `.capturecraft` archive.
    pub fn save_archive(&self, index: usize, generator: &str) -> Result<Vec<u8>> {
        let doc = self.documents().get(index).ok_or(EngineError::NoDocument)?;
        save(doc, generator)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::render::flatten;
    use serde_json::json;

    fn session() -> Session {
        let (w, h) = (600u32, 300u32);
        // A gradient on the left (PNG tiles), flat white on the right
        // (solid tiles), a transparent strip at the bottom (no tiles).
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                if y >= 280 {
                    [0, 0, 0, 0]
                } else if x < 256 {
                    [x as u8, y as u8, 128, 255]
                } else {
                    [255; 4]
                }
            })
            .collect();
        let mut s = Session::new();
        let size = Size::new(w, h);
        let mut m = Manifest::new(size);
        m.id = "doc-1".into();
        m.title = "Saved".into();
        s.open(m, Pixels::new(size, rgba)).unwrap();
        s
    }

    #[test]
    fn documents_round_trip_with_their_history() {
        let mut s = session();
        s.run(
            "annotation.addArrow",
            &json!({ "start": { "x": 10, "y": 10 }, "end": { "x": 200, "y": 100 } }),
        )
        .unwrap();
        s.run(
            "canvas.erase",
            &json!({ "points": [{ "x": 300, "y": 50 }], "radius": 10 }),
        )
        .unwrap();
        s.run(
            "annotation.addText",
            &json!({ "bounds": { "x": 20, "y": 150, "w": 200, "h": 40 }, "text": "Hi" }),
        )
        .unwrap();
        s.run("edit.undo", &json!({})).unwrap();
        let before = flatten(s.active().unwrap()).unwrap();
        let bytes = s.save_archive(0, "capturecraft test").unwrap();

        let zip = ZipReader::open(&bytes).unwrap();
        let names: Vec<&str> = zip.names().collect();
        assert!(names.contains(&"tiles/base/0_0.png"));
        assert!(
            !names.contains(&"tiles/base/1_0.png"),
            "flat white is a solid tile"
        );
        assert!(names.contains(&"tiles/edited/1_0.png"), "the erased hole");
        let m = Manifest::from_json(zip.get("manifest.json").unwrap()).unwrap();
        assert_eq!(m.generator, "capturecraft test");
        assert!(
            m.base
                .tiles
                .iter()
                .any(|t| matches!(t.data, TileData::Solid { .. }))
        );

        let mut s2 = Session::new();
        s2.open_archive(&bytes).unwrap();
        let d = s2.active().unwrap();
        assert_eq!(d.manifest.title, "Saved");
        assert_eq!(flatten(d).unwrap(), before, "pixel-identical");
        let (undo, redo) = d.history_labels();
        assert_eq!((undo, redo), (vec!["Add Arrow", "Erase"], vec!["Add Text"]));
        // The history works: redo the text, undo back past the erase.
        s2.run("edit.redo", &json!({})).unwrap();
        assert_eq!(s2.active().unwrap().manifest.annotations.len(), 2);
        s2.run("edit.jump", &json!({ "steps": -3 })).unwrap();
        let d = s2.active().unwrap();
        assert!(d.manifest.annotations.is_empty());
        assert_eq!(
            d.edited_pixels().unwrap().pixel(300, 50)[3],
            255,
            "erase undone"
        );
    }

    #[test]
    fn damaged_archives_open_nothing() {
        let s = session();
        let bytes = s.save_archive(0, "t").unwrap();
        let mut s2 = Session::new();
        assert!(s2.open_archive(&bytes[..bytes.len() / 2]).is_err());
        assert!(s2.open_archive(b"PK nonsense").is_err());
        assert!(s2.documents().is_empty());
        // A broken history keeps the document.
        let zip = ZipReader::open(&bytes).unwrap();
        let mut w = ZipWriter::new();
        for n in zip.names() {
            let data = if n == "history.json" {
                &b"{ broken"[..]
            } else {
                zip.get(n).unwrap()
            };
            w.add(n, data).unwrap();
        }
        s2.open_archive(&w.finish().unwrap()).unwrap();
        assert!(!s2.active().unwrap().can_undo());
    }
}
