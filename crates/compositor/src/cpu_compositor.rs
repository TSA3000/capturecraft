//! CPU reference compositor: the oracle every other backend is measured
//! against, and the renderer for headless exports and CI.
//!
//! Deterministic and portable: the same scene gives bit-identical frames
//! on every machine and thread count (each pixel is computed by exactly
//! one task, in a fixed order). Work is spread over 256×256 tiles with
//! rayon.
//!
//! [`CpuCompositor::update`] redraws only what changed. Damage starts at
//! the nodes whose revision (or raster tiles) changed and at the caller's
//! [`DirtyRegion`], and is pushed forward through every later op (a blur
//! spreads it by its radius, a magnifier spreads damage under its source
//! over the whole lens, a Cut Out moves it). Then the tiles each op must
//! produce are pulled backwards (a blurred tile needs its neighbours as
//! input), and only those tiles are computed, op by op. Cut outs and the
//! shadow frame start a new canvas of their output size.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use photocraft_algo::{Ctx, FilterParams, Image};
use photocraft_color::ColorMode;
use photocraft_geom::{Affine, Rect};
use photocraft_raster::Surface;
use rayon::prelude::*;

use crate::CompositorError;
use crate::canvas::{Frame, TILE, TileSet, TiledCanvas};
use crate::effects::backdrop::dim;
use crate::effects::{Kernel, Mask, Px, blur_mask, mix, over, scale};
use crate::prepare::{FillOp, FrameOp, ImageOp, Op, Prepared, prepare};
use crate::scene::{DirtyRegion, NodeId, RenderNode, RenderScene, RevisionId};

/// An op with its coverage resolved for the CPU (fill masks blurred,
/// inverted and clipped).
#[derive(Clone, Debug)]
pub struct CpuOp {
    pub op: Op,
    pub coverage: Option<Arc<Mask>>,
}

fn resolve_fill(f: &FillOp) -> Arc<Mask> {
    if f.blur.is_none() && !f.invert && f.clip.is_none() {
        return f.mask.clone();
    }
    let base = match &f.blur {
        Some(k) => Arc::new(blur_mask(&f.mask, k)),
        None => f.mask.clone(),
    };
    let rect = f.coverage_rect();
    let w = rect.width() as usize;
    let mut data = vec![0.0f32; w * rect.height() as usize];
    data.par_chunks_mut(w.max(1))
        .enumerate()
        .for_each(|(row, out)| {
            let y = rect.y0 + row as i32;
            for (i, v) in out.iter_mut().enumerate() {
                let x = rect.x0 + i as i32;
                let mut m = base.at(x, y);
                if f.invert {
                    m = 1.0 - m;
                }
                if let Some(c) = &f.clip {
                    m *= c.at(x, y);
                }
                *v = m;
            }
        });
    Arc::new(Mask { rect, data })
}

/// Resolves a prepared node for the CPU.
pub fn resolve(p: &Prepared) -> Vec<CpuOp> {
    p.ops
        .iter()
        .map(|op| {
            let coverage = match op {
                Op::Fill(f) => Some(resolve_fill(f)),
                Op::Spotlight(s) => s.mask.clone(),
                _ => None,
            };
            CpuOp {
                op: op.clone(),
                coverage,
            }
        })
        .collect()
}

struct CachedNode {
    revision: RevisionId,
    transform: Option<Affine>,
    stage: (u32, u32),
    ops: Arc<Vec<CpuOp>>,
    bounds: Rect,
    out_size: (u32, u32),
    /// Raster nodes: the surface and origin drawn, to diff tiles against.
    raster: Option<(Arc<Surface>, [i32; 2])>,
}

#[derive(PartialEq)]
struct SceneShape {
    size: (u32, u32),
    background: [u32; 4],
    nodes: Vec<NodeId>,
    stages: Vec<(u32, u32)>,
}

