//! CPU vs GPU pixel parity.
//!
//! Every scene is rendered by the CPU oracle and by the wgpu compositor on
//! a headless adapter (hardware if present, else the software fallback:
//! lavapipe/llvmpipe on Linux CI, WARP on Windows) and must match within
//! 2/255 per channel with SSIM >= 0.995. Without any adapter the GPU half is
//! skipped with a note; the CPU checks still run.

use std::sync::{Arc, Mutex};

use capturecraft_compositor::parity::{self, Comparison};
use capturecraft_compositor::scene::*;
use capturecraft_compositor::{CpuCompositor, Frame, GpuCompositor};
use photocraft_color::PixelFormat;
use photocraft_geom::Rect;
use photocraft_raster::Surface;

/// One device at a time (software adapters are slow to share).
static GPU: Mutex<Option<Option<GpuCompositor>>> = Mutex::new(None);

/// Runs `f` on the shared GPU compositor. Tests reuse node ids for
/// different scenes, so each test starts from an empty cache (`fresh`).
fn with_gpu(fresh: bool, f: impl FnOnce(&mut GpuCompositor)) {
    let mut guard = GPU
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let slot = guard.get_or_insert_with(|| match GpuCompositor::headless() {
        Ok(g) => {
            if let Some(i) = g.adapter_info() {
                eprintln!(
                    "GPU parity adapter: {} ({:?}, {:?})",
                    i.name, i.backend, i.device_type
                );
            }
            Some(g)
        }
        Err(e) => {
            eprintln!("skipping GPU parity: {e}");
            None
        }
    });
    if let Some(g) = slot.as_mut() {
        if fresh {
            g.clear_cache();
        }
        f(g);
    }
}

const W: u32 = 640;
const H: u32 = 420;

/// A capture-like base image across 3×2 tiles: gradients, a checkerboard,
/// thin lines and a translucent corner (so alpha paths are exercised).
fn base_surface(seed: u32) -> Surface {
    let mut s = Surface::new(PixelFormat::RGBA8);
    let mut data = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            let checker = ((x / 16) + (y / 16)).is_multiple_of(2);
            let line = (x + seed).is_multiple_of(37) || y.is_multiple_of(29);
            let (mut r, mut g, mut b) = (
                x as f32 / W as f32,
                y as f32 / H as f32,
                if checker { 0.85 } else { 0.25 },
            );
            if line {
                (r, g, b) = (0.05, 0.05, 0.1);
            }
            // Quantize like a real 8-bit capture.
            let q = |v: f32| (v * 255.0).round() / 255.0;
            let a = if x > 560 && y > 350 { 0.5 } else { 1.0 };
            data.extend([q(r), q(g), q(b), a]);
        }
    }
    s.write_region(Rect::new(0, 0, W as i32, H as i32), &data);
    s
}

fn raster(id: u64, rev: u64, surface: Arc<Surface>) -> SceneNode {
    SceneNode::new(
        id,
        rev,
        RenderNode::Raster(RasterSpec {
            surface,
            origin: [0, 0],
            opacity: 1.0,
        }),
    )
}

fn callout() -> CalloutSpec {
    CalloutSpec {
        body: CalloutBody::RoundedRect {
            rect: RectF32::new(250.5, 40.25, 170.0, 70.0),
            radius: 14.0,
        },
        tails: vec![
            CalloutTail {
                tail_tip: PointF32::new(215.0, 175.0),
                base_width: 26.0,
                curve_bias: 0.45,
                anchor: TailAnchor::NearestEdge,
            },
            CalloutTail {
                tail_tip: PointF32::new(455.0, 150.0),
                base_width: 18.0,
                curve_bias: -0.3,
                anchor: TailAnchor::Center,
            },
        ],
        fill: Some(Color::new(1.0, 0.93, 0.45, 0.85).into()),
        stroke: Some(StrokeSpec {
            color: Color::new(0.75, 0.2, 0.1, 1.0),
            width: 3.0,
            join: LineJoin::Round,
            dashes: vec![],
        }),
        shadow: Some(DropShadowStyle {
            color: Color::new(0.0, 0.0, 0.0, 0.4),
            offset: PointF32::new(4.0, 5.0),
            blur_sigma: 5.0,
            spread: 1.0,
        }),
        text: None,
    }
}

