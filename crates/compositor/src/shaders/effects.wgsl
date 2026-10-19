// CaptureCraft compositor: compute passes of the wgpu backend.
//
// Every function mirrors its CPU reference in src/effects (same f32
// operations, same summation order); change both together.
//
// The canvas is a storage buffer of premultiplied RGBA in tile-major order
// (256x256 tiles back to back, each row-major), the layout of
// TiledCanvas on the CPU. Masks and temporaries are row-major over their
// own rectangles. Every resource has its own binding number; each entry
// point uses a subset, and its pipeline layout is derived from that use.

const TILE: u32 = 256u;
const TILE_PX: u32 = 65536u;

@group(0) @binding(0) var<storage, read_write> canvas: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read> canvas_ro: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> mask: array<f32>;
@group(0) @binding(3) var<storage, read> clip: array<f32>;
@group(0) @binding(4) var<storage, read> tmp_in: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read_write> tmp_out: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read> weights: array<f32>;
@group(0) @binding(7) var<storage, read_write> mask_out: array<f32>;
@group(0) @binding(8) var<storage, read> image: array<vec4<f32>>;
// Gradient stops: (offset, _, _, _), (r, g, b, a) straight, per stop.
@group(0) @binding(9) var<storage, read> stops: array<vec4<f32>>;
@group(0) @binding(22) var<storage, read> mask2: array<f32>;

// size = (width, height, tiles_x, _); rectangles are (x0, y0, x1, y1)
// unless named *_rect with (x0, y0, w, h).

struct ClearParams { size: vec4<u32>, color: vec4<f32> }
struct ImageParams { size: vec4<u32>, rect: vec4<i32>, src: vec4<i32>, opacity: vec4<f32> }
// paint = (kind: 0 solid, 1 linear, 2 radial; stop count; _, _)
struct FillParams { size: vec4<u32>, rect: vec4<i32>, mask_rect: vec4<i32>, clip_rect: vec4<i32>, flags: vec4<u32>, color: vec4<f32>, paint: vec4<u32>, geo: vec4<f32> }
struct MaskBlurParams { dims: vec4<i32> }
struct BlurParams { size: vec4<u32>, rect: vec4<i32>, src_rect: vec4<i32>, radius: vec4<i32> }
struct MixParams { size: vec4<u32>, rect: vec4<i32>, mask_rect: vec4<i32> }
struct MosaicParams { size: vec4<u32>, rect: vec4<i32>, mask_rect: vec4<i32>, grid: vec4<i32>, blocks: vec4<i32> }
struct LensParams { size: vec4<u32>, rect: vec4<i32>, mask_rect: vec4<i32>, mode: vec4<u32>, geo: vec4<f32>, zoom: vec4<f32>, grid: vec4<f32> }
struct SpotParams { size: vec4<u32>, rect: vec4<i32>, mask_rect: vec4<i32>, flags: vec4<u32>, dim: vec4<f32> }
struct CutParams {
    src: vec4<u32>,      // w, h, tiles_x, vertical
    dst: vec4<u32>,      // w, h, tiles_x, octaves
    geo: vec4<i32>,      // t0, removed, gap, _
    seeds: vec4<u32>,    // upper (lo, hi), lower (lo, hi)
    noise: vec4<f32>,    // amplitude, roughness, 1/wavelength, _
    shadow: vec4<f32>,   // along, across, samples, enabled
    shadow_color: vec4<f32>,
    gap_fill: vec4<f32>,
}
struct PresentParams { size: vec4<u32> }
struct FrameParams {
    src: vec4<u32>,      // w, h, tiles_x, margin
    dst: vec4<u32>,      // w, h, tiles_x, flags (1 shadow, 2 border)
    shadow_rect: vec4<i32>,
    clip_rect: vec4<i32>,
    border_rect: vec4<i32>,
    shadow_color: vec4<f32>,
    border_color: vec4<f32>,
    color: vec4<f32>,    // solid backdrop
    paint: vec4<u32>,
    geo: vec4<f32>,
}

