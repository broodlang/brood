//! Bringing a `GpuWindow` up: the surface, adapter and device, the swapchain's format and
//! present mode, the offscreen target the headless harness draws into, and the assembly of
//! the quad pipelines, buffers and caches over a ready device.

use super::*;

/// Pick the swapchain format. A **non-sRGB** 8-bit format is preferred so the shader's
/// raw colour bytes land on screen as given — the same bytes the CPU painter writes —
/// rather than being re-encoded as if they were linear light. Every desktop backend
/// offers one; the first supported format is the fallback when none does.
pub(super) fn pick_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    formats
        .iter()
        .copied()
        .find(|f| {
            matches!(
                f,
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
            )
        })
        .or_else(|| formats.first().copied())
}

/// The present mode for the window's `:vsync` option (ADR-440):
///
/// - `None` (the default) — MAILBOX where the platform has it, else FIFO. Mailbox is the
///   low-latency mode that never tears: a frame replaces the one waiting for the next
///   refresh rather than queueing behind it, so what reaches the screen is the newest.
///   An editor wants exactly that; a torn frame is a visible defect in text.
/// - `Some(true)` — FIFO, which is vsync: every frame is shown, in step with the refresh.
/// - `Some(false)` — the work-bound rate, tearing allowed: IMMEDIATE, then MAILBOX, then
///   FIFO (the only mode some compositors offer, so then the choice is theirs).
pub(super) fn pick_present_mode(
    modes: &[wgpu::PresentMode],
    vsync: Option<bool>,
) -> wgpu::PresentMode {
    use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
    let preference: &[wgpu::PresentMode] = match vsync {
        Some(true) => &[Fifo],
        Some(false) => &[Immediate, Mailbox],
        None => &[Mailbox],
    };
    preference
        .iter()
        .copied()
        .find(|mode| modes.contains(mode))
        .unwrap_or(Fifo)
}