/// What an [`CpuCompositor::update`] redrew.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderStats {
    /// The whole frame was redrawn.
    pub full: bool,
    /// Tiles of the final frame that changed.
    pub tiles: usize,
    /// Their rectangles (frame coordinates).
    pub damage: Vec<Rect>,
}

/// Tile-parallel CPU compositor with per-node caches and damage tracking.
#[derive(Default)]
pub struct CpuCompositor {
    cache: HashMap<NodeId, CachedNode>,
    frame: Option<TiledCanvas>,
    scratch: Option<TiledCanvas>,
    shape: Option<SceneShape>,
}

/// Tiles of `a` and `b`'s grids whose pixels differ (by tile identity).
pub(crate) fn changed_tiles(a: &Surface, b: &Surface) -> Vec<Rect> {
    let mut out = Vec::new();
    let mut keys: HashSet<photocraft_geom::TileCoord> = a.tiles().map(|(c, _)| *c).collect();
    keys.extend(b.tiles().map(|(c, _)| *c));
    for c in keys {
        let same = match (a.tile(c), b.tile(c)) {
            (Some(x), Some(y)) => Arc::ptr_eq(x, y) || x == y,
            (None, None) => true,
            _ => false,
        };
        if !same {
            out.push(c.rect());
        }
    }
    out
}

impl CpuCompositor {
    pub fn new() -> Self {
        Self::default()
    }

    /// One-shot full render.
    pub fn render(scene: &RenderScene) -> Result<Frame, CompositorError> {
        let mut c = CpuCompositor::new();
        c.update(scene, &DirtyRegion::all())?;
        c.frame().ok_or(CompositorError::InvalidSize {
            width: scene.width,
            height: scene.height,
        })
    }

    /// The last frame (row-major copy).
    pub fn frame(&self) -> Option<Frame> {
        self.frame.as_ref().map(TiledCanvas::to_frame)
    }

    /// The last frame in its tile-major form (zero-copy upload source).
    pub fn canvas(&self) -> Option<&TiledCanvas> {
        self.frame.as_ref()
    }

