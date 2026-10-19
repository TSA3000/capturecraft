//! wgpu compositor for interactive editing.
//!
//! Runs the prepared ops of [`crate::prepare`] as compute dispatches
//! (`shaders/effects.wgsl`) over a storage-buffer canvas in the same
//! tile-major layout as [`TiledCanvas`]. Per node, everything that does not
//! change between frames lives on the GPU, keyed by `(NodeId, RevisionId,
//! transform)`: coverage masks (already blurred for shadows, feathered for
//! spotlights and blur regions), gradient stops, blur weights and raster
//! tiles. When a raster node gets a new revision with the same
//! tile grid, only the tiles whose copy-on-write pointer changed are
//! uploaded again. A frame then costs one dispatch chain and no uploads.
//!
//! The compositor works on any device: the app shares eframe's, tests and
//! headless tools use [`GpuCompositor::headless`] (hardware first, then
//! the platform's software adapter).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use photocraft_geom::{Affine, Rect, TILE_SIZE, TileCoord};
use photocraft_raster::Surface;
use wgpu::util::DeviceExt;

use crate::CompositorError;
use crate::canvas::{Frame, TILE_PX, TiledCanvas, tiles_for};
use crate::effects::backdrop::LensSampler;
use crate::effects::cutout::{CutParams, OCTAVES};
use crate::effects::{Kernel, PaintPx, Px};
use crate::prepare::{FrameOp, Op, PixelateOp, clamp_into, prepare};
use crate::scene::{NodeId, RenderNode, RenderScene, RevisionId};

