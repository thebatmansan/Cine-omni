// Separable Gaussian blur. Run twice: horizontal=1 then horizontal=0.

struct BlurParams {
    width: u32,
    height: u32,
    radius: u32,
    horizontal: u32,
    sigma: f32,
    _p0: f32,
    _p1: f32,
    _p2: f32,
};

@group(0) @binding(0) var<uniform> params: BlurParams;
@group(0) @binding(1) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> dst: array<vec4<f32>>;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    var sum = vec4<f32>(0.0);
    var wsum = 0.0;
    let r = i32(params.radius);
    let two_s2 = 2.0 * params.sigma * params.sigma;
    for (var k = -r; k <= r; k = k + 1) {
        var x = i32(gid.x);
        var y = i32(gid.y);
        if (params.horizontal == 1u) { x = x + k; } else { y = y + k; }
        x = clamp(x, 0, i32(params.width) - 1);
        y = clamp(y, 0, i32(params.height) - 1);
        let w = exp(-f32(k * k) / two_s2);
        sum = sum + src[idx2d(u32(x), u32(y), params.width)] * w;
        wsum = wsum + w;
    }
    dst[idx2d(gid.x, gid.y, params.width)] = sum / wsum;
}
