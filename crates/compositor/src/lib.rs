//! # capturecraft-compositor (L4)
//!
//! Renders a [`RenderScene`] (captured pixels plus annotations) with two
//! interchangeable backends, after PhotoCraft's dual compositor design:
//!
//! * [`CpuCompositor`]: the reference oracle. Deterministic, tile-parallel
//!   (rayon over 256×256 tiles), pure Rust; used for headless exports, the
//!   CLI, CI and as ground truth in tests.
//! * `GpuCompositor` (feature `gpu`): wgpu compute pipelines over the same
//!   tile-major canvas layout (Metal, Vulkan, DX12, WebGPU, GL), for the
//!   interactive editor canvas.
//!
//! Both start from the same prepared ops ([`prepare`]): vector work
//! (callout outline unions, strokes, arrow heads, glyph outlines shaped by
//! photocraft-text, node transforms) is done once and turned into
//! exact-area coverage masks, so anti-aliasing is identical on both; so are
//! the feathered edges (signed distance from photocraft-algo's distance
//! transform), the shadow frame's blurred shadow (photocraft-algo's
//! Gaussian) and resampled rasters (photocraft-algo's warp). The per-pixel
//! work (blending, solid and gradient paints, Gaussian blurs, mosaics,
//! magnifier resampling, spotlight dimming, torn-edge Cut Outs, the shadow
//! frame) is written twice, in [`effects`] and in `shaders/effects.wgsl`,
//! with the same arithmetic; the [`parity`] contract bounds their
//! difference (≤ 2/255 per channel, SSIM ≥ 0.995).
//!
//! The crate sits on layer 4: it uses only photocraft crates from L0–L3 and
//! knows nothing of the engine, the file format or egui. L5/L7 translate
//! their document into a [`RenderScene`].
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod canvas;
pub mod cpu_compositor;
pub mod effects;
#[cfg(feature = "gpu")]
pub mod gpu_compositor;
pub mod parity;
pub mod prepare;
pub mod primitives;
pub mod scene;
pub mod text;

pub use canvas::{Frame, TileSet, TiledCanvas};
pub use cpu_compositor::{CpuCompositor, RenderStats};
#[cfg(feature = "gpu")]
pub use gpu_compositor::{GpuCompositor, GpuError};
pub use scene::*;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompositorError {
    #[error("invalid canvas size {width}x{height}")]
    InvalidSize { width: u32, height: u32 },
    #[error("not enough memory for a {bytes}-byte canvas")]
    OutOfMemory { bytes: usize },
}
