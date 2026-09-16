// Presents planar YUV directly, converting in the fragment shader.
//
// The path this replaces wrote the conversion to a storage buffer, read it back
// to the CPU, and uploaded it again as a 32-bit float texture: roughly 30 MB
// across the bus per picture to deliver 1.4 MB of planes, measured at 69.4 ms
// each on this machine. Here the planes go up as three 8-bit textures and the
// conversion happens where the pixels already are.
//
// The arithmetic is deliberately identical to `cine-gpu`'s `yuv_to_rgb` compute
// kernel and to `yuv_to_rgb_cpu`, which is the definition both are checked
// against. `textureLoad` with integer coordinates rather than a sampler, so at
// 1:1 the result is exactly the reference's, not a filtered approximation of it.

struct YuvParams {
    width: u32,
    height: u32,
    chroma_width: u32,
    chroma_height: u32,
    // Luma coefficients of the chosen matrix; kg is derived as 1 - kr - kb.
    kr: f32,
    kb: f32,
    // Range normalisation: v' = (v - offset) * gain.
    y_offset: f32,
    y_gain: f32,
    c_offset: f32,
    c_gain: f32,
    _pad0: f32,
    _pad1: f32,
};

struct LutParams {
    size: u32,
    enabled: u32,
    pad0: u32,
    pad1: u32,
    domain_min: vec4<f32>,
    domain_max: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: YuvParams;
@group(0) @binding(1) var plane_y: texture_2d<f32>;
@group(0) @binding(2) var plane_u: texture_2d<f32>;
@group(0) @binding(3) var plane_v: texture_2d<f32>;
@group(0) @binding(4) var<uniform> lut: LutParams;
@group(0) @binding(5) var view_lut: texture_3d<f32>;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VertexOut;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    // Flip vertically: image rows run top-down, clip space runs bottom-up.
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

fn ycbcr_to_rgb(y: f32, cb: f32, cr: f32) -> vec3<f32> {
    let kr = params.kr;
    let kb = params.kb;
    let kg = 1.0 - kr - kb;
    let r = y + 2.0 * (1.0 - kr) * cr;
    let b = y + 2.0 * (1.0 - kb) * cb;
    // Green from the luma identity rather than a separate constant, so it stays
    // consistent with whatever kr/kb were supplied.
    let g = (y - kr * r - kb * b) / kg;
    return vec3<f32>(r, g, b);
}

// The view transform: the project's colour config, baked to a lattice by the
// runtime and interpolated here.
//
// Trilinear, written out rather than left to a sampler, so that it is exactly
// the interpolation `Lut3D::worst_error` measures on the CPU — the number the
// application quotes for how far the lattice is from the transform itself.
fn apply_view_lut(rgb: vec3<f32>) -> vec3<f32> {
    if (lut.enabled == 0u) {
        return rgb;
    }
    let n = f32(lut.size);
    let span = max(lut.domain_max.xyz - lut.domain_min.xyz, vec3<f32>(1e-12));
    let t = clamp((rgb - lut.domain_min.xyz) / span, vec3<f32>(0.0), vec3<f32>(1.0)) * (n - 1.0);
    let base = min(floor(t), vec3<f32>(n - 2.0));
    let f = t - base;
    let i = vec3<u32>(base);
    var out = vec3<f32>(0.0);
    for (var c = 0u; c < 8u; c = c + 1u) {
        let d = vec3<u32>(c & 1u, (c >> 1u) & 1u, (c >> 2u) & 1u);
        let w = mix(1.0 - f.x, f.x, f32(d.x))
              * mix(1.0 - f.y, f.y, f32(d.y))
              * mix(1.0 - f.z, f.z, f32(d.z));
        out = out + w * textureLoad(view_lut, i + d, 0).rgb;
    }
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let x = min(u32(in.uv.x * f32(params.width)), params.width - 1u);
    let y = min(u32(in.uv.y * f32(params.height)), params.height - 1u);
    // Nearest-sample chroma, by the same integer rule as the reference. Correct
    // siting per H.273 needs the chroma location code point, which decoded media
    // here does not carry; interpolating as if it were known would be a guess
    // dressed as precision.
    let cx = min(x * params.chroma_width / params.width, params.chroma_width - 1u);
    let cy = min(y * params.chroma_height / params.height, params.chroma_height - 1u);

    let luma = (textureLoad(plane_y, vec2<u32>(x, y), 0).r - params.y_offset) * params.y_gain;
    let cb = (textureLoad(plane_u, vec2<u32>(cx, cy), 0).r - params.c_offset) * params.c_gain;
    let cr = (textureLoad(plane_v, vec2<u32>(cx, cy), 0).r - params.c_offset) * params.c_gain;
    return vec4<f32>(apply_view_lut(ycbcr_to_rgb(luma, cb, cr)), 1.0);
}
