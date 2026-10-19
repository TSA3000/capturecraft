//! Tile-major premultiplied canvas, frames and tile sets.
//!
//! [`TiledCanvas`] stores 256×256 tiles back to back (edge tiles padded to
//! full size), each tile row-major. The GPU canvas buffer has exactly this
//! layout, so tiles move between the two as single contiguous copies, and
//! the CPU compositor can hand each tile to a different rayon worker as a
//! disjoint `&mut` slice.

use photocraft_geom::{Rect, TILE_SIZE};
use rayon::prelude::*;

use crate::CompositorError;
use crate::effects::Px;

pub const TILE: usize = TILE_SIZE as usize;
pub const TILE_PX: usize = TILE * TILE;

/// Largest canvas edge accepted.
pub const MAX_EDGE: u32 = 32_768;

pub fn tiles_for(len: u32) -> u32 {
    len.div_ceil(TILE as u32)
}

#[derive(Clone, Debug, PartialEq)]
pub struct TiledCanvas {
    width: u32,
    height: u32,
    tiles_x: u32,
    tiles_y: u32,
    data: Vec<Px>,
}

impl TiledCanvas {
    pub fn new(width: u32, height: u32) -> Result<Self, CompositorError> {
        if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
            return Err(CompositorError::InvalidSize { width, height });
        }
        let (tiles_x, tiles_y) = (tiles_for(width), tiles_for(height));
        let n = tiles_x as usize * tiles_y as usize * TILE_PX;
        let mut data = Vec::new();
        data.try_reserve_exact(n)
            .map_err(|_| CompositorError::OutOfMemory {
                bytes: n.saturating_mul(16),
            })?;
        data.resize(n, [0.0; 4]);
        Ok(Self {
            width,
            height,
            tiles_x,
            tiles_y,
            data,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn tiles_x(&self) -> u32 {
        self.tiles_x
    }
    pub fn tiles_y(&self) -> u32 {
        self.tiles_y
    }
    pub fn rect(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }
    pub fn tile_count(&self) -> usize {
        self.tiles_x as usize * self.tiles_y as usize
    }

    /// Raw tile-major pixels (the GPU buffer layout).
    pub fn data(&self) -> &[Px] {
        &self.data
    }
    pub fn data_mut(&mut self) -> &mut [Px] {
        &mut self.data
    }

    /// Canvas rectangle of tile `i` (clipped to the canvas).
    pub fn tile_rect(&self, i: usize) -> Rect {
        let tx = (i % self.tiles_x as usize) as i32;
        let ty = (i / self.tiles_x as usize) as i32;
        Rect::from_xywh(tx * TILE_SIZE, ty * TILE_SIZE, TILE as u32, TILE as u32)
            .intersect(&self.rect())
    }

    #[inline]
    pub fn index(&self, x: i32, y: i32) -> usize {
        let (x, y) = (x as usize, y as usize);
        ((y / TILE) * self.tiles_x as usize + x / TILE) * TILE_PX + (y % TILE) * TILE + x % TILE
    }

    /// Pixel at `(x, y)`; transparent outside the canvas.
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> Px {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return [0.0; 4];
        }
        self.data.get(self.index(x, y)).copied().unwrap_or([0.0; 4])
    }

    pub fn fill(&mut self, p: Px) {
        self.data.par_iter_mut().for_each(|v| *v = p);
    }

    /// Runs `f(tile_rect, tile_pixels)` on every tile in `set`, in parallel.
    pub fn par_tiles(&mut self, set: &TileSet, f: impl Fn(Rect, &mut [Px]) + Sync) {
        let (tx, w, h) = (self.tiles_x as usize, self.width as i32, self.height as i32);
        let canvas = Rect::new(0, 0, w, h);
        self.data
            .par_chunks_mut(TILE_PX)
            .enumerate()
            .filter(|(i, _)| set.contains(*i))
            .for_each(|(i, px)| {
                let r = Rect::from_xywh(
                    (i % tx) as i32 * TILE_SIZE,
                    (i / tx) as i32 * TILE_SIZE,
                    TILE as u32,
                    TILE as u32,
                )
                .intersect(&canvas);
                f(r, px);
            });
    }

    /// Copies the tiles of `set` from `src` (same size).
    pub fn copy_tiles(&mut self, src: &TiledCanvas, set: &TileSet) {
        if src.tiles_x != self.tiles_x || src.tiles_y != self.tiles_y {
            return;
        }
        self.data
            .par_chunks_mut(TILE_PX)
            .zip(src.data.par_chunks(TILE_PX))
            .enumerate()
            .filter(|(i, _)| set.contains(*i))
            .for_each(|(_, (d, s))| d.copy_from_slice(s));
    }

    /// Row-major copy of `r` (clipped to the canvas).
    pub fn read_rect(&self, r: Rect) -> Snapshot {
        let rect = r.intersect(&self.rect());
        let w = rect.width() as usize;
        let mut data = vec![[0.0f32; 4]; w * rect.height() as usize];
        data.par_chunks_mut(w.max(1))
            .enumerate()
            .for_each(|(row, out)| {
                let y = rect.y0 + row as i32;
                for (i, v) in out.iter_mut().enumerate() {
                    *v = self.get(rect.x0 + i as i32, y);
                }
            });
        Snapshot { rect, data }
    }

    pub fn to_frame(&self) -> Frame {
        let w = self.width as usize;
        let mut pixels = vec![[0.0f32; 4]; w * self.height as usize];
        pixels.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                *v = self.get(x as i32, y as i32);
            }
        });
        Frame {
            width: self.width,
            height: self.height,
            pixels,
        }
    }

    /// Builds a canvas from tile-major data of the right length.
    pub fn from_tile_data(width: u32, height: u32, data: Vec<Px>) -> Result<Self, CompositorError> {
        let c = Self {
            width,
            height,
            tiles_x: tiles_for(width),
            tiles_y: tiles_for(height),
            data,
        };
        if width == 0 || height == 0 || c.data.len() != c.tile_count() * TILE_PX {
            return Err(CompositorError::InvalidSize { width, height });
        }
        Ok(c)
    }
}

