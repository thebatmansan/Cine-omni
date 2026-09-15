//! Getting a converted frame onto the surface.
//!
//! ## Two paths, and why both exist
//!
//! [`Presenter::set_planes`] is the one playback uses: the decoded planes go to
//! the GPU as three 8-bit textures and the YUV→RGB conversion happens in the
//! fragment shader, where the pixels already are.
//!
//! [`Presenter::set_image`] takes an already-converted RGBA image. It is the
//! older path — `cine-gpu`'s compute kernel writes to a storage buffer, which is
//! read back to the CPU and uploaded again as a 32-bit float texture, roughly
//! 30 MB across the bus per picture to deliver 1.4 MB of planes. Measured at
//! **69.4 ms per picture** against a 41.7 ms frame budget, it was the reason
//! on-screen playback dropped 77% of its pictures while headless playback
//! dropped none.
//!
//! It is kept because it is the path `cine-gpu` verifies against a CPU reference
//! on real hardware, and `--dump` writes what *it* produced. The planar path is
//! checked against the same reference in `tests/planar_present.rs`.

use crate::{Result, ShellError};
use cine_gpu::yuv::YuvConversion;
use wgpu::util::DeviceExt;

/// A picture held on the GPU as its three planes, ready to convert on draw.
struct Planes {
    y: wgpu::Texture,
    u: wgpu::Texture,
    v: wgpu::Texture,
    params: wgpu::Buffer,
    width: u32,
    height: u32,
    chroma_width: u32,
    chroma_height: u32,
}

/// A fullscreen blit of an RGBA image to the surface.
pub struct Presenter {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    texture: Option<(wgpu::Texture, u32, u32)>,
    planar_pipeline: wgpu::RenderPipeline,
    planar_layout: wgpu::BindGroupLayout,
    planes: Option<Planes>,
    /// Reused row-padding buffer. WebGPU requires 256-byte row alignment for a
    /// multi-row write, and real decoders produce whatever stride they like.
    staging: Vec<u8>,
}

