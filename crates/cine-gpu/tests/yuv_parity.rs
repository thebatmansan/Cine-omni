//! The YUV→RGB kernel must agree with its CPU reference on real hardware.
//!
//! A GPU path that has never been compared against a definition is not an
//! optimisation, it is a second implementation with no oracle. These tests run
//! the compute kernel on the actual adapter and compare it, pixel for pixel,
//! against `cine_gpu::yuv::yuv_to_rgb_cpu`.
//!
//! If no adapter is present the tests skip **loudly**, because a GPU test that
//! quietly passes with no GPU is worse than no test.

#![cfg(feature = "wgpu")]

use cine_gpu::device::{Arg, GpuContext};
use cine_gpu::kernels::KernelId;
use cine_gpu::yuv::{yuv_to_rgb_cpu, Matrix, MatrixChoice, Range, YuvConversion};

fn context() -> Option<GpuContext> {
    match GpuContext::new() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("SKIP: no GPU adapter on this machine ({e})");
            None
        }
    }
}

/// A deterministic pseudo-image with full-range chroma excursions, so the
/// comparison exercises saturated colour rather than only greys.
fn synthetic_planes(w: usize, h: usize) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let (cw, ch) = (w / 2, h / 2);
    let y = (0..w * h)
        .map(|i| ((i * 37 % 256) as f32) / 255.0)
        .collect();
    let u = (0..cw * ch)
        .map(|i| ((i * 91 % 256) as f32) / 255.0)
        .collect();
    let v = (0..cw * ch)
        .map(|i| ((i * 53 % 256) as f32) / 255.0)
        .collect();
    (y, u, v)
}

fn run_gpu(
    ctx: &mut GpuContext,
    conv: &YuvConversion,
    y: &[f32],
    u: &[f32],
    v: &[f32],
) -> Vec<f32> {
    let params = conv.params();
    let bytes: Vec<u8> = params.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let out = conv.width as usize * conv.height as usize * 4;
    let mut results = ctx
        .dispatch(
            KernelId::YuvToRgb,
            &[
                Arg::Uniform(&bytes),
                Arg::Storage(y),
                Arg::Storage(u),
                Arg::Storage(v),
                Arg::Output(out),
            ],
            [(conv.width).div_ceil(16), (conv.height).div_ceil(16), 1],
        )
        .expect("dispatch");
    let bytes = results.pop().expect("output buffer");
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_ne_bytes(*b))
        .collect()
}

fn conversion(matrix: Matrix, range: Range, w: u32, h: u32) -> YuvConversion {
    YuvConversion {
        choice: MatrixChoice::Declared { code: 1, matrix },
        range,
        width: w,
        height: h,
        chroma_width: w / 2,
        chroma_height: h / 2,
    }
}

#[test]
fn gpu_yuv_conversion_matches_the_cpu_reference() {
    let Some(mut ctx) = context() else { return };
    eprintln!("adapter: {}", ctx.adapter_name());

    const W: u32 = 64;
    const H: u32 = 64;
    let (y, u, v) = synthetic_planes(W as usize, H as usize);

    // Every combination that a real file can ask for.
    for matrix in [Matrix::BT601, Matrix::BT709, Matrix::BT2020_NCL] {
        for range in [Range::Limited, Range::Full] {
            let conv = conversion(matrix, range, W, H);
            let gpu = run_gpu(&mut ctx, &conv, &y, &u, &v);
            let cpu = yuv_to_rgb_cpu(&conv, &y, &u, &v);
            assert_eq!(gpu.len(), cpu.len());

            let mut worst = 0.0f32;
            let mut worst_at = 0;
            for (i, (g, c)) in gpu.iter().zip(&cpu).enumerate() {
                let d = (g - c).abs();
                if d > worst {
                    worst = d;
                    worst_at = i;
                }
            }
            // f32 arithmetic in a different order; a few ULP is expected, a
            // visible difference is not.
            assert!(
                worst < 1e-4,
                "{matrix:?} {range:?}: GPU and CPU differ by {worst} at index {worst_at} \
                 (gpu {}, cpu {})",
                gpu[worst_at],
                cpu[worst_at]
            );
        }
    }
}

/// The conversion must be exercised on a real decoded frame, not only on
/// synthetic planes, because real media has the geometry and the chroma
/// subsampling that synthetic data can accidentally avoid.
#[test]
fn gpu_conversion_matches_cpu_on_a_real_decoded_frame() {
    let path =
        std::path::Path::new("/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4");
    if !path.exists() {
        eprintln!("SKIP: no real media on this machine");
        return;
    }
    let Some(mut ctx) = context() else { return };

    let mut reader = cine_media::VideoReader::open_path(path).expect("open video");
    let frame = reader.next_frame().expect("decode").expect("a frame");

    // Normalise the decoded 8-bit planes to 0..1, which is what the kernel
    // consumes. Crop to a manageable window: the point is correctness, and a
    // full 720x1280 readback per matrix would dominate the test's runtime.
    const W: u32 = 128;
    const H: u32 = 128;
    let (yp, y_stride) = frame.plane(0).unwrap();
    let (up, c_stride) = frame.plane(1).unwrap();
    let (vp, _) = frame.plane(2).unwrap();

    let crop = |src: &[u8], stride: usize, w: u32, h: u32| -> Vec<f32> {
        (0..h as usize)
            .flat_map(|row| (0..w as usize).map(move |col| src[row * stride + col] as f32 / 255.0))
            .collect()
    };
    let y = crop(yp, y_stride, W, H);
    let u = crop(up, c_stride, W / 2, H / 2);
    let v = crop(vp, c_stride, W / 2, H / 2);

    // The file declares no matrix, so the viewer must assume one — and say so.
    let choice = MatrixChoice::resolve(None, frame.height);
    assert!(
        choice.is_assumed(),
        "this asset carries no matrix_coefficients"
    );
    let conv = YuvConversion {
        choice,
        range: Range::Limited,
        width: W,
        height: H,
        chroma_width: W / 2,
        chroma_height: H / 2,
    };

    let gpu = run_gpu(&mut ctx, &conv, &y, &u, &v);
    let cpu = yuv_to_rgb_cpu(&conv, &y, &u, &v);
    let worst = gpu
        .iter()
        .zip(&cpu)
        .map(|(g, c)| (g - c).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "real-frame GPU/CPU mismatch of {worst}");

    // And the result must be a real picture: not uniform, and in range.
    let luma: Vec<f32> = cpu.chunks(4).map(|p| p[1]).collect();
    let (lo, hi) = luma
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    assert!(
        hi - lo > 0.05,
        "converted frame is nearly uniform: {lo}..{hi}"
    );
    assert!(
        lo > -0.1 && hi < 1.6,
        "converted values are wildly out of range: {lo}..{hi}"
    );
}