    /// Brings the frame up to date with `scene`, redrawing only damaged
    /// tiles.
    pub fn update(
        &mut self,
        scene: &RenderScene,
        dirty: &DirtyRegion,
    ) -> Result<RenderStats, CompositorError> {
        let size0 = (scene.width, scene.height);
        if dirty.all {
            self.cache.clear();
        }
        // 1. Prepare what changed and note where it changed.
        let mut stages = Vec::with_capacity(scene.nodes.len());
        let mut plan: Vec<Arc<Vec<CpuOp>>> = Vec::with_capacity(scene.nodes.len());
        let mut node_damage: Vec<Vec<Rect>> = Vec::with_capacity(scene.nodes.len());
        let mut size = size0;
        for n in &scene.nodes {
            stages.push(size);
            let fresh = self.cache.get(&n.id).is_some_and(|c| {
                c.revision == n.revision && c.transform == n.transform && c.stage == size
            });
            let mut damage = Vec::new();
            if !fresh {
                let prepared = prepare(&n.node, n.transform.as_ref(), size);
                let bounds = prepared.bounds(size);
                let out_size = prepared.out_size(size);
                let raster = match &n.node {
                    RenderNode::Raster(r) if n.transform.is_none() => {
                        Some((r.surface.clone(), r.origin))
                    }
                    _ => None,
                };
                match self.cache.get(&n.id) {
                    Some(old) if old.stage == size => match (&old.raster, &raster) {
                        // Same placement: only the replaced tiles changed.
                        (Some((a, oa)), Some((b, ob)))
                            if oa == ob
                                && a.tile_bounds() == b.tile_bounds()
                                && old.bounds == bounds =>
                        {
                            damage.extend(
                                changed_tiles(a, b)
                                    .into_iter()
                                    .map(|r| r.translate(ob[0], ob[1])),
                            );
                            if old.ops.len() != prepared.ops.len() {
                                damage.push(bounds);
                            }
                        }
                        _ => damage.extend([old.bounds, bounds]),
                    },
                    Some(old) => damage.extend([old.bounds, bounds]),
                    None => damage.push(bounds),
                }
                let ops = Arc::new(resolve(&prepared));
                self.cache.insert(
                    n.id,
                    CachedNode {
                        revision: n.revision,
                        transform: n.transform,
                        stage: size,
                        ops,
                        bounds,
                        out_size,
                        raster,
                    },
                );
            }
            let Some(c) = self.cache.get(&n.id) else {
                continue;
            };
            plan.push(c.ops.clone());
            node_damage.push(damage);
            size = c.out_size;
        }
        let final_size = size;
        let ids: HashSet<NodeId> = scene.nodes.iter().map(|n| n.id).collect();
        self.cache.retain(|id, _| ids.contains(id));

        let bg = scene.background.premultiplied();
        let shape = SceneShape {
            size: size0,
            background: bg.map(f32::to_bits),
            nodes: scene.nodes.iter().map(|n| n.id).collect(),
            stages: stages.clone(),
        };
        let full = dirty.all
            || self.shape.as_ref() != Some(&shape)
            || ids.len() != scene.nodes.len()
            || self
                .frame
                .as_ref()
                .is_none_or(|f| (f.width(), f.height()) != final_size);

        // 2. Damage in final-frame tiles.
        let damage = if full {
            TileSet::full(final_size.0, final_size.1)
        } else {
            // Exact rectangles while they stay few, so a blur widens the
            // damage by its radius rather than by whole tiles.
            let mut d: Vec<Rect> = dirty.rects.clone();
            let mut s = size0;
            for (ops, extra) in plan.iter().zip(&node_damage) {
                for op in ops.iter() {
                    d = d
                        .iter()
                        .flat_map(|r| op.op.forward(*r, s))
                        .filter(|r| !r.is_empty())
                        .collect();
                    s = op.op.out_size(s);
                    coalesce(&mut d, s);
                }
                d.extend(extra.iter().filter(|r| !r.is_empty()));
                coalesce(&mut d, s);
            }
            let mut t = TileSet::empty(final_size.0, final_size.1);
            d.iter().for_each(|r| t.insert_rect(*r));
            t
        };
        self.shape = Some(shape);
        let mut stats = RenderStats {
            full,
            tiles: damage.count(),
            damage: damage.rects().collect(),
        };
        if damage.is_empty() {
            return Ok(stats);
        }

        // 3. Tiles each op must produce, pulled backwards from the damage.
        let flat: Vec<(&CpuOp, (u32, u32))> = {
            let mut s = size0;
            let mut v = Vec::new();
            for ops in &plan {
                for op in ops.iter() {
                    v.push((op, s));
                    s = op.op.out_size(s);
                }
            }
            v
        };
        let mut needed = vec![damage.clone()];
        for (op, in_size) in flat.iter().rev() {
            let Some(out) = needed.last() else { break };
            let w = op.op.write_rect(*in_size);
            let mut s = TileSet::empty(in_size.0, in_size.1);
            for r in out.rects() {
                if !op.op.resizes() {
                    s.insert_rect(r);
                }
                let rw = r.intersect(&w);
                if !rw.is_empty() {
                    op.op
                        .read_rects(rw, *in_size)
                        .into_iter()
                        .for_each(|rr| s.insert_rect(rr));
                }
            }
            needed.push(s);
        }
        needed.reverse();

        // 4. Run the ops on the needed tiles.
        let mut canvas = match self.scratch.take() {
            Some(c) if (c.width(), c.height()) == size0 => c,
            _ => TiledCanvas::new(size0.0, size0.1)?,
        };
        canvas.par_tiles(&needed[0], |_, px| px.iter_mut().for_each(|p| *p = bg));
        for (k, (op, in_size)) in flat.iter().enumerate() {
            let targets = &needed[k + 1];
            if let Op::Cut(p) = &op.op {
                let mut out = TiledCanvas::new(p.dst_w, p.dst_h)?;
                apply_cut(p, &canvas, &mut out, targets);
                canvas = out;
                continue;
            }
            if let Op::Frame(f) = &op.op {
                let mut out = TiledCanvas::new(f.dst.0, f.dst.1)?;
                apply_frame(f, &canvas, &mut out, targets);
                canvas = out;
                continue;
            }
            let w = op.op.write_rect(*in_size);
            let write = targets.restricted(w);
            if !write.is_empty() {
                apply(op, &mut canvas, &write, w, *in_size);
            }
        }
        if full {
            self.scratch = self.frame.take();
            self.frame = Some(canvas);
        } else if let Some(frame) = &mut self.frame {
            frame.copy_tiles(&canvas, &damage);
            self.scratch = Some(canvas);
        }
        stats.tiles = damage.count();
        Ok(stats)
    }
}

