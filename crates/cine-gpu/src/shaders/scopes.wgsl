// Histogram + waveform + vectorscope accumulation with atomics.
// Bins: histogram 4*256 (r,g,b,luma), waveform width*256 luma, vectorscope 256*256.

struct ScopeParams {
    width: u32,
    height: u32,
    hdr_pq: u32,      // 1 = interpret input as PQ code values for nits scale
    _pad: u32,
};

@group(0) @binding(0) var<uniform> params: ScopeParams;
@group(0) @binding(1) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> histogram: array<atomic<u32>>;
@group(0) @binding(3) var<storage, read_write> waveform: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> vectorscope: array<atomic<u32>>;

fn bin256(v: f32) -> u32 {
    return u32(clamp(v, 0.0, 1.0) * 255.0 + 0.5);
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    let p = src[idx2d(gid.x, gid.y, params.width)];
    var c = p.rgb;
    if (params.hdr_pq == 1u) {
        // Map nits (0..10000) to PQ code so the waveform reads in nits.
        c = pq_encode(c / 10000.0);
    }
    let l = luma709(c);
    atomicAdd(&histogram[bin256(c.r)], 1u);
    atomicAdd(&histogram[256u + bin256(c.g)], 1u);
    atomicAdd(&histogram[512u + bin256(c.b)], 1u);
    atomicAdd(&histogram[768u + bin256(l)], 1u);
    atomicAdd(&waveform[gid.x * 256u + bin256(l)], 1u);
    // Vectorscope: Cb/Cr of Rec.709, centred at (128,128).
    let cb = (c.b - l) / 1.8556;
    let cr = (c.r - l) / 1.5748;
    let vx = bin256(cb + 0.5);
    let vy = bin256(cr + 0.5);
    atomicAdd(&vectorscope[vy * 256u + vx], 1u);
}