fn magnifier() -> MagnifierSpec {
    MagnifierSpec {
        source_center: PointF32::new(150.3, 300.7),
        lens_bounds: ShapeBounds::Circle {
            center: PointF32::new(520.0, 250.0),
            radius: 70.0,
        },
        zoom_factor: 3.0,
        resample: Resample::CatmullRom,
        pixel_grid_mode: false,
        grid_color: Color::new(0.0, 0.0, 0.0, 0.25),
        border_stroke: Some(StrokeSpec::solid(Color::WHITE, 4.0)),
        inner_shadow: Some(InnerShadowStyle::default()),
        drop_shadow: Some(DropShadowStyle::default()),
        connector: Some(StrokeSpec {
            color: Color::new(1.0, 1.0, 1.0, 0.9),
            width: 2.0,
            join: LineJoin::Round,
            dashes: vec![6.0, 4.0],
        }),
    }
}

/// The scene the contract is stated for: base tiles, a blur region, a 3×
/// magnifier, a curved callout and a torn-edge Cut Out (plus an arrow drawn
/// after the cut, in post-cut coordinates).
fn composite_scene(surface: Arc<Surface>) -> RenderScene {
    RenderScene {
        width: W,
        height: H,
        background: Color::WHITE,
        nodes: vec![
            raster(1, 1, surface),
            SceneNode::new(
                2,
                1,
                RenderNode::BackdropFilter(BackdropFilterSpec {
                    shape: ShapeGeometry::RoundedRect {
                        rect: RectF32::new(30.0, 30.0, 180.0, 90.0),
                        radius: 10.0,
                    },
                    filter: BackdropFilterKind::GaussianBlur { sigma: 4.5 },
                    feather_px: 0.0,
                }),
            ),
            SceneNode::new(3, 1, RenderNode::Callout(callout())),
            SceneNode::new(4, 1, RenderNode::Magnifier(magnifier())),
            SceneNode::new(
                5,
                1,
                RenderNode::CutOut(CutOutSeamSpec {
                    axis: CutAxis::Horizontal,
                    t0: 190,
                    t1: 230,
                    gap_px: 10,
                    gap_fill: Color::TRANSPARENT,
                    seam: SeamStyle::TornEdge {
                        amplitude_px: 7.0,
                        roughness: 0.55,
                        wavelength_px: 28.0,
                        seed: 0x5eed_cafe,
                    },
                    shadow: Some(SeamShadow {
                        color: Color::new(0.0, 0.0, 0.0, 0.45),
                        offset: PointF32::new(2.0, 5.0),
                        samples: 8,
                    }),
                }),
            ),
            SceneNode::new(
                6,
                1,
                RenderNode::Arrow(ArrowSpec {
                    start: PointF32::new(40.0, 360.0),
                    end: PointF32::new(300.0, 250.0),
                    curve: ArrowCurve::Quadratic {
                        control: PointF32::new(120.0, 220.0),
                    },
                    color: Color::new(0.9, 0.1, 0.2, 0.7),
                    width: 6.0,
                    head: Some(ArrowHeadStyle {
                        shape: ArrowHeadShape::Triangle,
                        length: 26.0,
                        width: 24.0,
                    }),
                    tail_head: None,
                    dashes: vec![],
                    shadow: Some(DropShadowStyle::default()),
                }),
            ),
        ],
    }
}

fn report(c: &Comparison) -> String {
    format!(
        "max diff {:.5} ({:.2}/255) at {:?}, {} px over tolerance, SSIM {:.6}",
        c.max_diff,
        c.max_diff * 255.0,
        c.at,
        c.over_tolerance,
        c.ssim
    )
}

fn cpu(scene: &RenderScene) -> Frame {
    CpuCompositor::render(scene).unwrap_or_else(|e| panic!("CPU render failed: {e}"))
}

fn assert_parity(scene: &RenderScene, what: &str, fresh: bool) {
    let reference = cpu(scene);
    with_gpu(fresh, |g| {
        let frame = g
            .render(scene)
            .unwrap_or_else(|e| panic!("{what}: GPU render failed: {e}"));
        let c = parity::compare(&reference, &frame).unwrap_or_else(|| {
            panic!(
                "{what}: size mismatch {}x{} vs {}x{}",
                reference.width, reference.height, frame.width, frame.height
            )
        });
        eprintln!("{what}: {}", report(&c));
        assert!(c.passes(), "{what}: parity broken: {}", report(&c));
    });
}

#[test]
fn composite_scene_matches_on_cpu_and_gpu() {
    let scene = composite_scene(Arc::new(base_surface(0)));
    let reference = cpu(&scene);
    // The cut removed 40 rows and added a 10 px gap.
    assert_eq!((reference.width, reference.height), (W, H - 30));
    // Sanity: the frame is not trivially uniform, and the gap is see-through.
    assert!(reference.pixels.iter().any(|p| p[3] < 0.5));
    assert!(reference.pixels.windows(2).filter(|w| w[0] != w[1]).count() > 10_000);
    assert_parity(&scene, "composite", true);
}