@group(0) @binding(10) var<uniform> clear_p: ClearParams;
@group(0) @binding(11) var<uniform> image_p: ImageParams;
@group(0) @binding(12) var<uniform> fill_p: FillParams;
@group(0) @binding(13) var<uniform> mblur_p: MaskBlurParams;
@group(0) @binding(14) var<uniform> blur_p: BlurParams;
@group(0) @binding(15) var<uniform> mix_p: MixParams;
@group(0) @binding(16) var<uniform> mosaic_p: MosaicParams;
@group(0) @binding(17) var<uniform> lens_p: LensParams;
@group(0) @binding(18) var<uniform> spot_p: SpotParams;
@group(0) @binding(19) var<uniform> cut_p: CutParams;
@group(0) @binding(20) var<uniform> present_p: PresentParams;
@group(0) @binding(21) var present_out: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(23) var<uniform> frame_p: FrameParams;

fn cidx(x: i32, y: i32, tiles_x: u32) -> u32 {
    let ux = u32(x);
    let uy = u32(y);
    return ((uy / TILE) * tiles_x + ux / TILE) * TILE_PX + (uy % TILE) * TILE + (ux % TILE);
}

fn over(src: vec4<f32>, dst: vec4<f32>) -> vec4<f32> {
    return src + dst * (1.0 - src.a);
}

fn mask_at(x: i32, y: i32, r: vec4<i32>) -> f32 {
    let lx = x - r.x;
    let ly = y - r.y;
    if (lx < 0 || ly < 0 || lx >= r.z || ly >= r.w) {
        return 0.0;
    }
    return mask[u32(ly * r.z + lx)];
}

fn mask2_at(x: i32, y: i32, r: vec4<i32>) -> f32 {
    let lx = x - r.x;
    let ly = y - r.y;
    if (lx < 0 || ly < 0 || lx >= r.z || ly >= r.w) {
        return 0.0;
    }
    return mask2[u32(ly * r.z + lx)];
}

fn clip_at(x: i32, y: i32, r: vec4<i32>) -> f32 {
    let lx = x - r.x;
    let ly = y - r.y;
    if (lx < 0 || ly < 0 || lx >= r.z || ly >= r.w) {
        return 0.0;
    }
    return clip[u32(ly * r.z + lx)];
}

// Premultiplied paint at pixel (x, y): the solid colour, or the gradient
// sampled at the pixel centre (CPU: PaintPx::at).
fn paint_at(x: i32, y: i32, paint: vec4<u32>, geo: vec4<f32>, solid: vec4<f32>) -> vec4<f32> {
    if (paint.x == 0u) {
        return solid;
    }
    let px = f32(x) + 0.5;
    let py = f32(y) + 0.5;
    var t: f32;
    if (paint.x == 2u) {
        let dx = px - geo.x;
        let dy = py - geo.y;
        t = sqrt(dx * dx + dy * dy) * geo.z;
    } else {
        t = (px - geo.x) * geo.z + (py - geo.y) * geo.w;
    }
    t = clamp(t, 0.0, 1.0);
    let n = paint.y;
    var c = stops[1];
    if (t > stops[0].x) {
        c = stops[2u * (n - 1u) + 1u];
        for (var i = 1u; i < n; i++) {
            let o0 = stops[2u * (i - 1u)].x;
            let o1 = stops[2u * i].x;
            if (t <= o1) {
                let span = o1 - o0;
                var f = 1.0;
                if (span > 0.0) {
                    f = (t - o0) / span;
                }
                c = stops[2u * (i - 1u) + 1u] * (1.0 - f) + stops[2u * i + 1u] * f;
                break;
            }
        }
    }
    return vec4<f32>(c.rgb * c.a, c.a);
}

// ---------------------------------------------------------------- basics

@compute @workgroup_size(16, 16)
fn clear(@builtin(global_invocation_id) g: vec3<u32>) {
    let s = clear_p.size;
    if (g.x >= s.x || g.y >= s.y) {
        return;
    }
    canvas[cidx(i32(g.x), i32(g.y), s.z)] = clear_p.color;
}