const SHADER: &str = include_str!("shaders/effects.wgsl");
const PX_BYTES: u64 = 16;
const WG: u32 = 16;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GpuError {
    #[error("no usable GPU adapter: {0}")]
    NoAdapter(String),
    #[error("GPU device request failed: {0}")]
    Device(String),
    #[error("a {bytes}-byte GPU buffer exceeds the device limit of {limit} bytes")]
    TooLarge { bytes: u64, limit: u64 },
    #[error("GPU validation error: {0}")]
    Validation(String),
    #[error("GPU readback failed: {0}")]
    Readback(String),
    #[error(transparent)]
    Compositor(#[from] CompositorError),
}

// Uniform blocks, field for field as in the shader.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ClearParams {
    size: [u32; 4],
    color: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ImageParams {
    size: [u32; 4],
    rect: [i32; 4],
    src: [i32; 4],
    opacity: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FillParams {
    size: [u32; 4],
    rect: [i32; 4],
    mask_rect: [i32; 4],
    clip_rect: [i32; 4],
    flags: [u32; 4],
    color: [f32; 4],
    paint: [u32; 4],
    geo: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaskBlurParams {
    dims: [i32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlurParams {
    size: [u32; 4],
    rect: [i32; 4],
    src_rect: [i32; 4],
    radius: [i32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MixParams {
    size: [u32; 4],
    rect: [i32; 4],
    mask_rect: [i32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MosaicParams {
    size: [u32; 4],
    rect: [i32; 4],
    mask_rect: [i32; 4],
    grid: [i32; 4],
    blocks: [i32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LensParams {
    size: [u32; 4],
    rect: [i32; 4],
    mask_rect: [i32; 4],
    mode: [u32; 4],
    geo: [f32; 4],
    zoom: [f32; 4],
    grid: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SpotParams {
    size: [u32; 4],
    rect: [i32; 4],
    mask_rect: [i32; 4],
    flags: [u32; 4],
    dim: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CutUniform {
    src: [u32; 4],
    dst: [u32; 4],
    geo: [i32; 4],
    seeds: [u32; 4],
    noise: [f32; 4],
    shadow: [f32; 4],
    shadow_color: [f32; 4],
    gap_fill: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PresentParams {
    size: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    src: [u32; 4],
    dst: [u32; 4],
    shadow_rect: [i32; 4],
    clip_rect: [i32; 4],
    border_rect: [i32; 4],
    shadow_color: [f32; 4],
    border_color: [f32; 4],
    color: [f32; 4],
    paint: [u32; 4],
    geo: [f32; 4],
}

struct Pipelines {
    clear: wgpu::ComputePipeline,
    image_over: wgpu::ComputePipeline,
    fill: wgpu::ComputePipeline,
    blur_mask_h: wgpu::ComputePipeline,
    blur_mask_v: wgpu::ComputePipeline,
    blur_rgba_h: wgpu::ComputePipeline,
    blur_rgba_v: wgpu::ComputePipeline,
    mix_masked: wgpu::ComputePipeline,
    mosaic_reduce: wgpu::ComputePipeline,
    mosaic_apply: wgpu::ComputePipeline,
    magnifier: wgpu::ComputePipeline,
    spotlight: wgpu::ComputePipeline,
    cutout: wgpu::ComputePipeline,
    frame: wgpu::ComputePipeline,
    present: wgpu::ComputePipeline,
}

/// A paint on the GPU: the uniform fields of `paint_at` and the stops.
#[derive(Clone)]
struct GpuPaint {
    paint: [u32; 4],
    geo: [f32; 4],
    color: [f32; 4],
    stops: Option<wgpu::Buffer>,
}

/// A mask on the GPU (row-major over `rect`).
#[derive(Clone)]
struct GpuMask {
    buf: wgpu::Buffer,
    rect: Rect,
}

#[derive(Clone)]
enum GpuOp {
    Image {
        buf: wgpu::Buffer,
        src: [i32; 4],
        opacity: f32,
        grid: Rect,
    },
    Fill {
        mask: GpuMask,
        clip: Option<GpuMask>,
        invert: bool,
        paint: GpuPaint,
    },
    Blur {
        mask: GpuMask,
        weights: wgpu::Buffer,
        radius: i32,
    },
    Pixelate {
        mask: GpuMask,
        op: PixelateOp,
    },
    Lens {
        mask: GpuMask,
        sampler: LensSampler,
    },
    Spotlight {
        mask: Option<GpuMask>,
        blur: Option<(wgpu::Buffer, i32)>,
        dim: [f32; 4],
    },
    Cut(CutParams),
    Frame {
        op: Box<FrameOp>,
        backdrop: GpuPaint,
        shadow: Option<(GpuMask, Px)>,
        clip: GpuMask,
        border: Option<(GpuMask, Px)>,
    },
}

struct GpuNode {
    revision: RevisionId,
    transform: Option<Affine>,
    stage: (u32, u32),
    /// The prepared op (for its data-flow rectangles) and its GPU form.
    ops: Vec<(Op, GpuOp)>,
    out_size: (u32, u32),
    raster: Option<(Arc<Surface>, [i32; 2])>,
}

fn xyxy(r: Rect) -> [i32; 4] {
    [r.x0, r.y0, r.x1, r.y1]
}
fn xywh(r: Rect) -> [i32; 4] {
    [r.x0, r.y0, r.width() as i32, r.height() as i32]
}
fn canvas_bytes(size: (u32, u32)) -> u64 {
    u64::from(tiles_for(size.0)) * u64::from(tiles_for(size.1)) * TILE_PX as u64 * PX_BYTES
}

/// Wgpu backend; see the module docs.
pub struct GpuCompositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipes: Pipelines,
    cache: HashMap<NodeId, GpuNode>,
    canvases: Vec<(u64, wgpu::Buffer)>,
    scratch: Option<(u64, wgpu::Buffer)>,
    tmp: [Option<(u64, wgpu::Buffer)>; 2],
    dummy: wgpu::Buffer,
    adapter_info: Option<wgpu::AdapterInfo>,
}

impl GpuCompositor {
    /// Builds the pipelines on an existing device (e.g. eframe's).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self, GpuError> {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("capturecraft effects"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let p = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pipes = Pipelines {
            clear: p("clear"),
            image_over: p("image_over"),
            fill: p("fill"),
            blur_mask_h: p("blur_mask_h"),
            blur_mask_v: p("blur_mask_v"),
            blur_rgba_h: p("blur_rgba_h"),
            blur_rgba_v: p("blur_rgba_v"),
            mix_masked: p("mix_masked"),
            mosaic_reduce: p("mosaic_reduce"),
            mosaic_apply: p("mosaic_apply"),
            magnifier: p("magnifier"),
            spotlight: p("spotlight"),
            cutout: p("cutout"),
            frame: p("frame"),
            present: p("present"),
        };
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(GpuError::Validation(e.to_string()));
        }
        let dummy = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dummy"),
            contents: &[0u8; 16],
            usage: wgpu::BufferUsages::STORAGE,
        });
        Ok(Self {
            device: device.clone(),
            queue: queue.clone(),
            pipes,
            cache: HashMap::new(),
            canvases: Vec::new(),
            scratch: None,
            tmp: [None, None],
            dummy,
            adapter_info: None,
        })
    }

    /// Own device for headless rendering: the best hardware adapter, else
    /// the software fallback (llvmpipe/lavapipe, WARP, …).
    pub fn headless() -> Result<Self, GpuError> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        // GL lacks storage buffers in places; use it only when asked for
        // (WGPU_BACKEND=gl).
        if std::env::var_os("WGPU_BACKEND").is_none() {
            desc.backends = wgpu::Backends::PRIMARY;
        }
        let instance = wgpu::Instance::new(desc);
        let request = |fallback| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: fallback,
                compatible_surface: None,
                ..Default::default()
            }))
        };
        let adapter = request(false)
            .or_else(|_| request(true))
            .map_err(|e| GpuError::NoAdapter(e.to_string()))?;
        let al = adapter.limits();
        let required_limits = wgpu::Limits {
            max_storage_buffer_binding_size: al.max_storage_buffer_binding_size,
            max_buffer_size: al.max_buffer_size,
            ..wgpu::Limits::default()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("capturecraft compositor"),
            required_limits,
            ..Default::default()
        }))
        .map_err(|e| GpuError::Device(e.to_string()))?;
        let mut c = Self::new(&device, &queue)?;
        c.adapter_info = Some(adapter.get_info());
        Ok(c)
    }

    /// Adapter of a [`GpuCompositor::headless`] device.
    pub fn adapter_info(&self) -> Option<&wgpu::AdapterInfo> {
        self.adapter_info.as_ref()
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Drops every cached node (e.g. when another document is shown and
    /// node ids start over).
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    fn check(&self, bytes: u64) -> Result<(), GpuError> {
        let l = self.device.limits();
        let limit = l.max_storage_buffer_binding_size.min(l.max_buffer_size);
        if bytes > limit {
            Err(GpuError::TooLarge { bytes, limit })
        } else {
            Ok(())
        }
    }

    fn storage_f32(&self, data: &[f32]) -> wgpu::Buffer {
        let contents: &[u8] = if data.is_empty() {
            &[0u8; 4]
        } else {
            bytemuck::cast_slice(data)
        };
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents,
                usage: wgpu::BufferUsages::STORAGE,
            })
    }

    fn buffer(&self, bytes: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes.max(16),
            usage,
            mapped_at_creation: false,
        })
    }

    /// Grow-only temporary buffer `i`.
    fn tmp(&mut self, i: usize, bytes: u64) -> Result<wgpu::Buffer, GpuError> {
        self.check(bytes)?;
        let bytes = bytes.max(16);
        if let Some((cap, b)) = &self.tmp[i]
            && *cap >= bytes
        {
            return Ok(b.clone());
        }
        let b = self.buffer(bytes, wgpu::BufferUsages::STORAGE);
        self.tmp[i] = Some((bytes, b.clone()));
        Ok(b)
    }

    fn canvas_buffer(&mut self, slot: usize, size: (u32, u32)) -> Result<wgpu::Buffer, GpuError> {
        let bytes = canvas_bytes(size);
        self.check(bytes)?;
        if let Some((cap, b)) = self.canvases.get(slot)
            && *cap == bytes
        {
            return Ok(b.clone());
        }
        let b = self.buffer(
            bytes,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        if slot < self.canvases.len() {
            self.canvases[slot] = (bytes, b.clone());
        } else {
            self.canvases.push((bytes, b.clone()));
        }
        Ok(b)
    }

    fn scratch_buffer(&mut self, bytes: u64) -> Result<wgpu::Buffer, GpuError> {
        self.check(bytes)?;
        if let Some((cap, b)) = &self.scratch
            && *cap >= bytes
        {
            return Ok(b.clone());
        }
        let b = self.buffer(
            bytes,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        self.scratch = Some((bytes, b.clone()));
        Ok(b)
    }

    /// One dispatch: uniform at `ub`, storage buffers by binding.
    fn run<T: Pod>(
        &self,
        enc: &mut wgpu::CommandEncoder,
        pipe: &wgpu::ComputePipeline,
        ub: u32,
        params: &T,
        bufs: &[(u32, &wgpu::Buffer)],
        extent: (u32, u32),
    ) {
        if extent.0 == 0 || extent.1 == 0 {
            return;
        }
        let u = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let mut entries: Vec<wgpu::BindGroupEntry> = bufs
            .iter()
            .map(|(b, buf)| wgpu::BindGroupEntry {
                binding: *b,
                resource: buf.as_entire_binding(),
            })
            .collect();
        entries.push(wgpu::BindGroupEntry {
            binding: ub,
            resource: u.as_entire_binding(),
        });
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipe.get_bind_group_layout(0),
            entries: &entries,
        });
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &bg, &[]);
        pass.dispatch_workgroups(extent.0.div_ceil(WG), extent.1.div_ceil(WG), 1);
    }

    /// Uploads a mask; blurs it on the GPU first when `blur` is given.
    fn mask(
        &self,
        enc: &mut wgpu::CommandEncoder,
        m: &crate::effects::Mask,
        blur: Option<&Kernel>,
    ) -> Result<GpuMask, GpuError> {
        self.check(m.data.len() as u64 * 4)?;
        let raw = self.storage_f32(&m.data);
        let Some(k) = blur else {
            return Ok(GpuMask {
                buf: raw,
                rect: m.rect,
            });
        };
        let bytes = m.data.len() as u64 * 4;
        let weights = self.storage_f32(&k.weights);
        let tmp = self.buffer(bytes, wgpu::BufferUsages::STORAGE);
        let out = self.buffer(bytes, wgpu::BufferUsages::STORAGE);
        let (w, h) = (m.rect.width(), m.rect.height());
        let p = MaskBlurParams {
            dims: [w as i32, h as i32, k.radius as i32, 0],
        };
        self.run(
            enc,
            &self.pipes.blur_mask_h,
            13,
            &p,
            &[(2, &raw), (6, &weights), (7, &tmp)],
            (w, h),
        );
        self.run(
            enc,
            &self.pipes.blur_mask_v,
            13,
            &p,
            &[(2, &tmp), (6, &weights), (7, &out)],
            (w, h),
        );
        Ok(GpuMask {
            buf: out,
            rect: m.rect,
        })
    }

    fn paint(&self, p: &PaintPx) -> GpuPaint {
        match p {
            PaintPx::Solid(c) => GpuPaint {
                paint: [0; 4],
                geo: [0.0; 4],
                color: *c,
                stops: None,
            },
            PaintPx::Gradient(g) => {
                let data: Vec<f32> = g
                    .stops
                    .iter()
                    .flat_map(|(o, c)| [*o, 0.0, 0.0, 0.0, c[0], c[1], c[2], c[3]])
                    .collect();
                GpuPaint {
                    paint: [if g.radial { 2 } else { 1 }, g.stops.len() as u32, 0, 0],
                    geo: g.geo,
                    color: [0.0; 4],
                    stops: Some(self.storage_f32(&data)),
                }
            }
        }
    }

    /// Writes surface tiles `coords` into an image buffer whose tile grid
    /// covers `grid` (surface coordinates).
    fn upload_tiles(
        &self,
        buf: &wgpu::Buffer,
        surface: &Surface,
        grid: Rect,
        coords: impl Iterator<Item = TileCoord>,
    ) {
        let (gx, gy) = (grid.x0.div_euclid(TILE_SIZE), grid.y0.div_euclid(TILE_SIZE));
        let ntx = grid.width() as i32 / TILE_SIZE;
        let mut px = vec![[0.0f32; 4]; TILE_PX];
        for c in coords {
            if !grid.contains(c.tx * TILE_SIZE, c.ty * TILE_SIZE) {
                continue;
            }
            surface.read_rgba_into(c.rect(), &mut px);
            for p in &mut px {
                *p = [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]];
            }
            let idx = ((c.ty - gy) * ntx + (c.tx - gx)) as u64;
            self.queue.write_buffer(
                buf,
                idx * TILE_PX as u64 * PX_BYTES,
                bytemuck::cast_slice(&px),
            );
        }
    }

    fn build_node(
        &self,
        enc: &mut wgpu::CommandEncoder,
        node: &RenderNode,
        transform: Option<&Affine>,
        size: (u32, u32),
        old: Option<GpuNode>,
    ) -> Result<GpuNode, GpuError> {
        let prepared = prepare(node, transform, size);
        let raster = match node {
            RenderNode::Raster(r) if transform.is_none() => Some((r.surface.clone(), r.origin)),
            _ => None,
        };
        let mut ops = Vec::with_capacity(prepared.ops.len());
        for op in &prepared.ops {
            let g = match op {
                Op::Image(o) => {
                    let grid = o.surface.tile_bounds();
                    let (ntx, nty) = (
                        grid.width() as i32 / TILE_SIZE,
                        grid.height() as i32 / TILE_SIZE,
                    );
                    // Same grid as the previous revision: re-upload only
                    // the tiles that were replaced.
                    let reuse = old.as_ref().and_then(|n| {
                        let (os, _) = n.raster.as_ref()?;
                        n.ops.iter().find_map(|(_, g)| match g {
                            GpuOp::Image { buf, grid: og, .. } if *og == grid => {
                                Some((buf.clone(), os.clone()))
                            }
                            _ => None,
                        })
                    });
                    let buf = match reuse {
                        Some((buf, os)) => {
                            let changed = crate::cpu_compositor::changed_tiles(&os, &o.surface);
                            self.upload_tiles(
                                &buf,
                                &o.surface,
                                grid,
                                changed
                                    .into_iter()
                                    .map(|r| TileCoord::containing(r.x0, r.y0)),
                            );
                            buf
                        }
                        None => {
                            let bytes = (ntx * nty) as u64 * TILE_PX as u64 * PX_BYTES;
                            self.check(bytes)?;
                            let buf = self.buffer(
                                bytes,
                                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                            );
                            self.upload_tiles(&buf, &o.surface, grid, grid.tiles());
                            buf
                        }
                    };
                    GpuOp::Image {
                        buf,
                        src: [o.region.x0, o.region.y0, ntx, nty],
                        opacity: o.opacity,
                        grid,
                    }
                }
                Op::Fill(f) => {
                    let mask = self.mask(enc, &f.mask, f.blur.as_deref())?;
                    let clip = f
                        .clip
                        .as_ref()
                        .map(|c| self.mask(enc, c, None))
                        .transpose()?;
                    GpuOp::Fill {
                        mask,
                        clip,
                        invert: f.invert,
                        paint: self.paint(&f.paint),
                    }
                }
                Op::Blur(b) => GpuOp::Blur {
                    mask: self.mask(enc, &b.mask, None)?,
                    weights: self.storage_f32(&b.kernel.weights),
                    radius: b.kernel.radius as i32,
                },
                Op::Pixelate(p) => GpuOp::Pixelate {
                    mask: self.mask(enc, &p.mask, None)?,
                    op: p.clone(),
                },
                Op::Lens(l) => GpuOp::Lens {
                    mask: self.mask(enc, &l.mask, None)?,
                    sampler: l.sampler,
                },
                Op::Spotlight(s) => GpuOp::Spotlight {
                    mask: s
                        .mask
                        .as_ref()
                        .map(|m| self.mask(enc, m, None))
                        .transpose()?,
                    blur: s
                        .blur
                        .as_ref()
                        .map(|k| (self.storage_f32(&k.weights), k.radius as i32)),
                    dim: [s.dim_rgb[0], s.dim_rgb[1], s.dim_rgb[2], s.dim_a],
                },
                Op::Cut(c) => GpuOp::Cut(*c),
                Op::Frame(f) => GpuOp::Frame {
                    op: Box::new(f.clone()),
                    backdrop: self.paint(&f.backdrop),
                    shadow: f
                        .shadow
                        .as_ref()
                        .map(|(m, c)| Ok::<_, GpuError>((self.mask(enc, m, None)?, *c)))
                        .transpose()?,
                    clip: self.mask(enc, &f.clip, None)?,
                    border: f
                        .border
                        .as_ref()
                        .map(|(m, c)| Ok::<_, GpuError>((self.mask(enc, m, None)?, *c)))
                        .transpose()?,
                },
            };
            ops.push((op.clone(), g));
        }
        Ok(GpuNode {
            revision: RevisionId::default(),
            transform: transform.copied(),
            stage: size,
            ops,
            out_size: prepared.out_size(size),
            raster,
        })
    }

    /// Separable backdrop blur of `rect` into tmp buffer 1 (row-major over
    /// `rect`).
    fn blur_backdrop(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        canvas: &wgpu::Buffer,
        size: (u32, u32),
        rect: Rect,
        weights: &wgpu::Buffer,
        radius: i32,
    ) -> Result<wgpu::Buffer, GpuError> {
        let su = [size.0, size.1, tiles_for(size.0), 0];
        let h = Rect::new(
            rect.x0,
            (rect.y0 - radius).max(0),
            rect.x1,
            (rect.y1 + radius).min(size.1 as i32),
        );
        let ta = self.tmp(0, h.width() as u64 * h.height() as u64 * PX_BYTES)?;
        let tb = self.tmp(1, rect.width() as u64 * rect.height() as u64 * PX_BYTES)?;
        let p = BlurParams {
            size: su,
            rect: xyxy(h),
            src_rect: xyxy(h),
            radius: [radius, 0, 0, 0],
        };
        self.run(
            enc,
            &self.pipes.blur_rgba_h,
            14,
            &p,
            &[(1, canvas), (5, &ta), (6, weights)],
            (h.width(), h.height()),
        );
        let p = BlurParams {
            size: su,
            rect: xyxy(rect),
            src_rect: xyxy(h),
            radius: [radius, 0, 0, 0],
        };
        self.run(
            enc,
            &self.pipes.blur_rgba_v,
            14,
            &p,
            &[(4, &ta), (5, &tb), (6, weights)],
            (rect.width(), rect.height()),
        );
        Ok(tb)
    }

    /// Encodes the scene; returns the final canvas buffer and size.
    fn encode(
        &mut self,
        scene: &RenderScene,
        enc: &mut wgpu::CommandEncoder,
    ) -> Result<(wgpu::Buffer, (u32, u32)), GpuError> {
        let size0 = (scene.width, scene.height);
        if size0.0 == 0
            || size0.1 == 0
            || size0.0 > crate::canvas::MAX_EDGE
            || size0.1 > crate::canvas::MAX_EDGE
        {
            return Err(CompositorError::InvalidSize {
                width: size0.0,
                height: size0.1,
            }
            .into());
        }
        // Node resources.
        let mut size = size0;
        let mut order = Vec::with_capacity(scene.nodes.len());
        for n in &scene.nodes {
            let fresh = self.cache.get(&n.id).is_some_and(|c| {
                c.revision == n.revision && c.transform == n.transform && c.stage == size
            });
            if !fresh {
                let old = self.cache.remove(&n.id);
                let mut node = self.build_node(enc, &n.node, n.transform.as_ref(), size, old)?;
                node.revision = n.revision;
                self.cache.insert(n.id, node);
            }
            order.push((n.id, size));
            size = self.cache.get(&n.id).map_or(size, |c| c.out_size);
        }
        let ids: HashSet<NodeId> = scene.nodes.iter().map(|n| n.id).collect();
        self.cache.retain(|id, _| ids.contains(id));

        // Composite.
        let mut slot = 0;
        let mut canvas = self.canvas_buffer(slot, size0)?;
        let su = |s: (u32, u32)| [s.0, s.1, tiles_for(s.0), 0];
        let bg = scene.background.premultiplied();
        self.run(
            enc,
            &self.pipes.clear,
            10,
            &ClearParams {
                size: su(size0),
                color: bg,
            },
            &[(0, &canvas)],
            size0,
        );
        let mut size = size0;
        for (id, stage) in order {
            let Some(node) = self.cache.get(&id) else {
                continue;
            };
            if stage != size {
                continue;
            }
            let ops: Vec<(Op, GpuOp)> = node.ops.clone();
            for (op, g) in &ops {
                let w = op.write_rect(size);
                let s = su(size);
                match g {
                    GpuOp::Image {
                        buf, src, opacity, ..
                    } => {
                        let p = ImageParams {
                            size: s,
                            rect: xyxy(w),
                            src: *src,
                            opacity: [*opacity; 4],
                        };
                        self.run(
                            enc,
                            &self.pipes.image_over,
                            11,
                            &p,
                            &[(0, &canvas), (8, buf)],
                            (w.width(), w.height()),
                        );
                    }
                    GpuOp::Fill {
                        mask,
                        clip,
                        invert,
                        paint,
                    } => {
                        let (cb, cr) = clip
                            .as_ref()
                            .map_or((&mask.buf, Rect::EMPTY), |c| (&c.buf, c.rect));
                        let p = FillParams {
                            size: s,
                            rect: xyxy(w),
                            mask_rect: xywh(mask.rect),
                            clip_rect: xywh(cr),
                            flags: [u32::from(clip.is_some()), u32::from(*invert), 0, 0],
                            color: paint.color,
                            paint: paint.paint,
                            geo: paint.geo,
                        };
                        let stops = paint.stops.as_ref().unwrap_or(&self.dummy);
                        self.run(
                            enc,
                            &self.pipes.fill,
                            12,
                            &p,
                            &[(0, &canvas), (2, &mask.buf), (3, cb), (9, stops)],
                            (w.width(), w.height()),
                        );
                    }
                    GpuOp::Blur {
                        mask,
                        weights,
                        radius,
                    } => {
                        let tb = self.blur_backdrop(enc, &canvas, size, w, weights, *radius)?;
                        let p = MixParams {
                            size: s,
                            rect: xyxy(w),
                            mask_rect: xywh(mask.rect),
                        };
                        self.run(
                            enc,
                            &self.pipes.mix_masked,
                            15,
                            &p,
                            &[(0, &canvas), (2, &mask.buf), (4, &tb)],
                            (w.width(), w.height()),
                        );
                    }
                    GpuOp::Pixelate { mask, op: pz } => {
                        let (_, blocks) = pz.blocks(w);
                        let ta = self.tmp(0, (blocks[2] * blocks[3]) as u64 * PX_BYTES)?;
                        let p = MosaicParams {
                            size: s,
                            rect: xyxy(w),
                            mask_rect: xywh(mask.rect),
                            grid: [pz.origin[0], pz.origin[1], pz.block, 0],
                            blocks,
                        };
                        self.run(
                            enc,
                            &self.pipes.mosaic_reduce,
                            16,
                            &p,
                            &[(1, &canvas), (5, &ta)],
                            (blocks[2] as u32, blocks[3] as u32),
                        );
                        self.run(
                            enc,
                            &self.pipes.mosaic_apply,
                            16,
                            &p,
                            &[(0, &canvas), (2, &mask.buf), (4, &ta)],
                            (w.width(), w.height()),
                        );
                    }
                    GpuOp::Lens { mask, sampler } => {
                        // Snapshot the tile rows the lens reads.
                        let fp = clamp_into(sampler.footprint(w), size);
                        let row_bytes = u64::from(tiles_for(size.0)) * TILE_PX as u64 * PX_BYTES;
                        let (ty0, ty1) = (
                            (fp.y0 / TILE_SIZE) as u64,
                            ((fp.y1 - 1) / TILE_SIZE + 1) as u64,
                        );
                        let scratch = self.scratch_buffer(canvas_bytes(size))?;
                        enc.copy_buffer_to_buffer(
                            &canvas,
                            ty0 * row_bytes,
                            &scratch,
                            ty0 * row_bytes,
                            (ty1 - ty0) * row_bytes,
                        );
                        let p = LensParams {
                            size: s,
                            rect: xyxy(w),
                            mask_rect: xywh(mask.rect),
                            mode: [sampler.mode as u32, 0, 0, 0],
                            geo: [
                                sampler.center[0],
                                sampler.center[1],
                                sampler.source[0],
                                sampler.source[1],
                            ],
                            zoom: [sampler.inv_zoom, 0.0, 0.0, 0.0],
                            grid: sampler.grid,
                        };
                        self.run(
                            enc,
                            &self.pipes.magnifier,
                            17,
                            &p,
                            &[(0, &canvas), (1, &scratch), (2, &mask.buf)],
                            (w.width(), w.height()),
                        );
                    }
                    GpuOp::Spotlight { mask, blur, dim } => {
                        let blurred = match blur {
                            Some((weights, radius)) => {
                                Some(self.blur_backdrop(enc, &canvas, size, w, weights, *radius)?)
                            }
                            None => None,
                        };
                        let p = SpotParams {
                            size: s,
                            rect: xyxy(w),
                            mask_rect: mask.as_ref().map_or([0; 4], |m| xywh(m.rect)),
                            flags: [
                                u32::from(mask.is_some()),
                                u32::from(blurred.is_some()),
                                0,
                                0,
                            ],
                            dim: *dim,
                        };
                        let mb = mask.as_ref().map_or(&self.dummy, |m| &m.buf);
                        let tb = blurred.as_ref().unwrap_or(&self.dummy);
                        self.run(
                            enc,
                            &self.pipes.spotlight,
                            18,
                            &p,
                            &[(0, &canvas), (2, mb), (4, tb)],
                            (w.width(), w.height()),
                        );
                    }
                    GpuOp::Cut(c) => {
                        let out_size = (c.dst_w, c.dst_h);
                        slot = 1 - slot.min(1);
                        let dst = self.canvas_buffer(slot, out_size)?;
                        let shadow = c.shadow.map_or(([0.0; 4], [0.0; 4]), |(off, n, col)| {
                            ([off[0], off[1], n as f32, 1.0], col)
                        });
                        let p = CutUniform {
                            src: [c.src_w, c.src_h, tiles_for(c.src_w), u32::from(c.vertical)],
                            dst: [c.dst_w, c.dst_h, tiles_for(c.dst_w), OCTAVES],
                            geo: [c.t0, c.removed, c.gap, 0],
                            seeds: [
                                c.seed_upper[0],
                                c.seed_upper[1],
                                c.seed_lower[0],
                                c.seed_lower[1],
                            ],
                            noise: [c.amplitude, c.roughness, c.inv_wavelength, 0.0],
                            shadow: shadow.0,
                            shadow_color: shadow.1,
                            gap_fill: c.gap_fill,
                        };
                        self.run(
                            enc,
                            &self.pipes.cutout,
                            19,
                            &p,
                            &[(0, &dst), (1, &canvas)],
                            out_size,
                        );
                        canvas = dst;
                    }
                    GpuOp::Frame {
                        op: f,
                        backdrop,
                        shadow,
                        clip,
                        border,
                    } => {
                        slot = 1 - slot.min(1);
                        let dst = self.canvas_buffer(slot, f.dst)?;
                        let flags =
                            u32::from(shadow.is_some()) | (u32::from(border.is_some()) << 1);
                        let p = FrameUniform {
                            src: [f.src.0, f.src.1, tiles_for(f.src.0), f.margin as u32],
                            dst: [f.dst.0, f.dst.1, tiles_for(f.dst.0), flags],
                            shadow_rect: shadow.as_ref().map_or([0; 4], |(m, _)| xywh(m.rect)),
                            clip_rect: xywh(clip.rect),
                            border_rect: border.as_ref().map_or([0; 4], |(m, _)| xywh(m.rect)),
                            shadow_color: shadow.as_ref().map_or([0.0; 4], |(_, c)| *c),
                            border_color: border.as_ref().map_or([0.0; 4], |(_, c)| *c),
                            color: backdrop.color,
                            paint: backdrop.paint,
                            geo: backdrop.geo,
                        };
                        let sb = shadow.as_ref().map_or(&self.dummy, |(m, _)| &m.buf);
                        let bb = border.as_ref().map_or(&self.dummy, |(m, _)| &m.buf);
                        let stops = backdrop.stops.as_ref().unwrap_or(&self.dummy);
                        self.run(
                            enc,
                            &self.pipes.frame,
                            23,
                            &p,
                            &[
                                (0, &dst),
                                (1, &canvas),
                                (2, sb),
                                (3, &clip.buf),
                                (22, bb),
                                (9, stops),
                            ],
                            f.dst,
                        );
                        canvas = dst;
                    }
                }
                size = op.out_size(size);
            }
        }
        Ok((canvas, size))
    }

    /// Renders and reads the frame back (exports, tests).
    pub fn render(&mut self, scene: &RenderScene) -> Result<Frame, GpuError> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("capturecraft frame"),
            });
        let encoded = self.encode(scene, &mut enc);
        let readback = encoded.as_ref().ok().map(|(canvas, size)| {
            let bytes = canvas_bytes(*size);
            let staging = self.buffer(
                bytes,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            );
            enc.copy_buffer_to_buffer(canvas, 0, &staging, 0, bytes);
            (staging, *size)
        });
        self.queue.submit([enc.finish()]);
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(GpuError::Validation(e.to_string()));
        }
        let (_, size) = encoded?;
        let Some((staging, _)) = readback else {
            return Err(GpuError::Readback("nothing encoded".into()));
        };
        let (tx, rx) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| GpuError::Readback(e.to_string()))?;
        rx.recv()
            .map_err(|e| GpuError::Readback(e.to_string()))?
            .map_err(|e| GpuError::Readback(e.to_string()))?;
        let data: Vec<[f32; 4]> = bytemuck::pod_collect_to_vec(
            &staging
                .slice(..)
                .get_mapped_range()
                .map_err(|e| GpuError::Readback(e.to_string()))?,
        );
        staging.unmap();
        Ok(TiledCanvas::from_tile_data(size.0, size.1, data)?.to_frame())
    }

    /// Renders into `target` (an `Rgba8Unorm` texture with
    /// `STORAGE_BINDING`, at least the frame size) as premultiplied RGBA,
    /// for display. Returns the frame size.
    ///
    /// The bytes are the compositor's own values, premultiplied and
    /// sRGB-encoded, quantized without any transfer function. That is what
    /// egui-wgpu 0.36 expects of a native texture: its `egui.wgsl`
    /// (`fs_main_linear_framebuffer` / `fs_main_gamma_framebuffer`)
    /// multiplies the sampled texel, as premultiplied gamma-space colour,
    /// with the gamma-space vertex colour. The target must therefore stay
    /// `Rgba8Unorm`: an `Rgba8UnormSrgb` view would decode the bytes to
    /// linear on sampling and the canvas would show too dark (double
    /// gamma). `tests/color_encoding.rs` pins the bytes.
    pub fn render_to_texture(
        &mut self,
        scene: &RenderScene,
        target: &wgpu::TextureView,
    ) -> Result<(u32, u32), GpuError> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("capturecraft view"),
            });
        let encoded = self.encode(scene, &mut enc);
        if let Ok((canvas, size)) = &encoded {
            let u = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::bytes_of(&PresentParams {
                        size: [size.0, size.1, tiles_for(size.0), 0],
                    }),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.pipes.present.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: canvas.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 20,
                        resource: u.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 21,
                        resource: wgpu::BindingResource::TextureView(target),
                    },
                ],
            });
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipes.present);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(size.0.div_ceil(WG), size.1.div_ceil(WG), 1);
        }
        self.queue.submit([enc.finish()]);
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(GpuError::Validation(e.to_string()));
        }
        encoded.map(|(_, s)| s)
    }
}