/// Read-only row-major copy of part of a canvas (the backdrop a node
/// samples while it overwrites the canvas).
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub rect: Rect,
    pub data: Vec<Px>,
}

impl Snapshot {
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> Px {
        if !self.rect.contains(x, y) {
            return [0.0; 4];
        }
        let i =
            (y - self.rect.y0) as usize * self.rect.width() as usize + (x - self.rect.x0) as usize;
        self.data.get(i).copied().unwrap_or([0.0; 4])
    }
}

/// A rendered image: row-major, premultiplied, sRGB-encoded floats.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<Px>,
}

impl Frame {
    pub fn pixel(&self, x: u32, y: u32) -> Px {
        self.pixels
            .get(y as usize * self.width as usize + x as usize)
            .copied()
            .unwrap_or([0.0; 4])
    }

    /// Straight-alpha RGBA8 (for PNG export and the clipboard).
    pub fn to_rgba8(&self) -> Vec<u8> {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        self.pixels
            .iter()
            .flat_map(|p| {
                let a = p[3];
                if a <= 0.0 {
                    [0, 0, 0, 0]
                } else {
                    [q(p[0] / a), q(p[1] / a), q(p[2] / a), q(a)]
                }
            })
            .collect()
    }
}

/// Set of tiles of a canvas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileSet {
    tiles_x: u32,
    tiles_y: u32,
    width: u32,
    height: u32,
    bits: Vec<bool>,
}

impl TileSet {
    pub fn empty(width: u32, height: u32) -> Self {
        let (tiles_x, tiles_y) = (tiles_for(width), tiles_for(height));
        Self {
            tiles_x,
            tiles_y,
            width,
            height,
            bits: vec![false; tiles_x as usize * tiles_y as usize],
        }
    }

    pub fn full(width: u32, height: u32) -> Self {
        let mut s = Self::empty(width, height);
        s.bits.iter_mut().for_each(|b| *b = true);
        s
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    #[inline]
    pub fn contains(&self, i: usize) -> bool {
        self.bits.get(i).copied().unwrap_or(false)
    }

    pub fn insert_rect(&mut self, r: Rect) {
        let r = r.intersect(&Rect::new(0, 0, self.width as i32, self.height as i32));
        for tc in r.tiles() {
            let i = tc.ty as usize * self.tiles_x as usize + tc.tx as usize;
            if let Some(b) = self.bits.get_mut(i) {
                *b = true;
            }
        }
    }

    pub fn union_with(&mut self, o: &TileSet) {
        if o.bits.len() == self.bits.len() {
            self.bits
                .iter_mut()
                .zip(&o.bits)
                .for_each(|(a, b)| *a |= *b);
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.bits.iter().any(|b| *b)
    }

    pub fn count(&self) -> usize {
        self.bits.iter().filter(|b| **b).count()
    }

    pub fn is_full(&self) -> bool {
        self.bits.iter().all(|b| *b)
    }

    /// Canvas rectangles of the tiles in the set (clipped to the canvas).
    pub fn rects(&self) -> impl Iterator<Item = Rect> + '_ {
        let canvas = Rect::new(0, 0, self.width as i32, self.height as i32);
        self.bits
            .iter()
            .enumerate()
            .filter(|(_, b)| **b)
            .map(move |(i, _)| {
                let (tx, ty) = (
                    (i % self.tiles_x as usize) as i32,
                    (i / self.tiles_x as usize) as i32,
                );
                Rect::from_xywh(tx * TILE_SIZE, ty * TILE_SIZE, TILE as u32, TILE as u32)
                    .intersect(&canvas)
            })
    }

    /// Bounding rectangle of the set's tiles intersected with `r`.
    pub fn bounds_within(&self, r: Rect) -> Rect {
        self.rects()
            .fold(Rect::EMPTY, |acc, t| acc.union(&t.intersect(&r)))
    }

    /// Tiles of the set that intersect `r`.
    pub fn restricted(&self, r: Rect) -> TileSet {
        let mut s = TileSet::empty(self.width, self.height);
        for t in self.rects() {
            if !t.intersect(&r).is_empty() {
                s.insert_rect(t);
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_major_layout() {
        let c = TiledCanvas::new(300, 10).unwrap_or_else(|_| TiledCanvas {
            width: 0,
            height: 0,
            tiles_x: 0,
            tiles_y: 0,
            data: vec![],
        });
        assert_eq!(c.tile_count(), 2);
        assert_eq!(c.index(255, 0), 255);
        assert_eq!(c.index(256, 0), TILE_PX);
        assert_eq!(c.index(0, 1), TILE);
    }

    #[test]
    fn tile_sets() {
        let mut s = TileSet::empty(600, 300);
        s.insert_rect(Rect::new(250, 250, 260, 260));
        assert_eq!(s.count(), 4);
        assert_eq!(
            s.bounds_within(Rect::new(0, 0, 600, 300)),
            Rect::new(0, 0, 512, 300)
        );
        assert!(TiledCanvas::new(0, 5).is_err());
    }
}
