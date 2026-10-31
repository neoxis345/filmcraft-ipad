//! Drawing egui with wgpu on the `CAMetalLayer` of the Swift view.
//!
//! The same paint path as eframe's (`egui_wgpu::winit::Painter`), without winit: the host owns the
//! layer and tells us its size. The device asks the adapter for its real limits instead of wgpu's
//! defaults (an iPad's GPU differs from a desktop's), and the `RenderState` handed to FilmCraft is
//! the one its GPU compositor and monitors draw with.

use eframe::egui_wgpu::{self, RenderState, Renderer, RendererOptions, ScreenDescriptor, WgpuConfiguration, WgpuSetup, WgpuSetupExisting};
use eframe::wgpu;

/// The surface and the egui renderer.
pub struct Painter {
    pub render_state: RenderState,
    surface: wgpu::Surface<'static>,
    size: [u32; 2],
    needs_configure: bool,
}

/// The device every iPad GPU this app supports can give: wgpu's defaults, with the texture size the
/// adapter really supports (4K frames and the 2360 × 1640 screen fit easily) and nothing the adapter
/// lacks.
fn limits_for(adapter: &wgpu::Adapter) -> wgpu::Limits {
    let real = adapter.limits();
    let mut l = wgpu::Limits::default().or_worse_values_from(&real);
    l.max_texture_dimension_2d = real.max_texture_dimension_2d.min(16_384);
    l.max_texture_dimension_1d = real.max_texture_dimension_1d.min(16_384);
    l
}

impl Painter {
    /// A painter for the `CAMetalLayer` at `layer`.
    ///
    /// # Safety
    /// `layer` must point to a live `CAMetalLayer` that outlives the painter (the host keeps the view
    /// for the life of the scene and drops the painter before the view).
    #[cfg(target_vendor = "apple")]
    #[allow(unsafe_code)]
    pub unsafe fn for_layer(layer: *mut std::ffi::c_void, size: [u32; 2]) -> Result<Self, String> {
        if layer.is_null() {
            return Err("no Metal layer".into());
        }
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::METAL;
        let instance = wgpu::Instance::new(desc);
        // SAFETY: the caller guarantees `layer` is a live CAMetalLayer outliving the surface.
        let surface =
            unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer)) }.map_err(|e| format!("Metal surface: {e}"))?;
        Self::with_surface(instance, surface, size)
    }

    /// A painter for a surface the instance already made.
    pub fn with_surface(instance: wgpu::Instance, surface: wgpu::Surface<'static>, size: [u32; 2]) -> Result<Self, String> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
        let info = adapter.get_info();
        log::info!("GPU: {} ({:?})", info.name, info.backend);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("filmcraft-ios"),
            required_limits: limits_for(&adapter),
            // a 4 GB iPad shares its memory with the GPU: favour footprint over allocation speed
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        }))
        .map_err(|e| format!("GPU device: {e}"))?;
        let config = WgpuConfiguration {
            wgpu_setup: WgpuSetup::Existing(WgpuSetupExisting { instance: instance.clone(), adapter, device, queue }),
            surface: egui_wgpu::SurfaceConfig::LOW_LATENCY,
            ..Default::default()
        };
        let render_state = pollster::block_on(RenderState::create(&config, &instance, Some(&surface), RendererOptions::default()))
            .map_err(|e| format!("egui renderer: {e}"))?;
        let mut p = Self { render_state, surface, size: [size[0].max(1), size[1].max(1)], needs_configure: true };
        p.configure();
        Ok(p)
    }

    pub fn max_texture_side(&self) -> usize {
        self.render_state.device.limits().max_texture_dimension_2d as usize
    }

    /// The layer's drawable size changed (rotation, Split View, Stage Manager).
    pub fn resize(&mut self, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        if size != self.size {
            self.size = size;
            self.needs_configure = true;
        }
    }

    fn configure(&mut self) {
        let rs = &self.render_state;
        let caps = self.surface.get_capabilities(&rs.adapter);
        let alpha_mode = caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Opaque);
        let Some(mut config) = self.surface.get_default_config(&rs.adapter, self.size[0], self.size[1]) else {
            log::error!("the surface is not supported by the GPU");
            return;
        };
        config.format = rs.target_format;
        config.view_formats = vec![rs.target_format];
        config.present_mode = rs.surface_config.present_mode;
        config.alpha_mode = alpha_mode;
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if let Some(n) = rs.surface_config.desired_maximum_frame_latency {
            config.desired_maximum_frame_latency = n;
        }
        self.surface.configure(&rs.device, &config);
        self.needs_configure = false;
    }

    /// Upload the textures and draw one frame.
    pub fn paint(&mut self, pixels_per_point: f32, clear: [f32; 4], primitives: &[egui::ClippedPrimitive], textures: &mut egui::TexturesDelta) {
        if self.needs_configure {
            self.configure();
        }
        let rs = &self.render_state;
        let mut encoder = rs.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("egui") });
        let screen = ScreenDescriptor { size_in_pixels: self.size, pixels_per_point };
        let user_buffers = {
            let mut renderer = rs.renderer.write();
            for (id, deltas) in std::mem::take(&mut textures.set) {
                for delta in deltas {
                    renderer.update_texture(&rs.device, &rs.queue, id, &delta);
                }
            }
            renderer.update_buffers(&rs.device, &rs.queue, &mut encoder, primitives, &screen)
        };
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                self.needs_configure = true;
                f
            }
            other => {
                // a size change, a lost drawable or a timeout: configure again and skip this frame
                log::debug!(
                    "surface: {}",
                    match other {
                        wgpu::CurrentSurfaceTexture::Timeout => "timeout",
                        wgpu::CurrentSurfaceTexture::Occluded => "occluded",
                        wgpu::CurrentSurfaceTexture::Outdated => "outdated",
                        _ => "lost",
                    }
                );
                self.needs_configure = true;
                rs.queue.submit(user_buffers);
                Self::free(rs.renderer.write(), textures);
                return;
            }
        };
        {
            let renderer = rs.renderer.read();
            let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(clear[0]),
                            g: f64::from(clear[1]),
                            b: f64::from(clear[2]),
                            a: f64::from(clear[3]),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            renderer.render(&mut pass.forget_lifetime(), primitives, &screen);
        }
        rs.queue.submit(user_buffers.into_iter().chain([encoder.finish()]));
        // textures marked for freeing go after the submit: this frame may still use them
        Self::free(rs.renderer.write(), textures);
        rs.queue.present(frame);
    }

    fn free(mut renderer: impl std::ops::DerefMut<Target = Renderer>, textures: &mut egui::TexturesDelta) {
        for id in std::mem::take(&mut textures.free) {
            renderer.free_texture(&id);
        }
    }
}
