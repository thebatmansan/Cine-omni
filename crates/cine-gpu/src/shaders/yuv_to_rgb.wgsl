// Planar YUV -> linear-coded RGB, for presenting decoded camera media.
//
// The matrix is a parameter, never a constant, because the correct one depends
// on metadata the file may not carry. When it is absent the caller must choose
// explicitly and say so; this shader will not pick for it.
//
// Input planes arrive as normalised code values (byte / 255) so the same shader
// serves 8-bit and, later, higher depths without a second code path.

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

@group(0) @binding(0) var<uniform> params: YuvParams;
@group(0) @binding(1) var<storage, read> plane_y: array<f32>;
@group(0) @binding(2) var<storage, read> plane_u: array<f32>;
@group(0) @binding(3) var<storage, read> plane_v: array<f32>;
@group(0) @binding(4) var<storage, read_write> dst: array<vec4<f32>>;

fn ycbcr_to_rgb(y: f32, cb: f32, cr: f32) -> vec3<f32> {
    let kr = params.kr;
    let kb = params.kb;
    let kg = 1.0 - kr - kb;
    let r = y + 2.0 * (1.0 - kr) * cr;
    let b = y + 2.0 * (1.0 - kb) * cb;
    // Green is recovered from the luma identity rather than a separate
    // constant, so it stays consistent with whatever kr/kb were supplied.
    let g = (y - kr * r - kb * b) / kg;
    return vec3<f32>(r, g, b);
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }

    // Nearest-sample chroma. Correct siting per H.273 needs the chroma location
    // code point, which decoded media here does not carry; interpolating as if
    // it were known would be a guess dressed as precision.
    let cx = min(gid.x * params.chroma_width / params.width, params.chroma_width - 1u);
    let cy = min(gid.y * params.chroma_height / params.height, params.chroma_height - 1u);

    let yi = idx2d(gid.x, gid.y, params.width);
    let ci = idx2d(cx, cy, params.chroma_width);

    let y = (plane_y[yi] - params.y_offset) * params.y_gain;
    let cb = (plane_u[ci] - params.c_offset) * params.c_gain;
    let cr = (plane_v[ci] - params.c_offset) * params.c_gain;

    dst[yi] = vec4<f32>(ycbcr_to_rgb(y, cb, cr), 1.0);
}
