// Presents an RGBA texture on a fullscreen triangle.
//
// A triangle rather than a quad: it covers the viewport with three vertices and
// no diagonal seam, which is the standard way to do this.

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    // (0,0) (2,0) (0,2) in UV, giving clip-space coverage of the whole viewport.
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VertexOut;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    // Flip vertically: image rows run top-down, clip space runs bottom-up.
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(src, src_sampler, in.uv);
}
