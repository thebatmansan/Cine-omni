//! Every embedded WGSL kernel must parse and validate with naga (no GPU needed).

use cine_gpu::kernels::{KernelStage, KERNELS};
use naga::valid::{Capabilities, ValidationFlags, Validator};

#[test]
fn all_kernels_validate() {
    for k in KERNELS {
        let src = k.module_source();
        let module = naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| {
            panic!(
                "kernel `{}` failed to parse:\n{}",
                k.name,
                e.emit_to_string(&src)
            )
        });
        let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
        let info = validator
            .validate(&module)
            .unwrap_or_else(|e| panic!("kernel `{}` failed validation: {:?}", k.name, e));
        let _ = info;
        // Entry points exist.
        match k.stage {
            KernelStage::Compute { entry, .. } => {
                assert!(
                    module.entry_points.iter().any(|ep| ep.name == entry),
                    "{}: missing entry {entry}",
                    k.name
                );
            }
            KernelStage::Render { vertex, fragment } => {
                assert!(
                    module.entry_points.iter().any(|ep| ep.name == vertex),
                    "{}: missing {vertex}",
                    k.name
                );
                assert!(
                    module.entry_points.iter().any(|ep| ep.name == fragment),
                    "{}: missing {fragment}",
                    k.name
                );
            }
        }
    }
}

#[test]
fn dispatch_sizes_cover_image() {
    let k = cine_gpu::kernels::kernel(cine_gpu::KernelId::Merge);
    assert_eq!(k.dispatch_2d(1920, 1080), [120, 68, 1]);
    assert_eq!(k.dispatch_2d(17, 17), [2, 2, 1]);
}

#[cfg(feature = "wgpu")]
#[test]
fn gpu_merge_matches_cpu_when_adapter_available() {
    use cine_gpu::cpu;
    use cine_gpu::kernels::uniforms::{as_bytes, MergeParams};
    use cine_gpu::{KernelId, MergeOp};
    let Ok(mut ctx) = cine_gpu::device::GpuContext::new() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (w, h) = (64u32, 32u32);
    let n = (w * h * 4) as usize;
    let a: Vec<f32> = (0..n).map(|i| ((i * 7) % 13) as f32 / 13.0).collect();
    let b: Vec<f32> = (0..n).map(|i| ((i * 5) % 11) as f32 / 11.0).collect();
    let params = MergeParams {
        width: w,
        height: h,
        op: MergeOp::Over as u32,
        mix: 1.0,
    };
    let gpu = ctx
        .run_image_kernel(KernelId::Merge, as_bytes(&params), &[&a, &b], w, h)
        .unwrap();
    let mut cpu_out = vec![0.0f32; n];
    cpu::merge_image(MergeOp::Over, 1.0, &a, &b, &mut cpu_out);
    for i in 0..n {
        assert!(
            (gpu[i] - cpu_out[i]).abs() < 1e-5,
            "mismatch at {i}: {} vs {}",
            gpu[i],
            cpu_out[i]
        );
    }
}

/// The declared binding list must match what the WGSL module actually
/// declares, or pipeline creation fails at runtime on a real device.
#[test]
fn declared_bindings_match_the_shader() {
    use cine_gpu::kernels::Binding;
    use naga::{AddressSpace, StorageAccess};
    for k in KERNELS {
        let src = k.module_source();
        let module = naga::front::wgsl::parse_str(&src).expect("parses");
        let mut declared: Vec<(u32, Binding)> = Vec::new();
        for (_, var) in module.global_variables.iter() {
            let Some(binding) = &var.binding else {
                continue;
            };
            if binding.group != 0 {
                continue;
            }
            let b = match var.space {
                AddressSpace::Uniform => Binding::Uniform,
                AddressSpace::Storage { access } if access.contains(StorageAccess::STORE) => {
                    Binding::Write
                }
                AddressSpace::Storage { .. } => Binding::Read,
                _ => continue,
            };
            declared.push((binding.binding, b));
        }
        declared.sort_by_key(|(i, _)| *i);
        let actual: Vec<Binding> = declared.into_iter().map(|(_, b)| b).collect();
        assert_eq!(
            actual,
            k.bindings.to_vec(),
            "kernel `{}` binding mismatch",
            k.name
        );
    }
}