impl GpuWindow {
    pub fn new(window: Rc<Window>, vsync: Option<bool>) -> Result<GpuWindow, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let raw_display_handle = window
            .display_handle()
            .map_err(|e| format!("display handle: {e}"))?
            .as_raw();
        let raw_window_handle = window
            .window_handle()
            .map_err(|e| format!("window handle: {e}"))?
            .as_raw();
        // SAFETY: the handles stay valid for as long as `window` lives, and `GpuWindow`
        // holds the `Rc<Window>` and declares `surface` before it, so the surface is
        // dropped first.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(raw_display_handle),
                raw_window_handle,
            })
        }
        .map_err(|e| format!("gpu surface: {e}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
            compatible_surface: Some(&surface),
        }))
        .map_err(|e| format!("gpu adapter: {e}"))?;
        // A machine with no GPU still offers an adapter when Mesa is installed: llvmpipe or
        // lavapipe, the GPU emulated on the CPU. Drawing through it costs more than the CPU
        // painter, which knows what changed, so it counts as no GPU — the caller falls back
        // with nothing to configure. `BROOD_GUI_GPU=1` still asks for it (the GPU-only
        // effects, a `BROOD_GUI_DUMP` readback on a machine without a GPU).
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu
            && std::env::var("BROOD_GUI_GPU").as_deref() != Ok("1")
        {
            return Err(format!("only a software adapter ({})", info.name));
        }
        let (device, queue) = request_device(&adapter)?;
        // The driver is loaded now: shut the GUI thread's gate before its exit handler runs.
        crate::host::gui::backend::close_at_exit();

        let caps = surface.get_capabilities(&adapter);
        let format = pick_format(&caps.formats).ok_or("gpu surface: no supported format")?;
        // The sRGB view of the same swapchain image the translucent geometry is drawn
        // through (linear-light blending, as the CPU painter's `blend`), when the
        // platform lets a surface be viewed in a second format. Without it every quad
        // blends in the encoded space, as before; text always does (ADR-358).
        let linear_format = linear_view_format(
            format,
            adapter
                .get_downlevel_capabilities()
                .flags
                .contains(wgpu::DownlevelFlags::SURFACE_VIEW_FORMATS),
        );
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | if dump_path().is_some() {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: w,
            height: h,
            present_mode: pick_present_mode(&caps.present_modes, vsync),
            // One frame in flight: the CPU never runs more than a frame ahead of the
            // display, so a keystroke's frame is the next one shown, not the one after.
            // An editor's frames are cheap; the throughput a deeper queue buys a game is
            // latency here (ADR-440).
            desired_maximum_frame_latency: 1,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: linear_format.into_iter().collect(),
        };
        surface.configure(&device, &config);
        Ok(GpuWindow::assemble(
            Target::Window { surface, window },
            device,
            queue,
            config,
            linear_format,
        ))
    }

    /// A target with no window: the frames are drawn into one `w`×`h` texture of the
    /// format a window on this platform is drawn in, its sRGB sibling viewable as the
    /// window's swapchain is where the surface allows it — the same pipelines, batches and
    /// passes, read back with `read_pixels`. What the headless harness checks the GPU path
    /// with on a real device, on nobody's screen.
    #[cfg(test)]
    pub(crate) fn offscreen(adapter: &wgpu::Adapter, w: u32, h: u32) -> Result<GpuWindow, String> {
        let (device, queue) = request_device(adapter)?;
        let format = wgpu::TextureFormat::Bgra8Unorm;
        let linear_format = linear_view_format(format, true);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: w.max(1),
            height: h.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 1,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: linear_format.into_iter().collect(),
        };
        let texture = offscreen_texture(&device, &config);
        Ok(GpuWindow::assemble(
            Target::Offscreen { texture },
            device,
            queue,
            config,
            linear_format,
        ))
    }

    /// What this window does less well than asked, as `(what, reason)` for
    /// `report_degraded` (ADR-636): text drawn grey because the driver rejected subpixel
    /// text, and frames held to vsync when `:vsync false` asked for the work-bound rate and
    /// the compositor offers only FIFO.
    pub fn degradations(&self, vsync: Option<bool>) -> Vec<(&'static str, String)> {
        let mut found = Vec::new();
        if self.lcd.is_none() {
            found.push((
                "lcd-text",
                "the GPU driver rejected subpixel text; text is drawn grey".to_string(),
            ));
        }
        if vsync == Some(false) && self.config.present_mode == wgpu::PresentMode::Fifo {
            found.push((
                "vsync",
                "the display offers only FIFO presentation; frames wait for vsync".to_string(),
            ));
        }
        found
    }

    /// The pipelines, buffers and caches over a device whose target is ready.
    pub(super) fn assemble(
        target: Target,
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: wgpu::SurfaceConfiguration,
        linear_format: Option<wgpu::TextureFormat>,
    ) -> GpuWindow {
        let format = config.format;
        let (w, h) = (config.width, config.height);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brood gui quads"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });
        let viewport_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let viewport_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let viewport_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport"),
            layout: &viewport_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // Nearest: a glyph is drawn at the size it was rasterised, pixel for pixel, and
        // a sprite scaled up stays crisp (pixel art) rather than blurring.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        };

        let solid_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("solid"),
            bind_group_layouts: &[Some(&viewport_layout)],
            immediate_size: 0,
        });
        // One solid pipeline per target format: the encoded one every frame draws with,
        // and (when the surface has an sRGB view) the linear-light one translucent
        // geometry is drawn with — the same vertex stage, its own fragment entry.
        let solid_quads = |label: &str, target_format: wgpu::TextureFormat, fragment: &str| {
            let target = [Some(wgpu::ColorTargetState {
                format: target_format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&solid_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_solid"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: (SOLID_FLOATS * 4) as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x4, 1 => Float32x4, 2 => Float32, 3 => Float32,
                            4 => Float32, 5 => Float32x4
                        ],
                    })],
                },
                primitive,
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &target,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let solid_pipeline = solid_quads("solid quads", format, "fs_solid");
        let solid_linear_pipeline = linear_format
            .map(|linear| solid_quads("solid quads, linear light", linear, "fs_solid_linear"));

        let tex_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("textured"),
            bind_group_layouts: &[Some(&viewport_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        // The textured quads: one vertex layout, drawn three ways — `fs_tex` (colour, grey
        // text) under alpha blending, `fs_tex_lcd` (subpixel text) under `LCD_BLEND`, or
        // subpixel text one channel per pass (`fs_tex_lcd_red` …, writing only that channel).
        let textured_quads = |target_format: wgpu::TextureFormat,
                              label: &str,
                              module: &wgpu::ShaderModule,
                              fragment: &str,
                              blend: wgpu::BlendState,
                              write_mask: wgpu::ColorWrites| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&tex_pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_tex"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: (TEX_WORDS * 4) as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Uint32,
                            4 => Float32, 5 => Float32x4, 6 => Float32
                        ],
                    })],
                },
                primitive,
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(blend),
                        write_mask,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // Text into a target format: `fs_tex{suffix}` for colour and grey glyphs, and the
        // subpixel pipelines — one dual-source pass where the device has it, else three
        // passes, one per colour channel. Built once for the encoded target and once (suffix
        // `_linear`, ADR-611) for its sRGB view.
        let lcd_module = device
            .features()
            .contains(wgpu::Features::DUAL_SOURCE_BLENDING)
            .then(|| {
                device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("lcd text"),
                    source: wgpu::ShaderSource::Wgsl(lcd_shader_source().into()),
                })
            });
        let text_pipelines = |target_format: wgpu::TextureFormat, suffix: &str| {
            let tex = textured_quads(
                target_format,
                "textured quads",
                &shader,
                &format!("fs_tex{suffix}"),
                wgpu::BlendState::ALPHA_BLENDING,
                wgpu::ColorWrites::ALL,
            );
            // Built inside a validation scope: a driver that rejects either costs subpixel
            // text, not the window.
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let lcd = if let Some(module) = &lcd_module {
                LcdText::DualSource(textured_quads(
                    target_format,
                    "lcd text",
                    module,
                    &format!("fs_tex_lcd{suffix}"),
                    LCD_BLEND,
                    wgpu::ColorWrites::ALL,
                ))
            } else {
                let channel = |colour: &str, mask: wgpu::ColorWrites| {
                    textured_quads(
                        target_format,
                        "lcd text, one channel",
                        &shader,
                        &format!("fs_tex_lcd_{colour}{suffix}"),
                        wgpu::BlendState::ALPHA_BLENDING,
                        mask,
                    )
                };
                LcdText::PerChannel([
                    channel("red", wgpu::ColorWrites::RED),
                    channel("green", wgpu::ColorWrites::GREEN),
                    channel("blue", wgpu::ColorWrites::BLUE),
                ])
            };
            let lcd = match pollster::block_on(scope.pop()) {
                None => Some(lcd),
                Some(error) => {
                    eprintln!("brood gui: subpixel text unavailable, drawing grey: {error}");
                    None
                }
            };
            (tex, lcd)
        };
        let (tex_pipeline, lcd) = text_pipelines(format, "");
        let (tex_linear_pipeline, lcd_linear) = match linear_format {
            Some(linear) => {
                let (tex, lcd) = text_pipelines(linear, "_linear");
                (Some(tex), lcd)
            }
            None => (None, None),
        };

        let solid_instances = InstanceBuffer::new(&device, "solid instances", 64 * 1024);
        let tex_instances = InstanceBuffer::new(&device, "textured instances", 64 * 1024);

        GpuWindow {
            target,
            device,
            queue,
            config,
            configured: (w, h),
            viewport_buffer,
            viewport_bind_group,
            solid_pipeline,
            solid_linear_pipeline,
            linear_format,
            solid_instances,
            tex_pipeline,
            lcd,
            tex_linear_pipeline,
            lcd_linear,
            tex_layout,
            tex_instances,
            sampler,
            atlas: Vec::new(),
            glyphs: HashMap::new(),
            textures: HashMap::new(),
            viewport_layout,
            effects: None,
            particles: None,
            runs: RunCache::default(),
            fragments: FragmentCache::default(),
            texture_generation: 0,
            reuse: reuse_enabled(),
            replay_fragments: reuse_enabled(),
            #[cfg(test)]
            last_fragments: (0, 0),
        }
    }
}

