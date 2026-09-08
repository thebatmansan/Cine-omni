// Depth of field post pass: per-pixel circle of confusion from the physical
// camera (focal length, f-stop, focus distance, sensor width) and a gather blur.

struct DofParams {
    width: u32,
    height: u32,
    max_radius: u32,
    _pad: u32,
    // x = focal length mm, y = f-stop, z = focus distance m, w = sensor width mm
    optics: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: DofParams;
@group(0) @binding(1) var<storage, read> color: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> depth: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> dst: array<vec4<f32>>;

// Circle of confusion diameter in pixels for a point at distance d (metres).
fn coc_px(d: f32) -> f32 {
    let f = params.optics.x / 1000.0;
    let N = params.optics.y;
    let s = params.optics.z;
    let sensor_w = params.optics.w / 1000.0;
    let aperture = f / N;
    let c = aperture * abs(d - s) / max(d, 1e-4) * (f / max(s - f, 1e-4));
    return c / sensor_w * f32(params.width);
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    let i = idx2d(gid.x, gid.y, params.width);
    let d0 = depth[i].r;
    let radius = min(coc_px(d0) * 0.5, f32(params.max_radius));
    if (radius < 0.5) { dst[i] = color[i]; return; }
    var sum = vec4<f32>(0.0);
    var wsum = 0.0;
    let r = i32(ceil(radius));
    for (var dy = -r; dy <= r; dy = dy + 1) {
        for (var dx = -r; dx <= r; dx = dx + 1) {
            let dist = sqrt(f32(dx * dx + dy * dy));
            if (dist > radius) { continue; }
            let x = clamp(i32(gid.x) + dx, 0, i32(params.width) - 1);
            let y = clamp(i32(gid.y) + dy, 0, i32(params.height) - 1);
            let j = idx2d(u32(x), u32(y), params.width);
            // Avoid foreground bleeding: only gather samples whose own CoC covers this pixel.
            let w = select(0.0, 1.0, coc_px(depth[j].r) * 0.5 >= dist - 0.5);
            sum = sum + color[j] * w;
            wsum = wsum + w;
        }
    }
    dst[i] = select(color[i], sum / wsum, wsum > 0.0);
}