/// Every pixel kernel with a CPU mirror must agree with it on real hardware.
#[cfg(feature = "wgpu")]
#[test]
fn gpu_kernels_match_the_cpu_reference() {
    use cine_gpu::cpu;
    use cine_gpu::device::GpuContext;
    use cine_gpu::kernels::uniforms::{as_bytes, BlurParams, CdlParams, KeyParams, MergeParams};
    use cine_gpu::{KernelId, MergeOp};

    let Ok(mut ctx) = GpuContext::new() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (w, h) = (64u32, 48u32);
    let n = (w * h * 4) as usize;
    let a: Vec<f32> = (0..n).map(|i| ((i * 7) % 13) as f32 / 13.0).collect();
    let b: Vec<f32> = (0..n).map(|i| ((i * 5) % 11) as f32 / 11.0).collect();

    // Every merge operation.
    for op in MergeOp::ALL {
        let params = MergeParams {
            width: w,
            height: h,
            op: op as u32,
            mix: 1.0,
        };
        let gpu = ctx
            .run_image_kernel(KernelId::Merge, as_bytes(&params), &[&a, &b], w, h)
            .unwrap();
        let mut want = vec![0.0f32; n];
        cpu::merge_image(op, 1.0, &a, &b, &mut want);
        for i in 0..n {
            assert!(
                (gpu[i] - want[i]).abs() < 1e-5,
                "{op:?} differs at {i}: {} vs {}",
                gpu[i],
                want[i]
            );
        }
    }

    // ASC CDL.
    let (slope, offset, power, sat) = (
        [1.2f32, 0.9, 1.05],
        [0.02f32, -0.01, 0.0],
        [0.95f32, 1.1, 1.0],
        0.8f32,
    );
    let params = CdlParams {
        slope: [slope[0], slope[1], slope[2], 1.0],
        offset: [offset[0], offset[1], offset[2], 0.0],
        power: [power[0], power[1], power[2], 1.0],
        saturation: sat,
        width: w,
        height: h,
        _pad: 0,
    };
    let gpu = ctx
        .run_image_kernel(KernelId::Cdl, as_bytes(&params), &[&a], w, h)
        .unwrap();
    for (i, px) in a.as_chunks::<4>().0.iter().enumerate() {
        let want = cpu::cdl([px[0], px[1], px[2]], slope, offset, power, sat);
        for c in 0..3 {
            assert!(
                (gpu[i * 4 + c] - want[c]).abs() < 1e-4,
                "cdl differs at {i}.{c}"
            );
        }
        assert!(
            (gpu[i * 4 + 3] - px[3]).abs() < 1e-6,
            "cdl must not touch alpha"
        );
    }

    // Chroma keyer alpha.
    let key = [0.1f32, 0.8, 0.1];
    let params = KeyParams {
        key_color: [key[0], key[1], key[2], 1.0],
        width: w,
        height: h,
        tolerance: 0.1,
        softness: 0.2,
        spill: 1.0,
        _p0: 0.0,
        _p1: 0.0,
        _p2: 0.0,
    };
    let gpu = ctx
        .run_image_kernel(KernelId::Keyer, as_bytes(&params), &[&a], w, h)
        .unwrap();
    for (i, px) in a.as_chunks::<4>().0.iter().enumerate() {
        let want = cpu::chroma_key_alpha([px[0], px[1], px[2]], key, 0.1, 0.2);
        assert!(
            (gpu[i * 4 + 3] - want).abs() < 1e-4,
            "key alpha differs at {i}"
        );
    }

    // A wide blur of a constant image is that constant (energy preserving).
    let flat = vec![0.5f32; n];
    let params = BlurParams {
        width: w,
        height: h,
        radius: 6,
        horizontal: 1,
        sigma: 2.5,
        ..Default::default()
    };
    let gpu = ctx
        .run_image_kernel(KernelId::Blur, as_bytes(&params), &[&flat], w, h)
        .unwrap();
    for v in &gpu {
        assert!((v - 0.5).abs() < 1e-5, "blur of a constant changed it: {v}");
    }
}