/// Bounds the damage list: past 64 rectangles it degrades to tile rects.
fn coalesce(d: &mut Vec<Rect>, size: (u32, u32)) {
    if d.len() <= 64 {
        return;
    }
    let mut t = TileSet::empty(size.0, size.1);
    d.iter().for_each(|r| t.insert_rect(*r));
    *d = t.rects().collect();
}

/// Index of `(x, y)` inside the tile whose clipped rectangle is `tr`.
#[inline]
fn local(tr: Rect, x: i32, y: i32) -> usize {
    (y - tr.y0) as usize * TILE + (x - tr.x0) as usize
}

fn each_pixel(tr: Rect, w: Rect, px: &mut [Px], mut f: impl FnMut(i32, i32, &mut Px)) {
    let r = tr.intersect(&w);
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            if let Some(p) = px.get_mut(local(tr, x, y)) {
                f(x, y, p);
            }
        }
    }
}

/// Separable blur of the canvas over `o` (edge pixels repeat), mirroring
/// `blur_rgba_h` + `blur_rgba_v`. Returns row-major values over `o`.
fn blur_region(canvas: &TiledCanvas, o: Rect, k: &Kernel) -> Vec<Px> {
    let (cw, ch) = (canvas.width() as i32, canvas.height() as i32);
    let r = k.radius as i32;
    let (y0, y1) = ((o.y0 - r).max(0), (o.y1 + r).min(ch));
    let snap = canvas.read_rect(Rect::new(o.x0 - r, y0, o.x1 + r, y1));
    let tw = o.width() as usize;
    let mut tmp = vec![[0.0f32; 4]; tw * (y1 - y0).max(0) as usize];
    tmp.par_chunks_mut(tw.max(1))
        .enumerate()
        .for_each(|(row, out)| {
            let y = y0 + row as i32;
            for (i, v) in out.iter_mut().enumerate() {
                let x = o.x0 + i as i32;
                let mut acc = [0.0f32; 4];
                for (kk, wt) in k.weights.iter().enumerate() {
                    let p = snap.get((x + kk as i32 - r).clamp(0, cw - 1), y);
                    for c in 0..4 {
                        acc[c] += wt * p[c];
                    }
                }
                *v = acc;
            }
        });
    let mut out = vec![[0.0f32; 4]; tw * o.height() as usize];
    out.par_chunks_mut(tw.max(1))
        .enumerate()
        .for_each(|(row, dst)| {
            let y = o.y0 + row as i32;
            for (i, v) in dst.iter_mut().enumerate() {
                let mut acc = [0.0f32; 4];
                for (kk, wt) in k.weights.iter().enumerate() {
                    let sy = (y + kk as i32 - r).clamp(0, ch - 1);
                    let p = tmp
                        .get((sy - y0) as usize * tw + i)
                        .copied()
                        .unwrap_or([0.0; 4]);
                    for c in 0..4 {
                        acc[c] += wt * p[c];
                    }
                }
                *v = acc;
            }
        });
    out
}

