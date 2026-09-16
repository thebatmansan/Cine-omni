//! The planar presentation path, checked against the CPU reference.
//!
//! Converting in the fragment shader made presentation much faster. That is only
//! worth having if it produces the *same picture*, so this renders through the
//! real pipeline on real hardware and compares every pixel against
//! `cine_gpu::yuv::yuv_to_rgb_cpu` — the same definition the compute kernel is
//! verified against.
//!
//! Rendered 1:1, so nothing is resampled and the comparison is exact up to
//! 8-bit quantisation. The surface this feeds is 8-bit, so that is the precision
//! that matters; a tolerance of one code value covers the difference between the
//! GPU's rounding and Rust's.
//!
//! Skips loudly when there is no GPU.

use cine_color::lut::{Interpolation, Lut3D};
use cine_gpu::yuv::{yuv_to_rgb_cpu, MatrixChoice, Range, YuvConversion};
use cine_shell::present::Presenter;

const W: u32 = 64;
const H: u32 = 32;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: String,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .ok()?;
    let name = adapter.get_info().name;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("planar present test"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        experimental_features: wgpu::ExperimentalFeatures::default(),
        trace: wgpu::Trace::Off,
    }))
    .ok()?;
    Some(Gpu {
        device,
        queue,
        adapter: name,
    })
}

macro_rules! need {
    ($opt:expr, $what:expr) => {
        match $opt {
            Some(v) => v,
            None => {
                eprintln!("SKIP: no {} on this machine", $what);
                return;
            }
        }
    };
}

/// Planes chosen to exercise both ends of the legal range and both chroma axes.
fn planes() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (cw, ch) = ((W / 2) as usize, (H / 2) as usize);
    let y: Vec<u8> = (0..(W * H))
        .map(|i| {
            let x = i % W;
            (x * 255 / (W - 1)) as u8
        })
        .collect();
    let u: Vec<u8> = (0..(cw * ch))
        .map(|i| ((i / cw) * 255 / ch.max(1)) as u8)
        .collect();
    let v: Vec<u8> = (0..(cw * ch)).map(|i| ((i % cw) * 8 % 256) as u8).collect();
    (y, u, v)
}

fn conversion(range: Range, matrix: MatrixChoice) -> YuvConversion {
    YuvConversion {
        choice: matrix,
        range,
        width: W,
        height: H,
        chroma_width: W / 2,
        chroma_height: H / 2,
    }
}

/// Render one picture through the real pipeline and read the result back.
fn render(gpu: &Gpu, conv: &YuvConversion, planes: [(&[u8], usize); 3]) -> Vec<u8> {
    render_through(gpu, conv, planes, None)
}