#[test]
fn lanczos_pixel_grid_pixelate_spotlight_and_vertical_cut_match() {
    let surface = Arc::new(base_surface(5));
    let mut m = magnifier();
    m.resample = Resample::Lanczos3;
    m.lens_bounds = ShapeBounds::RoundedRect {
        rect: RectF32::new(380.0, 40.0, 200.0, 130.0),
        radius: 18.0,
    };
    let mut grid = magnifier();
    grid.zoom_factor = 6.0;
    grid.pixel_grid_mode = true;
    grid.source_center = PointF32::new(600.0, 400.0);
    grid.lens_bounds = ShapeBounds::Circle {
        center: PointF32::new(120.0, 330.0),
        radius: 60.0,
    };
    let scene = RenderScene {
        width: W,
        height: H,
        background: Color::new(0.2, 0.2, 0.25, 1.0),
        nodes: vec![
            raster(1, 1, surface),
            SceneNode::new(
                2,
                1,
                RenderNode::BackdropFilter(BackdropFilterSpec {
                    shape: ShapeGeometry::Ellipse(RectF32::new(260.0, 230.0, 150.0, 110.0)),
                    filter: BackdropFilterKind::Pixelate { block_size_px: 12 },
                    feather_px: 0.0,
                }),
            ),
            SceneNode::new(3, 1, RenderNode::Magnifier(m)),
            SceneNode::new(4, 1, RenderNode::Magnifier(grid)),
            SceneNode::new(
                5,
                1,
                RenderNode::Shape(ShapeSpec {
                    geometry: ShapeGeometry::Polygon(vec![
                        PointF32::new(20.0, 20.0),
                        PointF32::new(200.0, 60.0),
                        PointF32::new(90.0, 160.0),
                    ]),
                    fill: Some(Color::new(0.1, 0.6, 0.3, 0.5).into()),
                    stroke: Some(StrokeSpec {
                        color: Color::BLACK,
                        width: 2.5,
                        join: LineJoin::Miter,
                        dashes: vec![8.0, 5.0],
                    }),
                    shadow: Some(DropShadowStyle {
                        spread: 3.0,
                        ..Default::default()
                    }),
                    text: None,
                }),
            ),
            SceneNode::new(
                6,
                1,
                RenderNode::Spotlight(SpotlightSpec {
                    targets: vec![
                        ShapeGeometry::Ellipse(RectF32::new(250.0, 220.0, 170.0, 130.0)),
                        ShapeGeometry::Rect(RectF32::new(370.0, 30.0, 220.0, 150.0)),
                    ],
                    dim_opacity: 0.55,
                    dim_color: Color::BLACK,
                    feather_px: 12.0,
                    blur_sigma: 2.0,
                }),
            ),
            SceneNode::new(
                7,
                1,
                RenderNode::CutOut(CutOutSeamSpec {
                    axis: CutAxis::Vertical,
                    t0: 220,
                    t1: 250,
                    gap_px: 0,
                    gap_fill: Color::TRANSPARENT,
                    seam: SeamStyle::TornEdge {
                        amplitude_px: 9.0,
                        roughness: 0.6,
                        wavelength_px: 20.0,
                        seed: 7,
                    },
                    shadow: Some(SeamShadow {
                        color: Color::new(0.0, 0.0, 0.0, 0.5),
                        offset: PointF32::new(4.0, 1.0),
                        samples: 6,
                    }),
                }),
            ),
        ],
    };
    assert_eq!(cpu(&scene).width, W - 30);
    assert_parity(&scene, "lanczos/grid/pixelate/spotlight/vertical cut", true);
}