/// `photocraft_algo` filter of the canvas over `o`, reading `src` (edge
/// pixels repeat outside the canvas). The canvas is premultiplied, so the
/// filter runs without alpha handling. Returns row-major values over `o`.
fn filter_region(
    canvas: &TiledCanvas,
    params: &FilterParams,
    bounds: Rect,
    src: Rect,
    o: Rect,
) -> Vec<Px> {
    let c = canvas.rect();
    let mut img = Image::new(src, 4);
    if c.is_empty() || src.is_empty() || o.is_empty() {
        return Vec::new();
    }
    let snap = canvas.read_rect(src);
    img.data
        .par_chunks_mut(src.width() as usize * 4)
        .enumerate()
        .for_each(|(row, out)| {
            let y = (src.y0 + row as i32).clamp(c.y0, c.y1 - 1);
            for (i, p) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let x = (src.x0 + i as i32).clamp(c.x0, c.x1 - 1);
                *p = snap.get(x, y);
            }
        });
    let ctx = Ctx {
        bounds,
        mode: ColorMode::Rgb,
        alpha: false,
    };
    photocraft_algo::kernel(params, &img, o, &ctx)
        .as_chunks::<4>()
        .0
        .to_vec()
}

/// Mixes `vals` (row-major over `o`) into the canvas by `mask`.
fn blend_masked(
    canvas: &mut TiledCanvas,
    write: &TileSet,
    w: Rect,
    mask: &Mask,
    o: Rect,
    vals: &[Px],
) {
    let ow = o.width() as usize;
    canvas.par_tiles(write, |tr, px| {
        each_pixel(tr, w, px, |x, y, p| {
            let m = mask.at(x, y);
            if m > 0.0
                && let Some(v) = vals.get((y - o.y0) as usize * ow + (x - o.x0) as usize)
            {
                *p = mix(*p, *v, m);
            }
        })
    });
}

fn apply_image(o: &ImageOp, canvas: &mut TiledCanvas, write: &TileSet, w: Rect) {
    canvas.par_tiles(write, |tr, px| {
        let r = tr.intersect(&w);
        if r.is_empty() {
            return;
        }
        let mut buf = vec![[0.0f32; 4]; r.width() as usize * r.height() as usize];
        o.surface
            .read_rgba_into(r.translate(-o.origin[0], -o.origin[1]), &mut buf);
        let rw = r.width() as usize;
        each_pixel(tr, r, px, |x, y, p| {
            let s = buf[(y - r.y0) as usize * rw + (x - r.x0) as usize];
            let pm = [s[0] * s[3], s[1] * s[3], s[2] * s[3], s[3]];
            *p = over(scale(pm, o.opacity), *p);
        });
    });
}