// Raster pixels (premultiplied, tile-major in the surface's own grid).
@compute @workgroup_size(16, 16)
fn image_over(@builtin(global_invocation_id) g: vec3<u32>) {
    let r = image_p.rect;
    let x = r.x + i32(g.x);
    let y = r.y + i32(g.y);
    if (x >= r.z || y >= r.w) {
        return;
    }
    let lx = x - image_p.src.x;
    let ly = y - image_p.src.y;
    if (lx < 0 || ly < 0 || lx >= image_p.src.z * 256 || ly >= image_p.src.w * 256) {
        return;
    }
    let si = ((u32(ly) / TILE) * u32(image_p.src.z) + u32(lx) / TILE) * TILE_PX + (u32(ly) % TILE) * TILE + (u32(lx) % TILE);
    let src = image[si] * image_p.opacity.x;
    let i = cidx(x, y, image_p.size.z);
    canvas[i] = over(src, canvas[i]);
}

// Colour through a coverage mask: blur(mask) [inverted] [* clip].
@compute @workgroup_size(16, 16)
fn fill(@builtin(global_invocation_id) g: vec3<u32>) {
    let r = fill_p.rect;
    let x = r.x + i32(g.x);
    let y = r.y + i32(g.y);
    if (x >= r.z || y >= r.w) {
        return;
    }
    var m = mask_at(x, y, fill_p.mask_rect);
    if (fill_p.flags.y != 0u) {
        m = 1.0 - m;
    }
    if (fill_p.flags.x != 0u) {
        m = m * clip_at(x, y, fill_p.clip_rect);
    }
    if (m <= 0.0) {
        return;
    }
    let i = cidx(x, y, fill_p.size.z);
    canvas[i] = over(paint_at(x, y, fill_p.paint, fill_p.geo, fill_p.color) * m, canvas[i]);
}

// ------------------------------------------------------- mask blur (zero pad)

@compute @workgroup_size(16, 16)
fn blur_mask_h(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = mblur_p.dims.x;
    let h = mblur_p.dims.y;
    let rad = mblur_p.dims.z;
    let x = i32(g.x);
    let y = i32(g.y);
    if (x >= w || y >= h) {
        return;
    }
    var acc = 0.0;
    for (var k = 0; k <= 2 * rad; k++) {
        let sx = x + k - rad;
        if (sx >= 0 && sx < w) {
            acc += weights[k] * mask[u32(y * w + sx)];
        }
    }
    mask_out[u32(y * w + x)] = acc;
}

@compute @workgroup_size(16, 16)
fn blur_mask_v(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = mblur_p.dims.x;
    let h = mblur_p.dims.y;
    let rad = mblur_p.dims.z;
    let x = i32(g.x);
    let y = i32(g.y);
    if (x >= w || y >= h) {
        return;
    }
    var acc = 0.0;
    for (var k = 0; k <= 2 * rad; k++) {
        let sy = y + k - rad;
        if (sy >= 0 && sy < h) {
            acc += weights[k] * mask[u32(sy * w + x)];
        }
    }
    mask_out[u32(y * w + x)] = acc;
}

// ---------------------------------------------- backdrop blur (edge clamp)

// Canvas -> tmp_out over rect (rows already extended by the radius).
@compute @workgroup_size(16, 16)
fn blur_rgba_h(@builtin(global_invocation_id) g: vec3<u32>) {
    let r = blur_p.rect;
    let x = r.x + i32(g.x);
    let y = r.y + i32(g.y);
    if (x >= r.z || y >= r.w) {
        return;
    }
    let rad = blur_p.radius.x;
    let wmax = i32(blur_p.size.x) - 1;
    var acc = vec4<f32>(0.0);
    for (var k = 0; k <= 2 * rad; k++) {
        let sx = clamp(x + k - rad, 0, wmax);
        acc += weights[k] * canvas_ro[cidx(sx, y, blur_p.size.z)];
    }
    tmp_out[u32((y - r.y) * (r.z - r.x) + (x - r.x))] = acc;
}

