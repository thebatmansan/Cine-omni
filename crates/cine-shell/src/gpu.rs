//! Window surface and swapchain, on the same device as the compute kernels.
//!
//! The adapter is requested with `compatible_surface: Some(&surface)`, which
//! matters: on a multi-GPU machine the best *compute* adapter may not be the
//! one driving the display, and picking it would mean every presented frame
//! crosses the PCIe bus. The headless [`cine_gpu::device::GpuContext`] passes
//! `None` because it never presents.

use crate::{Result, ShellError};
use std::sync::Arc;

/// A window surface plus the device that draws to it.
pub struct SurfaceContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

impl SurfaceContext {
    /// Create a device compatible with `target`'s surface.
    ///
    /// `target` must outlive the returned context; callers hold the window in
    /// an `Arc` and pass a clone, which is what makes the `'static` surface
    /// sound.
    pub fn new<T>(target: T, width: u32, height: u32) -> Result<SurfaceContext>
    where
        T: Into<wgpu::SurfaceTarget<'static>>,
    {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance
            .create_surface(target)
            .map_err(|e| ShellError::Surface(e.to_string()))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            // The display adapter, not merely the fastest one.
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|_| ShellError::NoAdapter)?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("cine-omni shell"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| ShellError::Gpu(e.to_string()))?;

        let caps = surface.get_capabilities(&adapter);
        let format = choose_format(&caps);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            // Fifo is the only present mode guaranteed to exist, and it is the
            // right default for a viewer: tearing in a reference monitor is not
            // a trade anyone wants.
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            // SDR for now. HDR presentation needs a wide-gamut colour space
            // here *and* a matching encode in the viewer; declaring one without
            // the other would change how output must be encoded and silently
            // misrepresent every frame. `Auto` never resolves to HDR, which is
            // the conservative behaviour we want until the pair is built.
            color_space: wgpu::SurfaceColorSpace::Auto,
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        Ok(SurfaceContext {
            instance,
            adapter,
            device: Arc::new(device),
            queue: Arc::new(queue),
            surface,
            config,
        })
    }

    /// A compute context on **this** device.
    ///
    /// One device for compute and presentation, which is the point: a kernel's
    /// result is already on the device that will draw it. The window's adapter
    /// is the display's, chosen with `compatible_surface`, so compute follows
    /// the display rather than the other way round.
    pub fn compute_context(&self) -> cine_gpu::device::GpuContext {
        cine_gpu::device::GpuContext::adopt(
            self.instance.clone(),
            self.adapter.clone(),
            self.device.clone(),
            self.queue.clone(),
        )
    }

    pub fn adapter_name(&self) -> String {
        let info = self.adapter.get_info();
        format!("{} ({:?})", info.name, info.backend)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }
}

/// Choose a surface format the viewer can write correctly.
///
/// This is a colour decision, not a performance one. `SurfaceColorSpace::Auto`
/// resolves a float format such as `Rgba16Float` to *extended sRGB linear* —
/// the surface then expects linear light. The viewer's YUV→RGB output is
/// gamma-encoded video, so writing it to a linear surface would display it far
/// too bright, with nothing to indicate the error beyond "it looks wrong".
///
/// So an 8-bit unorm format is preferred, which `Auto` resolves to the sRGB
/// colour space, matching the encoded values the viewer actually produces. A
/// non-sRGB *format* is chosen within that so the hardware does not apply a
/// second encode on top.
///
/// Float surfaces become the right answer once the viewer linearises and
/// applies a display transform — that is the HDR path, and it is not built.
fn choose_format(caps: &wgpu::SurfaceCapabilities) -> wgpu::TextureFormat {
    use wgpu::TextureFormat as F;
    const PREFERRED: [F; 2] = [F::Bgra8Unorm, F::Rgba8Unorm];
    for want in PREFERRED {
        if caps.formats.contains(&want) {
            return want;
        }
    }
    // Otherwise take any 8-bit format, sRGB or not, before falling back to a
    // float surface whose colour space we are not yet equipped to feed.
    caps.formats
        .iter()
        .copied()
        .find(|f| matches!(f, F::Bgra8UnormSrgb | F::Rgba8UnormSrgb))
        .unwrap_or(caps.formats[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::TextureFormat as F;

    fn caps(formats: Vec<F>) -> wgpu::SurfaceCapabilities {
        wgpu::SurfaceCapabilities {
            formats,
            present_modes: vec![wgpu::PresentMode::Fifo],
            alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
            ..Default::default()
        }
    }

    /// The defect this guards: preferring "any non-sRGB format" selects
    /// `Rgba16Float`, which `Auto` resolves to a *linear* colour space. The
    /// viewer writes gamma-encoded video, so that surface displays it far too
    /// bright.
    #[test]
    fn a_float_surface_is_not_chosen_over_an_eight_bit_one() {
        let chosen = choose_format(&caps(vec![
            F::Rgba16Float,
            F::Bgra8UnormSrgb,
            F::Bgra8Unorm,
        ]));
        assert_eq!(
            chosen,
            F::Bgra8Unorm,
            "a float surface expects linear light; the viewer produces encoded video"
        );
    }

    #[test]
    fn a_non_srgb_eight_bit_format_is_preferred_over_the_srgb_one() {
        let chosen = choose_format(&caps(vec![F::Bgra8UnormSrgb, F::Bgra8Unorm]));
        assert_eq!(
            chosen,
            F::Bgra8Unorm,
            "an sRGB surface would encode a second time"
        );
    }

    #[test]
    fn an_srgb_only_surface_is_still_preferred_to_a_float_one() {
        let chosen = choose_format(&caps(vec![F::Rgba16Float, F::Bgra8UnormSrgb]));
        assert_eq!(chosen, F::Bgra8UnormSrgb);
    }

    /// With nothing suitable, something must still be returned rather than
    /// panicking — the limitation is documented, not fatal.
    #[test]
    fn a_float_only_surface_still_yields_a_format() {
        assert_eq!(choose_format(&caps(vec![F::Rgba16Float])), F::Rgba16Float);
    }
}