#[test]
fn straight_cut_cloud_and_banner_callouts_match() {
    let mut cloud = callout();
    cloud.body = CalloutBody::Cloud {
        rect: RectF32::new(60.0, 60.0, 220.0, 130.0),
        puffs: 12,
    };
    let mut banner = callout();
    banner.body = CalloutBody::Banner {
        rect: RectF32::new(330.0, 250.0, 240.0, 60.0),
        notch: 16.0,
    };
    banner.tails = vec![CalloutTail {
        tail_tip: PointF32::new(600.0, 390.0),
        base_width: 20.0,
        curve_bias: 0.0,
        anchor: TailAnchor::NearestEdge,
    }];
    let scene = RenderScene {
        width: W,
        height: H,
        background: Color::TRANSPARENT,
        nodes: vec![
            raster(1, 1, Arc::new(base_surface(11))),
            SceneNode::new(2, 1, RenderNode::Callout(cloud)),
            SceneNode::new(3, 1, RenderNode::Callout(banner)),
            SceneNode::new(
                4,
                1,
                RenderNode::CutOut(CutOutSeamSpec {
                    axis: CutAxis::Horizontal,
                    t0: 200,
                    t1: 240,
                    gap_px: 0,
                    gap_fill: Color::TRANSPARENT,
                    seam: SeamStyle::Straight,
                    shadow: None,
                }),
            ),
        ],
    };
    assert_parity(&scene, "cloud/banner/straight cut", true);
}

#[test]
fn raster_edits_reupload_only_changed_tiles_and_stay_in_parity() {
    let surface = base_surface(0);
    let scene = composite_scene(Arc::new(surface.clone()));
    // Paint into one tile (copy-on-write: the other tiles stay shared).
    let mut edited = surface;
    edited.fill_rect(Rect::new(300, 300, 340, 330), &[1.0, 0.0, 1.0, 1.0]);
    let mut scene2 = composite_scene(Arc::new(edited));
    scene2.nodes[0].revision = RevisionId(2);
    let reference = cpu(&scene2);
    // Both renders under one lock: the second must take the partial
    // re-upload path on the cache the first one filled.
    with_gpu(true, |g| {
        let _ = g
            .render(&scene)
            .unwrap_or_else(|e| panic!("GPU render failed: {e}"));
        let frame = g
            .render(&scene2)
            .unwrap_or_else(|e| panic!("GPU render failed: {e}"));
        let c = parity::compare(&reference, &frame).unwrap_or_else(|| panic!("size mismatch"));
        eprintln!("after raster edit: {}", report(&c));
        assert!(c.passes(), "after raster edit: {}", report(&c));
    });
}

#[test]
fn incremental_cpu_updates_equal_full_renders() {
    let surface = Arc::new(base_surface(0));
    let mut c = CpuCompositor::new();
    let first = c
        .update(&composite_scene(surface.clone()), &DirtyRegion::none())
        .unwrap_or_default();
    assert!(first.full);

    // Move the callout.
    let mut scene = composite_scene(surface.clone());
    if let RenderNode::Callout(cl) = &mut scene.nodes[2].node {
        cl.body = CalloutBody::RoundedRect {
            rect: RectF32::new(40.0, 130.0, 150.0, 50.0),
            radius: 8.0,
        };
    }
    scene.nodes[2].revision = RevisionId(2);
    let stats = c.update(&scene, &DirtyRegion::none()).unwrap_or_default();
    assert!(!stats.full && stats.tiles > 0);
    assert_eq!(c.frame(), Some(cpu(&scene)), "callout move");

    // Paint under the magnifier's source: the lens must follow.
    let mut edited = (*surface).clone();
    edited.fill_rect(Rect::new(140, 290, 160, 310), &[0.0, 1.0, 0.0, 1.0]);
    let mut scene2 = scene.clone();
    scene2.nodes[0] = raster(1, 2, Arc::new(edited));
    let stats = c.update(&scene2, &DirtyRegion::none()).unwrap_or_default();
    // The source tile, and the lens (in post-cut coordinates) as well.
    assert!(!stats.full, "{stats:?}");
    assert!(
        stats.damage.iter().any(|r| r.contains(520, 250 - 30)),
        "lens not redrawn: {stats:?}"
    );
    assert_eq!(c.frame(), Some(cpu(&scene2)), "raster edit under the lens");

    // An explicit dirty rectangle with nothing changed redraws in place.
    let mut d = DirtyRegion::none();
    d.add(Rect::new(0, 0, 10, 10));
    let stats = c.update(&scene2, &d).unwrap_or_default();
    assert_eq!(stats.tiles, 1);
    assert_eq!(c.frame(), Some(cpu(&scene2)));
}

#[test]
fn cpu_oracle_is_deterministic_across_thread_counts() {
    let scene = composite_scene(Arc::new(base_surface(3)));
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .map(|p| p.install(|| cpu(&scene)));
    let many = rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .map(|p| p.install(|| cpu(&scene)));
    match (one, many) {
        (Ok(a), Ok(b)) => assert!(a == b, "frames differ between 1 and 8 threads"),
        _ => panic!("thread pool"),
    }
}
