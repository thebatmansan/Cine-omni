// Porter-Duff merge of B over A (premultiplied RGBA, scene-linear).
// op codes match cine_gpu::kernels::MergeOp.

struct MergeParams {
    width: u32,
    height: u32,
    op: u32,
    mix: f32,
};

@group(0) @binding(0) var<uniform> params: MergeParams;
@group(0) @binding(1) var<storage, read> a: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> b: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> out: array<vec4<f32>>;

fn merge_op(op: u32, pa: vec4<f32>, pb: vec4<f32>) -> vec4<f32> {
    switch op {
        case 0u: { return pb + pa * (1.0 - pb.a); }                       // over
        case 1u: { return pa + pb * (1.0 - pa.a); }                       // under
        case 2u: { return pa + pb; }                                      // plus
        case 3u: { return pa - pb; }                                      // minus
        case 4u: { return pa * pb; }                                      // multiply
        case 5u: { return pa + pb - pa * pb; }                            // screen
        case 6u: { return abs(pa - pb); }                                 // difference
        case 7u: { return max(pa, pb); }                                  // max
        case 8u: { return min(pa, pb); }                                  // min
        case 9u: { return pa * pb.a; }                                    // mask
        case 10u: { return pa * (1.0 - pb.a); }                           // stencil
        case 11u: { return pa * (1.0 - pb.a) + pb * (1.0 - pa.a); }       // xor
        case 12u: { return pb * pa.a; }                                   // in
        case 13u: { return pb * (1.0 - pa.a); }                           // out
        case 14u: { return pb * pa.a + pa * (1.0 - pb.a); }               // atop
        default: { return pb + pa * (1.0 - pb.a); }
    }
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    let i = idx2d(gid.x, gid.y, params.width);
    let pa = a[i];
    let pb = b[i];
    let merged = merge_op(params.op, pa, pb);
    out[i] = mix(pa, merged, params.mix);
}