// tmp_in (over src_rect) -> tmp_out over rect.
@compute @workgroup_size(16, 16)
fn blur_rgba_v(@builtin(global_invocation_id) g: vec3<u32>) {
    let r = blur_p.rect;
    let s = blur_p.src_rect;
    let x = r.x + i32(g.x);
    let y = r.y + i32(g.y);
    if (x >= r.z || y >= r.w) {
        return;
    }
    let rad = blur_p.radius.x;
    let hmax = i32(blur_p.size.y) - 1;
    var acc = vec4<f32>(0.0);
    for (var k = 0; k <= 2 * rad; k++) {
        let sy = clamp(y + k - rad, 0, hmax);
        acc += weights[k] * tmp_in[u32((sy - s.y) * (s.z - s.x) + (x - s.x))];
    }
    tmp_out[u32((y - r.y) * (r.z - r.x) + (x - r.x))] = acc;
}

// canvas = mix(canvas, tmp_in, mask) over rect (tmp_in row-major over rect).
@compute @workgroup_size(16, 16)
fn mix_masked(@builtin(global_invocation_id) g: vec3<u32>) {
    let r = mix_p.rect;
    let x = r.x + i32(g.x);
    let y = r.y + i32(g.y);
    if (x >= r.z || y >= r.w) {
        return;
    }
    let m = mask_at(x, y, mix_p.mask_rect);
    if (m <= 0.0) {
        return;
    }
    let i = cidx(x, y, mix_p.size.z);
    canvas[i] = mix(canvas[i], tmp_in[u32((y - r.y) * (r.z - r.x) + (x - r.x))], m);
}

// ---------------------------------------------------------------- mosaic

// One invocation per block: mean of the block (inside the canvas).
@compute @workgroup_size(16, 16)
fn mosaic_reduce(@builtin(global_invocation_id) g: vec3<u32>) {
    let nb = mosaic_p.blocks;
    if (i32(g.x) >= nb.z || i32(g.y) >= nb.w) {
        return;
    }
    let b = mosaic_p.grid.z;
    let bx = nb.x + i32(g.x);
    let by = nb.y + i32(g.y);
    let x0 = max(mosaic_p.grid.x + bx * b, 0);
    let y0 = max(mosaic_p.grid.y + by * b, 0);
    let x1 = min(mosaic_p.grid.x + (bx + 1) * b, i32(mosaic_p.size.x));
    let y1 = min(mosaic_p.grid.y + (by + 1) * b, i32(mosaic_p.size.y));
    var acc = vec4<f32>(0.0);
    for (var y = y0; y < y1; y++) {
        for (var x = x0; x < x1; x++) {
            acc += canvas_ro[cidx(x, y, mosaic_p.size.z)];
        }
    }
    let n = f32(max(x1 - x0, 0) * max(y1 - y0, 0));
    var mean = vec4<f32>(0.0);
    if (n > 0.0) {
        mean = acc / n;
    }
    tmp_out[u32(i32(g.y) * nb.z + i32(g.x))] = mean;
}

@compute @workgroup_size(16, 16)
fn mosaic_apply(@builtin(global_invocation_id) g: vec3<u32>) {
    let r = mosaic_p.rect;
    let x = r.x + i32(g.x);
    let y = r.y + i32(g.y);
    if (x >= r.z || y >= r.w) {
        return;
    }
    let m = mask_at(x, y, mosaic_p.mask_rect);
    if (m <= 0.0) {
        return;
    }
    let b = mosaic_p.grid.z;
    let nb = mosaic_p.blocks;
    // x, y lie inside the mask, at or after the grid origin.
    let bi = ((y - mosaic_p.grid.y) / b - nb.y) * nb.z + ((x - mosaic_p.grid.x) / b - nb.x);
    let i = cidx(x, y, mosaic_p.size.z);
    canvas[i] = mix(canvas[i], tmp_in[u32(bi)], m);
}

// ------------------------------------------------------------- magnifier

fn lens_fetch(x: i32, y: i32) -> vec4<f32> {
    let cx = clamp(x, 0, i32(lens_p.size.x) - 1);
    let cy = clamp(y, 0, i32(lens_p.size.y) - 1);
    return canvas_ro[cidx(cx, cy, lens_p.size.z)];
}