impl Presenter {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Presenter {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        // 32-bit float textures are not filterable on most
                        // hardware, and a reference viewer should not silently
                        // interpolate anyway: at anything but 1:1 the correct
                        // answer is a real resampling filter, not bilinear
                        // taps. Nearest until that exists.
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(format.into())],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let planar_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv blit"),
            source: wgpu::ShaderSource::Wgsl(include_str!("yuv_blit.wgsl").into()),
        });
        let plane_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                // Loaded by integer coordinate, never sampled, so filterability
                // is irrelevant and nothing interpolates behind our back.
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let planar_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                plane_entry(1),
                plane_entry(2),
                plane_entry(3),
            ],
        });
        let planar_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("yuv blit"),
                bind_group_layouts: &[Some(&planar_layout)],
                immediate_size: 0,
            });
        let planar_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("yuv blit"),
            layout: Some(&planar_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &planar_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &planar_module,
                entry_point: Some("fs_main"),
                targets: &[Some(format.into())],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Presenter {
            pipeline,
            layout,
            sampler,
            texture: None,
            planar_pipeline,
            planar_layout,
            planes: None,
            staging: Vec::new(),
        }
    }

    /// Upload one decoded picture as its three planes.
    ///
    /// `conversion` says how the planes must be interpreted; it is resolved from
    /// what the file declares by [`crate::viewer::conversion_for`], which also
    /// records whether anything had to be assumed.
    pub fn set_planes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        conversion: &YuvConversion,
        planes: [(&[u8], usize); 3],
    ) -> Result<()> {
        let (w, h) = (conversion.width, conversion.height);
        let (cw, ch) = (conversion.chroma_width, conversion.chroma_height);
        let fits = self.planes.as_ref().is_some_and(|p| {
            p.width == w && p.height == h && p.chroma_width == cw && p.chroma_height == ch
        });
        if !fits {
            let plane = |label: &str, width: u32, height: u32| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            };
            self.planes = Some(Planes {
                y: plane("plane y", w, h),
                u: plane("plane u", cw, ch),
                v: plane("plane v", cw, ch),
                params: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("yuv params"),
                    contents: bytemuck::cast_slice(&conversion.params()),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                }),
                width: w,
                height: h,
                chroma_width: cw,
                chroma_height: ch,
            });
        }
        // Disjoint field borrows: the planes are read while the staging buffer
        // is written.
        let held = self.planes.as_ref().expect("just created");
        let staging = &mut self.staging;
        queue.write_buffer(&held.params, 0, bytemuck::cast_slice(&conversion.params()));

        let sizes = [(w, h), (cw, ch), (cw, ch)];
        for (i, ((data, stride), (pw, ph))) in planes.iter().zip(sizes).enumerate() {
            let texture = match i {
                0 => &held.y,
                1 => &held.u,
                _ => &held.v,
            };
            write_plane(staging, queue, texture, data, *stride, pw, ph)?;
        }
        // One picture at a time: an RGBA image and a set of planes would
        // otherwise both be drawable, and which one appeared would be an
        // accident of ordering.
        self.texture = None;
        Ok(())
    }

    /// Upload an already-converted RGBA f32 image, replacing the previous one.
    ///
    /// The slow path; see the module note. `--dump` uses it so the written PNG
    /// is what `cine-gpu`'s verified kernel produced.
    pub fn set_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba: &[f32],
        width: u32,
        height: u32,
    ) -> Result<()> {
        let expected = width as usize * height as usize * 4;
        if rgba.len() != expected {
            return Err(ShellError::Gpu(format!(
                "image is {} floats, expected {expected} for {width}x{height}",
                rgba.len()
            )));
        }
        let bytes: &[u8] = bytemuck::cast_slice(rgba);
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("frame"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytes,
        );
        self.texture = Some((texture, width, height));
        // Drawing prefers planes, so an image set after them would silently
        // never appear.
        self.planes = None;
        Ok(())
    }

    pub fn has_image(&self) -> bool {
        self.texture.is_some() || self.planes.is_some()
    }

    /// Draw the current image, or clear to the surround colour if there is none.
    pub fn draw(&self, device: &wgpu::Device, queue: &wgpu::Queue, view: &wgpu::TextureView) {
        self.draw_into(device, queue, view, None);
    }

    /// Draw into a rectangle of the target, in physical pixels, letterboxed by
    /// the caller.
    ///
    /// The whole target is still cleared to the surround colour first, so the
    /// area outside the picture is the neutral grey a viewer should have rather
    /// than whatever was there before.
    pub fn draw_into(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        viewport: Option<[f32; 4]>,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("present"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Neutral dark surround, as a viewer should have.
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.055,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some([x, y, w, h]) = viewport {
                if w < 1.0 || h < 1.0 {
                    return;
                }
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
            }
            if let Some(p) = &self.planes {
                let view = |t: &wgpu::Texture| t.create_view(&Default::default());
                let (yv, uv, vv) = (view(&p.y), view(&p.u), view(&p.v));
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("yuv blit"),
                    layout: &self.planar_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: p.params.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&yv),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&uv),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&vv),
                        },
                    ],
                });
                pass.set_pipeline(&self.planar_pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..3, 0..1);
            } else if let Some((texture, _, _)) = &self.texture {
                let tv = texture.create_view(&Default::default());
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("blit"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&tv),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        queue.submit(Some(encoder.finish()));
    }
}

/// Copy one plane into a texture, padding rows to the 256-byte alignment that a
/// multi-row `write_texture` requires. Real decoders produce whatever stride
/// they like, and it is rarely 256-aligned.
fn write_plane(
    staging: &mut Vec<u8>,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    data: &[u8],
    stride: usize,
    width: u32,
    height: u32,
) -> Result<()> {
    let (w, h) = (width as usize, height as usize);
    if data.len() < stride * (h - 1) + w {
        return Err(ShellError::Gpu(format!(
            "plane is {} bytes, too small for {w}x{h} at stride {stride}",
            data.len()
        )));
    }
    let padded = w.div_ceil(256) * 256;
    staging.clear();
    staging.resize(padded * h, 0);
    for row in 0..h {
        let src = &data[row * stride..row * stride + w];
        staging[row * padded..row * padded + w].copy_from_slice(src);
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        staging,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(padded as u32),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    Ok(())
}
