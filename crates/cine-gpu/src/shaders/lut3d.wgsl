// 3D LUT with tetrahedral interpolation. LUT stored as a flat vec4 array,
// red fastest (the .cube convention), size^3 entries.

struct LutParams {
    size: u32,
    width: u32,
    height: u32,
    _pad: u32,
    domain_min: vec4<f32>,
    domain_max: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: LutParams;
@group(0) @binding(1) var<storage, read> lut: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> dst: array<vec4<f32>>;

fn lut_at(r: u32, g: u32, b: u32) -> vec3<f32> {
    let n = params.size;
    return lut[r + g * n + b * n * n].rgb;
}

fn sample_tetrahedral(c: vec3<f32>) -> vec3<f32> {
    let n = f32(params.size - 1u);
    let norm = clamp((c - params.domain_min.rgb) / (params.domain_max.rgb - params.domain_min.rgb), vec3<f32>(0.0), vec3<f32>(1.0));
    let p = norm * n;
    let i0 = vec3<u32>(floor(p));
    let i1 = min(i0 + vec3<u32>(1u), vec3<u32>(u32(n)));
    let f = p - vec3<f32>(i0);

    let c000 = lut_at(i0.x, i0.y, i0.z);
    let c111 = lut_at(i1.x, i1.y, i1.z);
    var result: vec3<f32>;
    if (f.r >= f.g) {
        if (f.g >= f.b) {
            let c100 = lut_at(i1.x, i0.y, i0.z);
            let c110 = lut_at(i1.x, i1.y, i0.z);
            result = (1.0 - f.r) * c000 + (f.r - f.g) * c100 + (f.g - f.b) * c110 + f.b * c111;
        } else if (f.r >= f.b) {
            let c100 = lut_at(i1.x, i0.y, i0.z);
            let c101 = lut_at(i1.x, i0.y, i1.z);
            result = (1.0 - f.r) * c000 + (f.r - f.b) * c100 + (f.b - f.g) * c101 + f.g * c111;
        } else {
            let c001 = lut_at(i0.x, i0.y, i1.z);
            let c101 = lut_at(i1.x, i0.y, i1.z);
            result = (1.0 - f.b) * c000 + (f.b - f.r) * c001 + (f.r - f.g) * c101 + f.g * c111;
        }
    } else {
        if (f.b >= f.g) {
            let c001 = lut_at(i0.x, i0.y, i1.z);
            let c011 = lut_at(i0.x, i1.y, i1.z);
            result = (1.0 - f.b) * c000 + (f.b - f.g) * c001 + (f.g - f.r) * c011 + f.r * c111;
        } else if (f.b >= f.r) {
            let c010 = lut_at(i0.x, i1.y, i0.z);
            let c011 = lut_at(i0.x, i1.y, i1.z);
            result = (1.0 - f.g) * c000 + (f.g - f.b) * c010 + (f.b - f.r) * c011 + f.r * c111;
        } else {
            let c010 = lut_at(i0.x, i1.y, i0.z);
            let c110 = lut_at(i1.x, i1.y, i0.z);
            result = (1.0 - f.g) * c000 + (f.g - f.r) * c010 + (f.r - f.b) * c110 + f.b * c111;
        }
    }
    return result;
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    let i = idx2d(gid.x, gid.y, params.width);
    let p = src[i];
    dst[i] = vec4<f32>(sample_tetrahedral(p.rgb), p.a);
}
