//! egui on the shell's own surface, sharing the device the picture uses.
//!
//! The interface and the picture are drawn into the same swapchain image by the
//! same queue: the frame is put down first, letterboxed into the program
//! viewer's rectangle, and the panels are drawn over it with the program pane
//! left transparent. Nothing round-trips through the CPU and there is no second
//! presentation path — which is the whole reason the workspace is native.

use egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};
use winit::window::Window;

/// egui's context, its winit plumbing and its wgpu renderer.
pub struct EguiLayer {
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: Renderer,
    /// The last tessellated interface, kept so a new *picture* can be shown
    /// without rebuilding the *panels*.
    ///
    /// Running the interface costs real time — measured at ~9.7 ms a frame with
    /// a timeline drawn — and on two cores that competes directly with the
    /// decoder. The picture changes 24 times a second; the panels do not need
    /// rebuilding that often merely because it did.
    last: Option<Vec<egui::ClippedPrimitive>>,
    /// The screen the cached interface was built for. `ScreenDescriptor` is not
    /// `Clone`, and it is two plain numbers, so they are kept directly.
    last_screen: ([u32; 2], f32),
}

impl EguiLayer {
    pub fn new(window: &Window, device: &wgpu::Device, format: wgpu::TextureFormat) -> EguiLayer {
        let ctx = egui::Context::default();
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(2048),
        );
        let renderer = Renderer::new(
            device,
            format,
            RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                ..Default::default()
            },
        );
        EguiLayer {
            ctx,
            state,
            renderer,
            last: None,
            last_screen: ([0, 0], 1.0),
        }
    }

    pub fn context(&self) -> &egui::Context {
        &self.ctx
    }

    /// Feed a window event to egui. Returns whether egui consumed it, so the
    /// application does not also act on a keystroke meant for a text field.
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    /// Build the interface for this frame and draw it over `view`.
    ///
    /// The picture must already be on `view`: this loads rather than clears.
    ///
    /// Returns **when egui next needs painting**. `Duration::MAX` means "not
    /// until something happens", which is the common case for a still
    /// interface. Using this rather than "does it want a repaint at all" is the
    /// difference between redrawing 24 times a second and redrawing 74 — and on
    /// a two-core machine the difference between playback holding rate and
    /// dropping 38% of its pictures to the interface.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        window: &Window,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size_in_pixels: [u32; 2],
        build: impl FnMut(&mut egui::Ui),
    ) -> std::time::Duration {
        let input = self.state.take_egui_input(window);
        let mut output = self.ctx.run_ui(input, build);
        self.state
            .handle_platform_output(window, output.platform_output);

        let pixels_per_point = self.ctx.pixels_per_point();
        let jobs = self.ctx.tessellate(output.shapes, pixels_per_point);
        for (id, deltas) in &output.textures_delta.set {
            // 0.36 batches several deltas per texture: a font atlas grows in
            // pieces, and applying only the first would leave glyphs missing.
            for delta in deltas {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }
        let descriptor = ScreenDescriptor {
            size_in_pixels,
            pixels_per_point,
        };
        let buffers = self
            .renderer
            .update_buffers(device, queue, encoder, &jobs, &descriptor);
        self.last = Some(jobs.clone());
        self.last_screen = (size_in_pixels, pixels_per_point);
        if !buffers.is_empty() {
            queue.submit(buffers);
        }
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Load: the picture is already there.
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), &jobs, &descriptor);
        }
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        // Every delta above has been applied. epaint 0.36 checks that on drop
        // in debug builds and panics otherwise — which release builds never
        // saw, and a debug `--ui-smoke` did.
        output.textures_delta.clear();
        output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .unwrap_or(std::time::Duration::MAX)
    }

    /// Draw the interface exactly as it was last built, without running it.
    ///
    /// Used when only the picture changed. Returns whether there was anything
    /// to draw.
    pub fn redraw_last(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) -> bool {
        let Some(jobs) = self.last.clone() else {
            return false;
        };
        let descriptor = ScreenDescriptor {
            size_in_pixels: self.last_screen.0,
            pixels_per_point: self.last_screen.1,
        };
        let buffers = self
            .renderer
            .update_buffers(device, queue, encoder, &jobs, &descriptor);
        if !buffers.is_empty() {
            queue.submit(buffers);
        }
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui (cached)"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.renderer
            .render(&mut pass.forget_lifetime(), &jobs, &descriptor);
        true
    }
}
