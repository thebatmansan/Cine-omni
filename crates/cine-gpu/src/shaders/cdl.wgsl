// ASC CDL (slope / offset / power / saturation) on RGBA buffers.

struct CdlParams {
    slope: vec4<f32>,
    offset: vec4<f32>,
    power: vec4<f32>,
    saturation: f32,
    width: u32,
    height: u32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> params: CdlParams;
@group(0) @binding(1) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> dst: array<vec4<f32>>;

fn apply_cdl(c: vec3<f32>) -> vec3<f32> {
    let sop = pow(max(c * params.slope.rgb + params.offset.rgb, vec3<f32>(0.0)), params.power.rgb);
    let l = luma709(sop);
    return max(vec3<f32>(l) + params.saturation * (sop - vec3<f32>(l)), vec3<f32>(0.0));
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    let i = idx2d(gid.x, gid.y, params.width);
    let p = src[i];
    dst[i] = vec4<f32>(apply_cdl(p.rgb), p.a);
}