fn catmull_rom_weights(t: f32) -> vec4<f32> {
    return vec4<f32>(
        ((-0.5 * t + 1.0) * t - 0.5) * t,
        (1.5 * t - 2.5) * t * t + 1.0,
        ((-1.5 * t + 2.0) * t + 0.5) * t,
        (0.5 * t - 0.5) * t * t,
    );
}

const PI: f32 = 3.14159265358979323846;

fn sin_pi(x: f32) -> f32 {
    let n = round(x);
    let s = sin(PI * (x - n));
    if ((i32(n) & 1) == 0) {
        return s;
    }
    return -s;
}

fn lanczos3(x: f32) -> f32 {
    if (abs(x) < 1e-5) {
        return 1.0;
    }
    if (abs(x) >= 3.0) {
        return 0.0;
    }
    let px = PI * x;
    return 3.0 * sin_pi(x) * sin_pi(x / 3.0) / (px * px);
}

fn clamp_premultiplied(p: vec4<f32>) -> vec4<f32> {
    let a = clamp(p.a, 0.0, 1.0);
    return vec4<f32>(min(clamp(p.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(a)), a);
}

fn sample_catmull_rom(u: vec2<f32>) -> vec4<f32> {
    let sx = u.x - 0.5;
    let sy = u.y - 0.5;
    let fx = floor(sx);
    let fy = floor(sy);
    let ix = i32(fx);
    let iy = i32(fy);
    let wx = catmull_rom_weights(sx - fx);
    let wy = catmull_rom_weights(sy - fy);
    var acc = vec4<f32>(0.0);
    for (var j = 0; j < 4; j++) {
        var row = vec4<f32>(0.0);
        for (var i = 0; i < 4; i++) {
            row += wx[i] * lens_fetch(ix - 1 + i, iy - 1 + j);
        }
        acc += wy[j] * row;
    }
    return clamp_premultiplied(acc);
}

fn sample_lanczos3(u: vec2<f32>) -> vec4<f32> {
    let sx = u.x - 0.5;
    let sy = u.y - 0.5;
    let fx = floor(sx);
    let fy = floor(sy);
    let ix = i32(fx);
    let iy = i32(fy);
    var wx: array<f32, 6>;
    var wy: array<f32, 6>;
    var sumx = 0.0;
    var sumy = 0.0;
    for (var i = 0; i < 6; i++) {
        wx[i] = lanczos3(f32(i) - 2.0 - (sx - fx));
        sumx += wx[i];
        wy[i] = lanczos3(f32(i) - 2.0 - (sy - fy));
        sumy += wy[i];
    }
    for (var i = 0; i < 6; i++) {
        wx[i] = wx[i] / sumx;
        wy[i] = wy[i] / sumy;
    }
    var acc = vec4<f32>(0.0);
    for (var j = 0; j < 6; j++) {
        var row = vec4<f32>(0.0);
        for (var i = 0; i < 6; i++) {
            row += wx[i] * lens_fetch(ix - 2 + i, iy - 2 + j);
        }
        acc += wy[j] * row;
    }
    return clamp_premultiplied(acc);
}

// canvas_ro = backdrop snapshot; mode 0 Catmull-Rom, 1 Lanczos3, 2 nearest,
// 3 nearest + pixel grid.
@compute @workgroup_size(16, 16)
fn magnifier(@builtin(global_invocation_id) g: vec3<u32>) {
    let r = lens_p.rect;
    let x = r.x + i32(g.x);
    let y = r.y + i32(g.y);
    if (x >= r.z || y >= r.w) {
        return;
    }
    let m = mask_at(x, y, lens_p.mask_rect);
    if (m <= 0.0) {
        return;
    }
    let inv_zoom = lens_p.zoom.x;
    let u = vec2<f32>((f32(x) + 0.5 - lens_p.geo.x) * inv_zoom + lens_p.geo.z, (f32(y) + 0.5 - lens_p.geo.y) * inv_zoom + lens_p.geo.w);
    var s: vec4<f32>;
    switch lens_p.mode.x {
        case 0u: {
            s = sample_catmull_rom(u);
        }
        case 1u: {
            s = sample_lanczos3(u);
        }
        case 2u: {
            s = lens_fetch(i32(floor(u.x)), i32(floor(u.y)));
        }
        default: {
            let f = floor(u);
            s = lens_fetch(i32(f.x), i32(f.y));
            if (u.x - f.x < inv_zoom || u.y - f.y < inv_zoom) {
                s = over(lens_p.grid, s);
            }
        }
    }
    let i = cidx(x, y, lens_p.size.z);
    canvas[i] = mix(canvas[i], s, m);
}

// ------------------------------------------------------------- spotlight

// flags = (has_mask, has_blur); mask = feathered targets; tmp_in = blurred
// backdrop over rect.
@compute @workgroup_size(16, 16)
fn spotlight(@builtin(global_invocation_id) g: vec3<u32>) {
    let r = spot_p.rect;
    let x = r.x + i32(g.x);
    let y = r.y + i32(g.y);
    if (x >= r.z || y >= r.w) {
        return;
    }
    var outside = 1.0;
    if (spot_p.flags.x != 0u) {
        outside = 1.0 - mask_at(x, y, spot_p.mask_rect);
    }
    if (outside <= 0.0) {
        return;
    }
    let i = cidx(x, y, spot_p.size.z);
    let b = canvas[i];
    var f = b;
    if (spot_p.flags.y != 0u) {
        f = tmp_in[u32((y - r.y) * (r.z - r.x) + (x - r.x))];
    }
    let a = spot_p.dim.w;
    let k = 1.0 - a;
    let fa = a * f.a;
    let d = vec4<f32>(f.r * k + spot_p.dim.r * fa, f.g * k + spot_p.dim.g * fa, f.b * k + spot_p.dim.b * fa, f.a);
    canvas[i] = mix(b, d, outside);
}

// --------------------------------------------------------------- cut out

fn hash32(v: u32) -> u32 {
    var x = v;
    x ^= x >> 16u;
    x *= 0x7feb352du;
    x ^= x >> 15u;
    x *= 0x846ca68bu;
    x ^= x >> 16u;
    return x;
}

fn lattice(seed: vec2<u32>, octave: u32, i: i32) -> f32 {
    let h = hash32(hash32(hash32(bitcast<u32>(i) ^ seed.x) + seed.y) ^ (octave * 0x9e3779b9u));
    return f32(h >> 8u) * (1.0 / 16777215.0) * 2.0 - 1.0;
}

fn noise1(x: f32, seed: vec2<u32>, octave: u32) -> f32 {
    let fi = floor(x);
    let f = x - fi;
    let i = i32(fi);
    let g0 = lattice(seed, octave, i) * f;
    let g1 = lattice(seed, octave, i + 1) * (f - 1.0);
    let t = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    return (g0 + (g1 - g0) * t) * 2.0;
}

fn fbm(s: f32, seed: vec2<u32>) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var norm = 0.0;
    var freq = cut_p.noise.z;
    for (var o = 0u; o < cut_p.dst.w; o++) {
        sum += amp * noise1(s * freq, seed, o);
        norm += amp;
        amp *= cut_p.noise.y;
        freq *= 2.0;
    }
    return sum / max(norm, 1e-6);
}