/// Render one picture, optionally through a view transform.
fn render_through(
    gpu: &Gpu,
    conv: &YuvConversion,
    planes: [(&[u8], usize); 3],
    lut: Option<&std::sync::Arc<Lut3D>>,
) -> Vec<u8> {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut presenter = Presenter::new(&gpu.device, format);
    presenter
        .set_planes(&gpu.device, &gpu.queue, conv, planes)
        .expect("planes upload");
    presenter
        .set_view_lut(&gpu.device, &gpu.queue, lut)
        .expect("view lut upload");
    assert_eq!(presenter.has_view_lut(), lut.is_some());

    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    presenter.draw(&gpu.device, &gpu.queue, &view);

    // 64 px x 4 bytes is exactly the 256-byte row alignment a copy needs.
    let row = (W * 4) as u64;
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: row * H as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row as u32),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit(Some(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv()
        .expect("map callback")
        .expect("mapping the readback buffer");
    let result = slice.get_mapped_range().expect("mapped range").to_vec();
    readback.unmap();
    result
}

fn reference(conv: &YuvConversion, y: &[u8], u: &[u8], v: &[u8]) -> Vec<u8> {
    let norm = |p: &[u8]| -> Vec<f32> { p.iter().map(|&b| f32::from(b) / 255.0).collect() };
    let rgba = yuv_to_rgb_cpu(conv, &norm(y), &norm(u), &norm(v));
    rgba.iter()
        .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

fn compare(label: &str, got: &[u8], want: &[u8]) {
    assert_eq!(got.len(), want.len(), "{label}: size");
    let mut worst = 0i32;
    let mut at = 0usize;
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        let d = (i32::from(*g) - i32::from(*w)).abs();
        if d > worst {
            worst = d;
            at = i;
        }
    }
    println!("MEASURED {label}: worst difference {worst} of 255 (at byte {at})");
    assert!(
        worst <= 1,
        "{label}: the shader and the CPU reference disagree by {worst} code values at byte {at} \
         — a faster path that changes the picture is not the same path"
    );
}

#[test]
fn the_shader_matches_the_cpu_reference_for_every_matrix_and_range() {
    let gpu = need!(gpu(), "GPU adapter");
    println!("adapter: {}", gpu.adapter);
    let (y, u, v) = planes();
    let cw = (W / 2) as usize;

    for range in [Range::Limited, Range::Full] {
        for code in [1u8, 5, 9] {
            let choice = MatrixChoice::resolve(Some(code), H);
            let conv = conversion(range, choice);
            let got = render(
                &gpu,
                &conv,
                [(&y[..], W as usize), (&u[..], cw), (&v[..], cw)],
            );
            let want = reference(&conv, &y, &u, &v);
            compare(&format!("matrix H.273 {code}, {range:?}"), &got, &want);
        }
    }
}

/// A frame whose rows are padded, as real decoders produce. The padding must be
/// stripped, not carried into the texture.
#[test]
fn a_padded_plane_stride_is_repacked_correctly() {
    let gpu = need!(gpu(), "GPU adapter");
    let (y, u, v) = planes();
    let stride = W as usize + 37; // deliberately not 256-aligned, nor even
    let pad = |src: &[u8], w: usize, h: usize, stride: usize| -> Vec<u8> {
        let mut out = vec![0xAAu8; stride * h];
        for r in 0..h {
            out[r * stride..r * stride + w].copy_from_slice(&src[r * w..(r + 1) * w]);
        }
        out
    };
    let (cw, ch) = ((W / 2) as usize, (H / 2) as usize);
    let cstride = cw + 13;
    let yp = pad(&y, W as usize, H as usize, stride);
    let up = pad(&u, cw, ch, cstride);
    let vp = pad(&v, cw, ch, cstride);

    let conv = conversion(Range::Limited, MatrixChoice::resolve(Some(1), H));
    let got = render(
        &gpu,
        &conv,
        [(&yp[..], stride), (&up[..], cstride), (&vp[..], cstride)],
    );
    let want = reference(&conv, &y, &u, &v);
    compare("padded strides", &got, &want);
}

/// An assumed matrix must convert exactly like a declared one — the assumption
/// is about *which* matrix, never about the arithmetic.
#[test]
fn an_assumed_matrix_converts_by_the_same_arithmetic() {
    let gpu = need!(gpu(), "GPU adapter");
    let (y, u, v) = planes();
    let cw = (W / 2) as usize;
    let conv = conversion(Range::Limited, MatrixChoice::resolve(None, H));
    assert!(conv.choice.is_assumed());
    let got = render(
        &gpu,
        &conv,
        [(&y[..], W as usize), (&u[..], cw), (&v[..], cw)],
    );
    let want = reference(&conv, &y, &u, &v);
    compare("assumed matrix", &got, &want);
}

/// The view transform on the GPU, against the same lattice on the CPU.
///
/// This is the whole claim of a colour-managed viewer: what the window shows
/// is the project's colour transform, not an approximation of it that only the
/// shader knows about. The lattice is the one the runtime bakes — the released
/// ACES 2.0 config, video seen through the ACES SDR view — and the reference is
/// the same lattice interpolated on the CPU, the way `Lut3D::worst_error`
/// measures it against the exact transform.
#[test]
fn the_view_transform_on_the_gpu_is_the_one_the_project_compiled() {
    let gpu = need!(gpu(), "GPU adapter");
    println!("adapter: {}", gpu.adapter);
    let config = cine_color::ocio::Config::parse(cine_color::ucg::BUNDLED_CONFIG).unwrap();
    let processor = config
        .display_processor(
            "Rec.1886 Rec.709 - Display",
            "sRGB - Display",
            "ACES 2.0 - SDR 100 nits (Rec.709)",
        )
        .expect("the view the window shows video through");
    let mut lut = Lut3D::bake_domain(&processor, 33, [-0.125; 3], [1.125; 3]);
    lut.interpolation = Interpolation::Trilinear;
    println!(
        "MEASURED lattice 33³ over [-0.125, 1.125]: worst {:.5} of full scale against the \
         transform itself",
        lut.worst_error(&processor, 4096)
    );
    let lut = std::sync::Arc::new(lut);

    let (y, u, v) = planes();
    let conv = conversion(Range::Limited, MatrixChoice::resolve(Some(1), H));
    let planes = [
        (y.as_slice(), W as usize),
        (u.as_slice(), (W / 2) as usize),
        (v.as_slice(), (W / 2) as usize),
    ];
    let got = render_through(&gpu, &conv, planes, Some(&lut));

    // The reference: the same conversion, through the same lattice, on the CPU.
    let norm = |p: &[u8]| -> Vec<f32> { p.iter().map(|&b| f32::from(b) / 255.0).collect() };
    let rgba = yuv_to_rgb_cpu(&conv, &norm(&y), &norm(&u), &norm(&v));
    let want: Vec<u8> = rgba
        .chunks(4)
        .flat_map(|px| {
            let out = lut.apply([px[0], px[1], px[2]]);
            [out[0], out[1], out[2], px[3]]
        })
        .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    compare("view transform", &got, &want);

    // And it is really doing something: without it the picture is different.
    let plain = render(&gpu, &conv, planes);
    let changed = got
        .iter()
        .zip(&plain)
        .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
        .max()
        .unwrap_or(0);
    println!("MEASURED the view changes the picture by up to {changed} of 255");
    assert!(
        changed > 8,
        "the view transform changed the picture by only {changed} code values — a viewer that \
         claims a transform and shows the decoded picture is worse than one that claims nothing"
    );
}