fn apply(op: &CpuOp, canvas: &mut TiledCanvas, write: &TileSet, w: Rect, size: (u32, u32)) {
    match &op.op {
        Op::Image(o) => apply_image(o, canvas, write, w),
        Op::Fill(f) => {
            let Some(cov) = &op.coverage else { return };
            canvas.par_tiles(write, |tr, px| {
                each_pixel(tr, w, px, |x, y, p| {
                    let m = cov.at(x, y);
                    if m > 0.0 {
                        *p = over(scale(f.paint.at(x, y), m), *p);
                    }
                })
            });
        }
        Op::Blur(b) => {
            let o = write.bounds_within(w);
            let src = o.inflate(b.reach());
            let vals = filter_region(canvas, &b.params(), canvas.rect(), src, o);
            blend_masked(canvas, write, w, &b.mask, o, &vals);
        }
        Op::Pixelate(pz) => {
            let o = write.bounds_within(w);
            let (cells, _) = pz.blocks(o);
            let params = FilterParams::Mosaic {
                cell_size: pz.block as f32,
            };
            let vals = filter_region(canvas, &params, pz.grid(canvas.rect()), cells, o);
            blend_masked(canvas, write, w, &pz.mask, o, &vals);
        }
        Op::Lens(l) => {
            let o = write.bounds_within(w);
            let fp = crate::prepare::clamp_into(l.sampler.footprint(o), size);
            let snap = canvas.read_rect(fp);
            canvas.par_tiles(write, |tr, px| {
                each_pixel(tr, w, px, |x, y, p| {
                    let m = l.mask.at(x, y);
                    if m > 0.0 {
                        *p = mix(*p, l.sampler.sample(&|sx, sy| snap.get(sx, sy), x, y), m);
                    }
                })
            });
        }
        Op::Spotlight(s) => {
            let o = write.bounds_within(w);
            let blurred = s.blur.as_ref().map(|k| blur_region(canvas, o, k));
            let ow = o.width() as usize;
            let cov = op.coverage.clone();
            canvas.par_tiles(write, |tr, px| {
                each_pixel(tr, w, px, |x, y, p| {
                    let outside = cov.as_ref().map_or(1.0, |m| 1.0 - m.at(x, y));
                    if outside <= 0.0 {
                        return;
                    }
                    let f = blurred
                        .as_ref()
                        .map_or(*p, |v| v[(y - o.y0) as usize * ow + (x - o.x0) as usize]);
                    *p = mix(*p, dim(f, s.dim_rgb, s.dim_a), outside);
                })
            });
        }
        Op::Cut(_) | Op::Frame(_) => {}
    }
}

fn apply_frame(f: &FrameOp, src: &TiledCanvas, out: &mut TiledCanvas, targets: &TileSet) {
    let w = out.rect();
    out.par_tiles(targets, |tr, px| {
        each_pixel(tr, w, px, |x, y, v| {
            *v = f.compose(&|sx, sy| src.get(sx, sy), x, y);
        });
    });
}