fn profile(s: f32, seed: vec2<u32>) -> f32 {
    return clamp(0.5 + 0.75 * fbm(s, seed), 0.0, 1.0);
}

fn cut_edges(along: f32) -> vec2<f32> {
    let t0 = f32(cut_p.geo.x);
    let gap = f32(cut_p.geo.z);
    let amp = cut_p.noise.x;
    if (amp <= 0.0) {
        return vec2<f32>(t0, t0 + gap);
    }
    let hu = profile(along, cut_p.seeds.xy);
    if (cut_p.geo.z == 0) {
        let e = t0 + amp * (hu - 0.5);
        return vec2<f32>(e, e);
    }
    let hl = profile(along, cut_p.seeds.zw);
    return vec2<f32>(t0 - amp * hu, t0 + gap + amp * hl);
}

fn cut_src(a: i32, across: i32) -> vec4<f32> {
    if (cut_p.src.w != 0u) {
        return canvas_ro[cidx(across, a, cut_p.src.z)];
    }
    return canvas_ro[cidx(a, across, cut_p.src.z)];
}

// canvas_ro = source canvas, canvas = destination (cut size).
@compute @workgroup_size(16, 16)
fn cutout(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= cut_p.dst.x || g.y >= cut_p.dst.y) {
        return;
    }
    let vertical = cut_p.src.w != 0u;
    var a = i32(g.x);
    var c = i32(g.y);
    var across_len = i32(cut_p.src.y);
    if (vertical) {
        a = i32(g.y);
        c = i32(g.x);
        across_len = i32(cut_p.src.x);
    }
    let along = f32(a) + 0.5;
    let e = cut_edges(along);
    let cf = f32(c);
    let cu = clamp(e.x - cf, 0.0, 1.0);
    let cl = clamp(cf + 1.0 - e.y, 0.0, 1.0);
    let cu_src = clamp(c, 0, across_len - 1);
    let cl_src = clamp(c - cut_p.geo.z + cut_p.geo.y, 0, across_len - 1);
    var out = cut_p.gap_fill * max(1.0 - cu - cl, 0.0);
    out = over(cut_src(a, cl_src) * cl, out);
    if (cut_p.shadow.w > 0.5) {
        let n = u32(cut_p.shadow.z);
        var s = 0.0;
        for (var k = 0u; k < n; k++) {
            let t = f32(k + 1u) / f32(n);
            let ea = cut_edges(along - cut_p.shadow.x * t).x;
            s += clamp(ea - (cf - cut_p.shadow.y * t), 0.0, 1.0);
        }
        s = s / f32(n);
        out = over(cut_p.shadow_color * s, out);
    }
    out = over(cut_src(a, cu_src) * cu, out);
    canvas[cidx(i32(g.x), i32(g.y), cut_p.dst.z)] = out;
}