/// Every hardware adapter on this machine — the software ones (llvmpipe, lavapipe) left
/// out, as `GpuWindow::new` leaves them out. Empty on a machine without a GPU (CI).
#[cfg(test)]
pub(crate) fn hardware_adapters() -> Vec<wgpu::Adapter> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
        .into_iter()
        .filter(|adapter| adapter.get_info().device_type != wgpu::DeviceType::Cpu)
        .collect()
}

/// The device and queue on `adapter`: dual-source blending where it has it.
pub(super) fn request_device(
    adapter: &wgpu::Adapter,
) -> Result<(wgpu::Device, wgpu::Queue), String> {
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("brood gui"),
        // Dual-source blending where the adapter has it: subpixel text in one pass
        // (`LCD_SRC`) rather than three (`LcdText`). `BROOD_GUI_NO_DUAL_SOURCE=1` declines
        // it, to see the three-pass path on an adapter that has the feature.
        required_features: if std::env::var("BROOD_GUI_NO_DUAL_SOURCE").as_deref() == Ok("1") {
            wgpu::Features::empty()
        } else {
            adapter.features() & wgpu::Features::DUAL_SOURCE_BLENDING
        },
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    }))
    .map_err(|e| format!("gpu device: {e}"))
}

/// The texture an offscreen target draws into: the window's size and format, drawable
/// through its sRGB view as a swapchain image is, and copyable for the readback.
#[cfg(test)]
pub(super) fn offscreen_texture(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen frame"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: config.format,
        usage: config.usage,
        view_formats: &config.view_formats,
    })
}