fn apply_cut(
    p: &crate::effects::cutout::CutParams,
    src: &TiledCanvas,
    out: &mut TiledCanvas,
    targets: &TileSet,
) {
    let w = out.rect();
    out.par_tiles(targets, |tr, px| {
        let (a0, a1) = if p.vertical {
            (tr.y0, tr.y1)
        } else {
            (tr.x0, tr.x1)
        };
        // The tear depends on the along coordinate only: once per column.
        let cols: Vec<_> = (a0..a1)
            .map(|a| (p.edges(a as f32 + 0.5), p.shadow_edges(a as f32 + 0.5)))
            .collect();
        each_pixel(tr, w, px, |x, y, v| {
            let (a, c) = if p.vertical { (y, x) } else { (x, y) };
            if let Some((e, se)) = cols.get((a - a0) as usize) {
                *v = p.compose(&|sx, sy| src.get(sx, sy), a, c, *e, se.as_ref());
            }
        });
    });
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use crate::scene::{
        BackdropFilterKind, BackdropFilterSpec, Color, RectF32, SceneNode, ShapeGeometry, ShapeSpec,
    };

    fn scene(rect_x: f32) -> RenderScene {
        RenderScene {
            width: 700,
            height: 300,
            background: Color::WHITE,
            nodes: vec![
                SceneNode::new(
                    1,
                    1,
                    RenderNode::Shape(ShapeSpec {
                        geometry: ShapeGeometry::Rect(RectF32::new(rect_x, 40.0, 60.0, 50.0)),
                        fill: Some(Color::new(0.1, 0.4, 0.9, 1.0).into()),
                        stroke: None,
                        shadow: None,
                        text: None,
                    }),
                ),
                SceneNode::new(
                    2,
                    1,
                    RenderNode::BackdropFilter(BackdropFilterSpec {
                        shape: ShapeGeometry::Rect(RectF32::new(0.0, 0.0, 700.0, 120.0)),
                        filter: BackdropFilterKind::GaussianBlur { sigma: 3.0 },
                        feather_px: 0.0,
                    }),
                ),
            ],
        }
    }

    #[test]
    fn incremental_update_matches_a_full_render() {
        let mut c = CpuCompositor::new();
        let a = c
            .update(&scene(20.0), &DirtyRegion::none())
            .unwrap_or_default();
        assert!(a.full);
        let mut s = scene(30.0);
        s.nodes[0].revision = RevisionId(2);
        let b = c.update(&s, &DirtyRegion::none()).unwrap_or_default();
        assert!(!b.full);
        // The shape and its blurred neighbourhood stay inside the first tile.
        assert_eq!(b.tiles, 1);
        let full = CpuCompositor::render(&s).ok();
        assert_eq!(c.frame(), full);
        // Nothing changed: nothing redrawn.
        assert_eq!(
            c.update(&s, &DirtyRegion::none()).map(|s| s.tiles).ok(),
            Some(0)
        );
    }

    /// A blue square under a backdrop filter over `rect`.
    fn filtered(filter: BackdropFilterKind, rect: RectF32, feather_px: f32) -> Frame {
        let mut s = scene(20.0);
        s.nodes[1].node = RenderNode::BackdropFilter(BackdropFilterSpec {
            shape: ShapeGeometry::Rect(rect),
            filter,
            feather_px,
        });
        CpuCompositor::render(&s).unwrap_or_else(|e| panic!("render: {e}"))
    }

    fn px(f: &Frame, x: u32, y: u32) -> Px {
        f.pixels[(y * f.width + x) as usize]
    }

    #[test]
    fn pixelate_averages_whole_blocks_from_the_shape_corner() {
        let f = filtered(
            BackdropFilterKind::Pixelate { block_size_px: 10 },
            RectF32::new(15.0, 35.0, 80.0, 70.0),
            0.0,
        );
        // Block (15..25, 35..45): 5×5 of blue at x 20..25, y 40..45, rest white.
        let a = px(&f, 15, 35);
        assert_eq!(a, px(&f, 24, 44), "one colour per block");
        let blue = [0.1f32, 0.4, 0.9];
        for c in 0..3 {
            let want = (25.0 * blue[c] + 75.0) / 100.0;
            assert!(
                (a[c] - want).abs() < 1e-4,
                "channel {c}: {} vs {want}",
                a[c]
            );
        }
        // Outside the shape the square is untouched.
        assert_eq!(px(&f, 14, 50), [1.0; 4]);
    }

    #[test]
    fn feathered_pixelate_keeps_blocks_before_the_grid_origin_opaque() {
        // The feather reaches left of and above the grid origin (40, 40).
        let f = filtered(
            BackdropFilterKind::Pixelate { block_size_px: 12 },
            RectF32::new(40.0, 40.0, 60.0, 60.0),
            8.0,
        );
        for (x, y) in [(37, 50), (50, 37), (37, 37)] {
            let p = px(&f, x, y);
            assert!((p[3] - 1.0).abs() < 1e-5, "alpha at ({x}, {y}): {p:?}");
            assert!(p[0] > 0.1, "not darkened at ({x}, {y}): {p:?}");
        }
    }

    #[test]
    fn blur_at_the_canvas_edge_repeats_edge_pixels() {
        // A blur over a uniform corner stays uniform (no fade to transparent).
        let f = filtered(
            BackdropFilterKind::GaussianBlur { sigma: 12.0 },
            RectF32::new(500.0, 200.0, 200.0, 100.0),
            0.0,
        );
        for (x, y) in [(699, 299), (500, 299), (699, 200)] {
            let p = px(&f, x, y);
            assert!(
                p.iter().all(|v| (v - 1.0).abs() < 1e-4),
                "({x}, {y}): {p:?}"
            );
        }
        // Over the square's edge it mixes blue and white.
        let f = filtered(
            BackdropFilterKind::GaussianBlur { sigma: 6.0 },
            RectF32::new(0.0, 0.0, 200.0, 200.0),
            0.0,
        );
        let p = px(&f, 20, 60);
        assert!(p[0] > 0.2 && p[0] < 0.9, "{p:?}");
    }
}
