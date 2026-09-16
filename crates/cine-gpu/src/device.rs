//! wgpu backend (feature `wgpu`): device creation, kernel compilation, and
//! buffer-based compute dispatch with synchronous readback.

use crate::kernels::{kernel, Binding, Kernel, KernelId, KernelStage};
use crate::{GpuError, Result};
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;

pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pipelines: HashMap<KernelId, ComputeKernel>,
}

pub struct ComputeKernel {
    pub kernel: &'static Kernel,
    pub module: wgpu::ShaderModule,
    pub layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::ComputePipeline,
}

/// One buffer argument of a dispatch: what to upload, or what to read back.
pub enum Arg<'a> {
    Uniform(&'a [u8]),
    Storage(&'a [f32]),
    StorageU32(&'a [u32]),
    /// Output storage buffer of `n` floats, read back after the dispatch.
    Output(usize),
    /// Output storage buffer of `n` u32 (atomics), zero-initialised.
    OutputU32(usize),
}

impl GpuContext {
    /// Create a context on the best available adapter (headless).
    pub fn new() -> Result<Self> {
        // Headless: no compositor connection. A windowed context builds its
        // descriptor from the platform display handle instead, which GLES on
        // Wayland requires in order to present at all.
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
        .map_err(|_| GpuError::NoAdapter)?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("cine-omni"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| GpuError::Device(e.to_string()))?;
        Ok(GpuContext {
            instance,
            adapter,
            device: Arc::new(device),
            queue: Arc::new(queue),
            pipelines: HashMap::new(),
        })
    }

    /// Adopt a device someone else created — the one drawing the window.
    ///
    /// A compositing kernel and the surface it will be presented on must run
    /// on the same device, or every result crosses the bus to get there. The
    /// window owns its device because the adapter has to match the display;
    /// this is how the compute side joins it instead of opening a second one.
    pub fn adopt(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Self {
        GpuContext {
            instance,
            adapter,
            device,
            queue,
            pipelines: HashMap::new(),
        }
    }

    pub fn adapter_name(&self) -> String {
        let info = self.adapter.get_info();
        format!("{} ({:?})", info.name, info.backend)
    }

    /// Compile (and cache) a compute kernel.
    pub fn compute_kernel(&mut self, id: KernelId) -> Result<&ComputeKernel> {
        if !self.pipelines.contains_key(&id) {
            let k = kernel(id);
            let entry = match k.stage {
                KernelStage::Compute { entry, .. } => entry,
                KernelStage::Render { .. } => {
                    return Err(GpuError::Shader {
                        name: k.name.into(),
                        message: "not a compute kernel".into(),
                    })
                }
            };
            let module = self
                .device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(k.name),
                    source: wgpu::ShaderSource::Wgsl(k.module_source().into()),
                });
            // The layout must mirror the WGSL declarations exactly: a
            // `var<storage, read>` global cannot be bound as read_write.
            let entries: Vec<wgpu::BindGroupLayoutEntry> = k
                .bindings
                .iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupLayoutEntry {
                    binding: i as u32,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: match b {
                            Binding::Uniform => wgpu::BufferBindingType::Uniform,
                            Binding::Read => wgpu::BufferBindingType::Storage { read_only: true },
                            Binding::Write => wgpu::BufferBindingType::Storage { read_only: false },
                        },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                })
                .collect();
            let layout = self
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some(k.name),
                    entries: &entries,
                });
            let pipeline_layout =
                self.device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some(k.name),
                        bind_group_layouts: &[Some(&layout)],
                        immediate_size: 0,
                    });
            let pipeline = self
                .device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(k.name),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                });
            self.pipelines.insert(
                id,
                ComputeKernel {
                    kernel: k,
                    module,
                    layout,
                    pipeline,
                },
            );
        }
        Ok(&self.pipelines[&id])
    }

    /// Run a compute kernel once and read back every `Output` binding (in order).
    pub fn dispatch(
        &mut self,
        id: KernelId,
        bindings: &[Arg<'_>],
        workgroups: [u32; 3],
    ) -> Result<Vec<Vec<u8>>> {
        self.compute_kernel(id)?;
        let ck = &self.pipelines[&id];
        let mut buffers: Vec<wgpu::Buffer> = Vec::new();
        let mut outputs: Vec<(usize, u64)> = Vec::new();
        for (i, b) in bindings.iter().enumerate() {
            let buf = match b {
                Arg::Uniform(bytes) => {
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: None,
                            contents: bytes,
                            usage: wgpu::BufferUsages::UNIFORM,
                        })
                }
                Arg::Storage(data) => {
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: None,
                            contents: bytemuck::cast_slice(data),
                            usage: wgpu::BufferUsages::STORAGE,
                        })
                }
                Arg::StorageU32(data) => {
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: None,
                            contents: bytemuck::cast_slice(data),
                            usage: wgpu::BufferUsages::STORAGE,
                        })
                }
                Arg::Output(n) | Arg::OutputU32(n) => {
                    let size = (*n as u64) * 4;
                    outputs.push((i, size));
                    self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: None,
                        size,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                        mapped_at_creation: false,
                    })
                }
            };
            buffers.push(buf);
        }
        let entries: Vec<wgpu::BindGroupEntry> = buffers
            .iter()
            .enumerate()
            .map(|(i, b)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: b.as_entire_binding(),
            })
            .collect();
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &ck.layout,
            entries: &entries,
        });
        let staging: Vec<wgpu::Buffer> = outputs
            .iter()
            .map(|(_, size)| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: *size,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&ck.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(workgroups[0], workgroups[1], workgroups[2]);
        }
        for ((bi, size), st) in outputs.iter().zip(staging.iter()) {
            encoder.copy_buffer_to_buffer(&buffers[*bi], 0, st, 0, *size);
        }
        self.queue.submit(Some(encoder.finish()));
        let mut results = Vec::new();
        for st in &staging {
            let slice = st.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|e| GpuError::Device(e.to_string()))?;
            rx.recv()
                .map_err(|e| GpuError::Device(e.to_string()))?
                .map_err(|e| GpuError::Device(e.to_string()))?;
            let data = slice
                .get_mapped_range()
                .map_err(|e| GpuError::Device(e.to_string()))?
                .to_vec();
            st.unmap();
            results.push(data);
        }
        Ok(results)
    }

    /// Convenience: run an image kernel with `[uniform, in..., out]` bindings on
    /// a `width × height` RGBA f32 image and return the output floats.
    pub fn run_image_kernel(
        &mut self,
        id: KernelId,
        uniform: &[u8],
        inputs: &[&[f32]],
        width: u32,
        height: u32,
    ) -> Result<Vec<f32>> {
        let n = (width * height * 4) as usize;
        let mut bindings: Vec<Arg<'_>> = vec![Arg::Uniform(uniform)];
        for i in inputs {
            if i.len() != n {
                return Err(GpuError::Size {
                    expected: n,
                    actual: i.len(),
                });
            }
            bindings.push(Arg::Storage(i));
        }
        bindings.push(Arg::Output(n));
        let wg = kernel(id).dispatch_2d(width, height);
        let out = self.dispatch(id, &bindings, wg)?;
        Ok(bytemuck::cast_slice(&out[0]).to_vec())
    }
}