// ---------------------------------------------------------- shadow frame

// canvas_ro = source canvas, canvas = destination (frame size); mask =
// blurred shadow, clip = rounded-rect coverage, mask2 = border (CPU:
// FrameOp::compose).
@compute @workgroup_size(16, 16)
fn frame(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= frame_p.dst.x || g.y >= frame_p.dst.y) {
        return;
    }
    let x = i32(g.x);
    let y = i32(g.y);
    var out = paint_at(x, y, frame_p.paint, frame_p.geo, frame_p.color);
    if ((frame_p.dst.w & 1u) != 0u) {
        let s = mask_at(x, y, frame_p.shadow_rect);
        if (s > 0.0) {
            out = over(frame_p.shadow_color * s, out);
        }
    }
    let k = clip_at(x, y, frame_p.clip_rect);
    if (k > 0.0) {
        let m = i32(frame_p.src.w);
        let sx = x - m;
        let sy = y - m;
        var src = vec4<f32>(0.0);
        if (sx >= 0 && sy >= 0 && sx < i32(frame_p.src.x) && sy < i32(frame_p.src.y)) {
            src = canvas_ro[cidx(sx, sy, frame_p.src.z)];
        }
        out = over(src * k, out);
    }
    if ((frame_p.dst.w & 2u) != 0u) {
        let b = mask2_at(x, y, frame_p.border_rect);
        if (b > 0.0) {
            out = over(frame_p.border_color * b, out);
        }
    }
    canvas[cidx(x, y, frame_p.dst.z)] = out;
}

// ---------------------------------------------------------------- present

// Canvas -> premultiplied RGBA8 texture for the editor view.
@compute @workgroup_size(16, 16)
fn present(@builtin(global_invocation_id) g: vec3<u32>) {
    let s = present_p.size;
    if (g.x >= s.x || g.y >= s.y) {
        return;
    }
    textureStore(present_out, vec2<i32>(i32(g.x), i32(g.y)), clamp(canvas_ro[cidx(i32(g.x), i32(g.y), s.z)], vec4<f32>(0.0), vec4<f32>(1.0)));
}
