//! Colour encoding of the editor texture.
//!
//! The compositors work on premultiplied, sRGB-encoded values and never
//! linearize; egui-wgpu 0.36 samples an `Rgba8Unorm` native texture as
//! exactly that (premultiplied gamma-space colour). This test renders
//! known swatches on the CPU oracle and through
//! [`GpuCompositor::render_to_texture`] into an `Rgba8Unorm` texture, reads
//! the texture back and checks every channel against the byte expected
//! from the hex value (within 1/255). The expected bytes go through
//! photocraft-cms's sRGB transfer curve (decode to linear light and encode
//! again), and the test also checks that a linearizing pipeline would have
//! failed it, so a double-gamma regression cannot pass.

#![allow(clippy::panic)]

use capturecraft_compositor::scene::*;
use capturecraft_compositor::{CpuCompositor, GpuCompositor};
use photocraft_cms::curve::srgb_trc;

const W: u32 = 64;
const H: u32 = 8;
const SWATCHES: [u32; 3] = [0xFF5733, 0x1E293B, 0xF8FAFC];

fn channels(hex: u32) -> [u8; 3] {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
}

fn scene(hex: u32, alpha: f32) -> RenderScene {
    let [r, g, b] = channels(hex);
    let mut c = Color::rgba8(r, g, b, 255);
    c.a = alpha;
    RenderScene {
        width: W,
        height: H,
        background: Color::TRANSPARENT,
        nodes: vec![SceneNode::new(
            1,
            1,
            RenderNode::Shape(ShapeSpec {
                geometry: ShapeGeometry::Rect(RectF32::new(0.0, 0.0, W as f32, H as f32)),
                fill: Some(c.into()),
                stroke: None,
                shadow: None,
                text: None,
            }),
        )],
    }
}

/// The premultiplied sRGB-encoded bytes of `hex` at `alpha`: each channel
/// decoded to linear light and encoded again with the sRGB curve (an
/// identity up to rounding, which is the point: the stored value is the
/// encoded one), then premultiplied.
fn expected(hex: u32, alpha: f32) -> [f32; 4] {
    let trc = srgb_trc();
    let [r, g, b] = channels(hex).map(|v| {
        let linear = trc.eval64(f64::from(v) / 255.0);
        trc.eval_inverse64(linear) as f32 * alpha * 255.0
    });
    [r, g, b, alpha * 255.0]
}

/// What a pipeline that linearized before writing would store.
fn linearized(hex: u32) -> [f32; 3] {
    let trc = srgb_trc();
    channels(hex).map(|v| trc.eval64(f64::from(v) / 255.0) as f32 * 255.0)
}

fn check(what: &str, got: [u8; 4], want: [f32; 4]) {
    for c in 0..4 {
        let d = (f32::from(got[c]) - want[c]).abs();
        assert!(
            d <= 1.0,
            "{what}: channel {c} is {} want {:.2}",
            got[c],
            want[c]
        );
    }
}

fn gpu_bytes(g: &mut GpuCompositor, scene: &RenderScene) -> Vec<u8> {
    let device = g.device().clone();
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("swatches"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // The format the editor registers with egui-wgpu.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    g.render_to_texture(scene, &view)
        .unwrap_or_else(|e| panic!("GPU render failed: {e}"));
    // 64 px × 4 bytes is one 256-byte aligned row.
    let row = W * 4;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * H),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(H),
            },
        },
        tex.size(),
    );
    g.queue().submit([enc.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap_or_else(|e| panic!("poll failed: {e}"));
    rx.recv()
        .unwrap_or_else(|e| panic!("map failed: {e}"))
        .unwrap_or_else(|e| panic!("map failed: {e}"));
    let bytes = buf
        .slice(..)
        .get_mapped_range()
        .unwrap_or_else(|e| panic!("map failed: {e}"))
        .to_vec();
    buf.unmap();
    bytes
}

#[test]
fn swatches_reach_the_egui_texture_without_double_gamma() {
    // The test can tell the encodings apart: for every swatch, linear light
    // misses the encoded value by well over the 1/255 tolerance.
    for hex in SWATCHES {
        let enc = expected(hex, 1.0);
        let lin = linearized(hex);
        assert!(
            (0..3).any(|c| (enc[c] - lin[c]).abs() > 2.0),
            "{hex:06X} cannot detect a linearized pipeline"
        );
    }

    let mut gpu = match GpuCompositor::headless() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipping the GPU half: {e}");
            None
        }
    };
    for hex in SWATCHES {
        for alpha in [1.0, 0.5] {
            let what = format!("#{hex:06X} at {alpha}");
            let s = scene(hex, alpha);
            let want = expected(hex, alpha);
            let frame = CpuCompositor::render(&s).unwrap_or_else(|e| panic!("{what}: {e}"));
            // The CPU frame quantized the way the texture write does.
            let p = frame.pixel(W / 2, H / 2);
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            check(
                &format!("{what} (CPU)"),
                [q(p[0]), q(p[1]), q(p[2]), q(p[3])],
                want,
            );
            if let Some(g) = gpu.as_mut() {
                g.clear_cache();
                let bytes = gpu_bytes(g, &s);
                for (i, px) in bytes.as_chunks::<4>().0.iter().enumerate() {
                    check(&format!("{what} (GPU texture, pixel {i})"), *px, want);
                }
            }
        }
    }
}
