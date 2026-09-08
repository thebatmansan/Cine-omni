// Chroma keyer in YCbCr with spill suppression. Outputs premultiplied RGBA.

struct KeyParams {
    key_color: vec4<f32>,
    width: u32,
    height: u32,
    tolerance: f32,
    softness: f32,
    spill: f32,
    _p0: f32,
    _p1: f32,
    _p2: f32,
};

@group(0) @binding(0) var<uniform> params: KeyParams;
@group(0) @binding(1) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> dst: array<vec4<f32>>;

fn to_cbcr(c: vec3<f32>) -> vec2<f32> {
    let l = luma709(c);
    return vec2<f32>((c.b - l) / 1.8556, (c.r - l) / 1.5748);
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    let i = idx2d(gid.x, gid.y, params.width);
    let p = src[i];
    let d = distance(to_cbcr(p.rgb), to_cbcr(params.key_color.rgb));
    let alpha = smoothstep(params.tolerance, params.tolerance + params.softness, d);
    // Spill suppression: pull the key channel towards the average of the others.
    var c = p.rgb;
    let key = params.key_color.rgb;
    if (key.g > key.r && key.g > key.b) {
        let limit = (c.r + c.b) * 0.5;
        c.g = mix(c.g, min(c.g, limit), params.spill);
    } else if (key.b > key.r && key.b > key.g) {
        let limit = (c.r + c.g) * 0.5;
        c.b = mix(c.b, min(c.b, limit), params.spill);
    }
    dst[i] = vec4<f32>(c * alpha, alpha);
}
